//! Manages the embedded NATS server lifecycle.
//!
//! Looks for nats-server in: 1) PATH, 2) forge data dir, 3) auto-downloads it.

use anyhow::{Context, Result};
use forge_core::config::ForgeConfig;
use forge_core::config::NatsConfig;
use std::net::TcpListener;
use std::path::PathBuf;
use tokio::process::Command;
use tracing::info;

const NATS_VERSION: &str = "v2.10.24";

/// Start an embedded nats-server and return the connection URL.
///
/// Automatically downloads nats-server if not found.
pub async fn start_embedded_nats(config: &NatsConfig) -> Result<String> {
    let port = if config.port == 0 {
        find_available_port()?
    } else {
        config.port
    };

    let url = format!("nats://{}:{}", config.host, port);

    let nats_bin = find_or_install_nats().await?;
    info!("Using nats-server at: {}", nats_bin.display());

    let child = Command::new(&nats_bin)
        .arg("--port")
        .arg(port.to_string())
        .arg("--addr")
        .arg(&config.host)
        .arg("--log_size_limit")
        .arg("0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("Failed to start nats-server at {}", nats_bin.display()))?;

    // Leak the child handle so it lives as long as the daemon process.
    // On daemon shutdown, the child gets SIGHUP when the parent exits.
    std::mem::forget(child);

    wait_for_nats(&url).await?;

    Ok(url)
}

/// Find nats-server binary: check PATH, then forge bin dir, then auto-download.
async fn find_or_install_nats() -> Result<PathBuf> {
    // 1. Check PATH
    if let Ok(path) = which("nats-server") {
        return Ok(path);
    }

    // 2. Check forge bin directory
    let forge_bin = forge_nats_path();
    if forge_bin.exists() {
        return Ok(forge_bin);
    }

    // 3. Auto-download
    info!("nats-server not found — downloading {} ...", NATS_VERSION);
    download_nats(&forge_bin).await?;

    Ok(forge_bin)
}

/// Path where forge stores its bundled nats-server binary.
fn forge_nats_path() -> PathBuf {
    ForgeConfig::data_dir().join("bin").join("nats-server")
}

/// Simple which(1) implementation.
fn which(binary: &str) -> Result<PathBuf> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        let candidate = PathBuf::from(dir).join(binary);
        if candidate.exists() && candidate.is_file() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("{binary} not found in PATH")
}

/// Download nats-server for the current platform.
async fn download_nats(dest: &PathBuf) -> Result<()> {
    let (os, arch) = platform_tag()?;
    let filename = format!("nats-server-{NATS_VERSION}-{os}-{arch}.zip");
    let url = format!(
        "https://github.com/nats-io/nats-server/releases/download/{NATS_VERSION}/{filename}"
    );

    info!("Downloading {}", url);

    // Download to temp file
    let tmp_zip = std::env::temp_dir().join("forge-nats-download.zip");
    let output = tokio::process::Command::new("curl")
        .arg("-sfL")
        .arg("-o")
        .arg(&tmp_zip)
        .arg(&url)
        .output()
        .await
        .context("Failed to run curl — is curl installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to download nats-server: {stderr}");
    }

    // Extract
    let tmp_dir = std::env::temp_dir().join("forge-nats-extract");
    let _ = std::fs::remove_dir_all(&tmp_dir);
    let output = tokio::process::Command::new("unzip")
        .arg("-o")
        .arg(&tmp_zip)
        .arg("-d")
        .arg(&tmp_dir)
        .output()
        .await
        .context("Failed to run unzip — is unzip installed?")?;

    if !output.status.success() {
        anyhow::bail!("Failed to extract nats-server zip");
    }

    // Find the binary inside the extracted directory
    let extracted_name = format!("nats-server-{NATS_VERSION}-{os}-{arch}");
    let extracted_bin = tmp_dir.join(&extracted_name).join("nats-server");
    if !extracted_bin.exists() {
        anyhow::bail!(
            "Expected nats-server at {} but not found",
            extracted_bin.display()
        );
    }

    // Install to destination
    let dest_dir = dest.parent().unwrap();
    std::fs::create_dir_all(dest_dir)?;
    std::fs::copy(&extracted_bin, dest)?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
    }

    // Cleanup
    let _ = std::fs::remove_file(&tmp_zip);
    let _ = std::fs::remove_dir_all(&tmp_dir);

    info!("nats-server installed to {}", dest.display());
    Ok(())
}

/// Return (os, arch) strings matching NATS release naming.
fn platform_tag() -> Result<(&'static str, &'static str)> {
    let os = if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        anyhow::bail!("Unsupported OS for nats-server auto-download");
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        anyhow::bail!("Unsupported architecture for nats-server auto-download");
    };

    Ok((os, arch))
}

async fn wait_for_nats(url: &str) -> Result<()> {
    for attempt in 0..50 {
        match async_nats::connect(url).await {
            Ok(_) => return Ok(()),
            Err(_) if attempt < 49 => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => return Err(e).context("NATS server failed to start"),
        }
    }
    unreachable!()
}

fn find_available_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok(port)
}
