//! File lock manager — enforces read/write lock semantics over NATS.

use anyhow::Result;
use forge_core::config::LockConfig;
use futures_util::StreamExt;
use forge_core::protocol::{LockEntry, LockRequest, LockResponse};
use forge_core::subjects::FileLockSubjects;
use forge_core::types::LockMode;
use forge_core::db;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

/// Handle to the running lock manager.
pub struct LockManagerHandle {
    _task: JoinHandle<()>,
}

/// Start the lock manager, listening on NATS for lock requests.
pub fn start(
    db: Arc<Mutex<Connection>>,
    nats: async_nats::Client,
    config: &LockConfig,
) -> LockManagerHandle {
    let idle_timeout = config.idle_timeout_secs;

    let task = tokio::spawn(async move {
        if let Err(e) = run_lock_manager(db, nats, idle_timeout).await {
            error!("Lock manager exited with error: {e}");
        }
    });

    LockManagerHandle { _task: task }
}

async fn run_lock_manager(
    db: Arc<Mutex<Connection>>,
    nats: async_nats::Client,
    idle_timeout: u64,
) -> Result<()> {
    let mut sub = nats.subscribe(FileLockSubjects::request()).await?;
    info!("Lock manager listening on {}", FileLockSubjects::request());

    // Idle timeout: release locks that have been held past the configured threshold.
    if idle_timeout > 0 {
        let db_clone = Arc::clone(&db);
        let nats_clone = nats.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(idle_timeout));
            interval.tick().await; // skip the immediate first tick
            loop {
                interval.tick().await;
                let stale = {
                    let conn = db_clone.lock().unwrap();
                    db::get_stale_lock_paths(&conn, idle_timeout).unwrap_or_default()
                };
                if !stale.is_empty() {
                    for path in &stale {
                        let conn = db_clone.lock().unwrap();
                        let _ = db::force_release_lock(&conn, path);
                        info!("Auto-released idle lock on {path}");
                    }
                    if let Err(e) = publish_lock_state(&db_clone, &nats_clone).await {
                        warn!("Failed to broadcast lock state after idle release: {e}");
                    }
                }
            }
        });
    }

    while let Some(msg) = sub.next().await {
        let request: LockRequest = match serde_json::from_slice(&msg.payload) {
            Ok(r) => r,
            Err(e) => {
                warn!("Invalid lock request: {e}");
                continue;
            }
        };

        let response = {
            let conn = db.lock().unwrap();
            handle_request(&conn, &request)
        };

        let response = match response {
            Ok(r) => r,
            Err(e) => {
                error!("Lock request handling error: {e}");
                continue;
            }
        };

        // Reply if there's a reply subject
        let payload = serde_json::to_vec(&response).unwrap();
        if let Some(reply) = msg.reply {
            if let Err(e) = nats.publish(reply, payload.into()).await {
                error!("Failed to send lock response: {e}");
            }
        }

        // Broadcast state change
        if let Err(e) = publish_lock_state(&db, &nats).await {
            warn!("Failed to broadcast lock state: {e}");
        }
    }

    Ok(())
}

fn handle_request(conn: &Connection, request: &LockRequest) -> Result<LockResponse> {
    match request {
        LockRequest::Acquire {
            agent_id,
            path,
            mode,
        } => {
            let granted = db::acquire_lock(conn, path, agent_id, mode)?;
            if granted {
                Ok(LockResponse::Granted {
                    path: path.clone(),
                    mode: mode.clone(),
                })
            } else {
                Ok(LockResponse::Denied {
                    path: path.clone(),
                    reason: match mode {
                        LockMode::Write => "File is locked by another agent".to_string(),
                        LockMode::Read => "File has an active writer".to_string(),
                    },
                })
            }
        }
        LockRequest::Release { agent_id, path } => {
            db::release_lock(conn, path, agent_id)?;
            Ok(LockResponse::Released {
                path: path.clone(),
            })
        }
        LockRequest::ForceRelease { path } => {
            db::force_release_lock(conn, path)?;
            Ok(LockResponse::Released {
                path: path.clone(),
            })
        }
        LockRequest::Query => {
            let locks = db::get_all_locks(conn)?;
            Ok(LockResponse::State {
                locks: locks
                    .into_iter()
                    .map(|(path, agent_id, mode, acquired_at)| LockEntry {
                        path,
                        agent_id,
                        mode,
                        acquired_at,
                    })
                    .collect(),
            })
        }
    }
}

async fn publish_lock_state(
    db: &Arc<Mutex<Connection>>,
    nats: &async_nats::Client,
) -> Result<()> {
    let locks = {
        let conn = db.lock().unwrap();
        db::get_all_locks(&conn)?
    };

    let state = LockResponse::State {
        locks: locks
            .into_iter()
            .map(|(path, agent_id, mode, acquired_at)| LockEntry {
                path,
                agent_id,
                mode,
                acquired_at,
            })
            .collect(),
    };

    let payload = serde_json::to_vec(&state)?;
    nats.publish(FileLockSubjects::state(), payload.into())
        .await?;

    Ok(())
}
