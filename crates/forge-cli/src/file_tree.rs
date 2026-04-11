//! File tree data model — maintains directory structure and agent overlays.

use forge_core::types::{AgentId, LockMode};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A node in the file tree.
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
    pub children: Vec<TreeNode>,
}

/// Lock info for display.
#[derive(Debug, Clone)]
pub struct FileLockInfo {
    pub agent_id: AgentId,
    pub mode: LockMode,
}

/// Manages the file tree state.
pub struct FileTreeState {
    pub root: Option<TreeNode>,
    /// Flattened visible nodes for rendering and navigation.
    pub visible: Vec<VisibleNode>,
    /// Currently selected index in the visible list.
    pub selected: usize,
    /// File path → list of agents holding locks.
    pub locks: HashMap<String, Vec<FileLockInfo>>,
    /// File path → agent currently editing.
    pub active_edits: HashMap<String, AgentId>,
    /// Whether the detail section is expanded for the selected file.
    pub detail_expanded: bool,
}

/// A flattened node for rendering.
#[derive(Debug, Clone)]
pub struct VisibleNode {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
}

impl FileTreeState {
    pub fn new() -> Self {
        Self {
            root: None,
            visible: Vec::new(),
            selected: 0,
            locks: HashMap::new(),
            active_edits: HashMap::new(),
            detail_expanded: false,
        }
    }

    /// Scan a directory and build the tree.
    pub fn scan(&mut self, root_path: &Path) {
        self.root = Some(scan_directory(root_path, 0, 2));
        // Expand root by default
        if let Some(ref mut root) = self.root {
            root.expanded = true;
        }
        self.rebuild_visible();
    }

    /// Rebuild the flattened visible list from the tree.
    pub fn rebuild_visible(&mut self) {
        self.visible.clear();
        if let Some(ref root) = self.root {
            flatten_tree(root, &mut self.visible);
        }
        // Clamp selection
        if !self.visible.is_empty() && self.selected >= self.visible.len() {
            self.selected = self.visible.len() - 1;
        }
    }

    /// Move selection down.
    pub fn select_next(&mut self) {
        if !self.visible.is_empty() {
            self.selected = (self.selected + 1).min(self.visible.len() - 1);
        }
    }

    /// Move selection up.
    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Toggle expand/collapse on a directory, or no-op on a file.
    pub fn toggle_expand(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let node = &self.visible[self.selected];
        if !node.is_dir {
            return;
        }
        let path = node.path.clone();
        // Find and toggle in the tree
        if let Some(ref mut root) = self.root {
            toggle_node(root, &path);
        }
        self.rebuild_visible();
    }

    /// Expand a directory. If it's a file, no-op.
    pub fn expand(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let node = &self.visible[self.selected];
        if node.is_dir && !node.expanded {
            let path = node.path.clone();
            if let Some(ref mut root) = self.root {
                set_expanded(root, &path, true);
            }
            self.rebuild_visible();
        }
    }

    /// Collapse a directory. If on a file, collapse its parent.
    pub fn collapse(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let node = &self.visible[self.selected];
        if node.is_dir && node.expanded {
            let path = node.path.clone();
            if let Some(ref mut root) = self.root {
                set_expanded(root, &path, false);
            }
            self.rebuild_visible();
        } else if node.depth > 0 {
            // Move to parent directory
            let target_depth = node.depth - 1;
            for i in (0..self.selected).rev() {
                if self.visible[i].is_dir && self.visible[i].depth == target_depth {
                    self.selected = i;
                    break;
                }
            }
        }
    }

    /// Get the currently selected path.
    pub fn selected_path(&self) -> Option<&Path> {
        self.visible.get(self.selected).map(|n| n.path.as_path())
    }

    /// Get lock info for a given path.
    pub fn locks_for(&self, path: &str) -> Option<&Vec<FileLockInfo>> {
        self.locks.get(path)
    }

    /// Update lock state from a NATS event.
    pub fn set_locks(&mut self, path: String, locks: Vec<FileLockInfo>) {
        if locks.is_empty() {
            self.locks.remove(&path);
        } else {
            self.locks.insert(path, locks);
        }
    }

    /// Mark a file as being actively edited by an agent.
    pub fn set_active_edit(&mut self, path: String, agent_id: AgentId) {
        self.active_edits.insert(path, agent_id);
    }

    /// Clear active edit marker.
    pub fn clear_active_edit(&mut self, path: &str) {
        self.active_edits.remove(path);
    }
}

/// Scan a directory recursively up to max_depth.
fn scan_directory(path: &Path, depth: usize, max_initial_depth: usize) -> TreeNode {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(".")
        .to_string();

    let mut children = Vec::new();

    if path.is_dir() && depth < max_initial_depth {
        if let Ok(entries) = std::fs::read_dir(path) {
            let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
            // Sort: directories first, then alphabetical
            entries.sort_by(|a, b| {
                let a_dir = a.path().is_dir();
                let b_dir = b.path().is_dir();
                match (a_dir, b_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.file_name().cmp(&b.file_name()),
                }
            });

            for entry in entries {
                let entry_path = entry.path();
                let entry_name = entry
                    .file_name()
                    .to_str()
                    .unwrap_or("")
                    .to_string();

                // Skip hidden files and common noise
                if entry_name.starts_with('.') || entry_name == "target" || entry_name == "node_modules" {
                    continue;
                }

                children.push(scan_directory(&entry_path, depth + 1, max_initial_depth));
            }
        }
    }

    TreeNode {
        name,
        path: path.to_path_buf(),
        is_dir: path.is_dir(),
        depth,
        expanded: depth == 0, // Root is expanded by default
        children,
    }
}

/// Flatten the tree into a list of visible nodes (respecting expanded state).
fn flatten_tree(node: &TreeNode, out: &mut Vec<VisibleNode>) {
    out.push(VisibleNode {
        name: node.name.clone(),
        path: node.path.clone(),
        is_dir: node.is_dir,
        depth: node.depth,
        expanded: node.expanded,
    });

    if node.is_dir && node.expanded {
        for child in &node.children {
            flatten_tree(child, out);
        }
    }
}

/// Toggle expanded state of a node at the given path.
fn toggle_node(node: &mut TreeNode, path: &Path) -> bool {
    if node.path == path {
        node.expanded = !node.expanded;
        // Lazy load children if expanding and empty
        if node.expanded && node.children.is_empty() && node.is_dir {
            if let Ok(entries) = std::fs::read_dir(&node.path) {
                let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
                entries.sort_by(|a, b| {
                    let a_dir = a.path().is_dir();
                    let b_dir = b.path().is_dir();
                    match (a_dir, b_dir) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        _ => a.file_name().cmp(&b.file_name()),
                    }
                });
                for entry in entries {
                    let entry_name = entry.file_name().to_str().unwrap_or("").to_string();
                    if entry_name.starts_with('.') || entry_name == "target" || entry_name == "node_modules" {
                        continue;
                    }
                    let entry_path = entry.path();
                    node.children.push(TreeNode {
                        name: entry_name,
                        path: entry_path.clone(),
                        is_dir: entry_path.is_dir(),
                        depth: node.depth + 1,
                        expanded: false,
                        children: Vec::new(),
                    });
                }
            }
        }
        return true;
    }
    for child in &mut node.children {
        if toggle_node(child, path) {
            return true;
        }
    }
    false
}

/// Set expanded state of a specific node.
fn set_expanded(node: &mut TreeNode, path: &Path, expanded: bool) -> bool {
    if node.path == path {
        node.expanded = expanded;
        // Lazy load on expand
        if expanded && node.children.is_empty() && node.is_dir {
            if let Ok(entries) = std::fs::read_dir(&node.path) {
                let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
                entries.sort_by(|a, b| {
                    let a_dir = a.path().is_dir();
                    let b_dir = b.path().is_dir();
                    match (a_dir, b_dir) {
                        (true, false) => std::cmp::Ordering::Less,
                        (false, true) => std::cmp::Ordering::Greater,
                        _ => a.file_name().cmp(&b.file_name()),
                    }
                });
                for entry in entries {
                    let entry_name = entry.file_name().to_str().unwrap_or("").to_string();
                    if entry_name.starts_with('.') || entry_name == "target" || entry_name == "node_modules" {
                        continue;
                    }
                    let entry_path = entry.path();
                    node.children.push(TreeNode {
                        name: entry_name,
                        path: entry_path.clone(),
                        is_dir: entry_path.is_dir(),
                        depth: node.depth + 1,
                        expanded: false,
                        children: Vec::new(),
                    });
                }
            }
        }
        return true;
    }
    for child in &mut node.children {
        if set_expanded(child, path, expanded) {
            return true;
        }
    }
    false
}
