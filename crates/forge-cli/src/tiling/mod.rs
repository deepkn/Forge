//! Tiling window manager for the center pane.
//!
//! Uses a binary tree structure where each leaf is an agent pane
//! and each branch is a split (horizontal or vertical).

use forge_core::types::AgentId;
use serde::{Deserialize, Serialize};

/// Direction of a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

/// Node in the tiling tree.
#[derive(Debug, Serialize, Deserialize)]
pub enum TilingNode {
    Leaf {
        agent_id: Option<AgentId>,
    },
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<TilingNode>,
        second: Box<TilingNode>,
    },
}

/// Manages the tiling layout.
pub struct TilingLayout {
    root: Option<TilingNode>,
    focused_index: usize,
    pane_count: usize,
}

const MIN_RATIO: f32 = 0.1;
const MAX_RATIO: f32 = 0.9;
const RESIZE_STEP: f32 = 0.05;

impl TilingLayout {
    pub fn new() -> Self {
        Self {
            root: None,
            focused_index: 0,
            pane_count: 0,
        }
    }

    pub fn has_panes(&self) -> bool {
        self.root.is_some()
    }

    pub fn pane_count(&self) -> usize {
        self.pane_count
    }

    /// Add a new pane (leaf) with the given agent.
    pub fn add_pane(&mut self, agent_id: AgentId) {
        let new_leaf = TilingNode::Leaf {
            agent_id: Some(agent_id),
        };

        self.root = Some(match self.root.take() {
            None => new_leaf,
            Some(existing) => TilingNode::Split {
                direction: if self.pane_count % 2 == 0 {
                    SplitDirection::Horizontal
                } else {
                    SplitDirection::Vertical
                },
                ratio: 0.5,
                first: Box::new(existing),
                second: Box::new(new_leaf),
            },
        });

        self.pane_count += 1;
        self.focused_index = self.pane_count - 1;
    }

    /// Split the focused pane horizontally.
    pub fn split_horizontal(&mut self) {
        self.split_focused(SplitDirection::Horizontal);
    }

    /// Split the focused pane vertically.
    pub fn split_vertical(&mut self) {
        self.split_focused(SplitDirection::Vertical);
    }

    fn split_focused(&mut self, direction: SplitDirection) {
        if self.root.is_none() {
            // Create first pane
            self.root = Some(TilingNode::Leaf { agent_id: None });
            self.pane_count = 1;
            self.focused_index = 0;
            return;
        }

        let mut counter = 0usize;
        let target = self.focused_index;
        if let Some(ref mut root) = self.root {
            split_leaf_at(root, target, direction, &mut counter);
        }
        self.pane_count += 1;
        // Focus the new (second) pane
        self.focused_index += 1;
    }

    /// Close the focused pane.
    pub fn close_focused(&mut self) {
        if self.pane_count <= 1 {
            self.root = None;
            self.pane_count = 0;
            self.focused_index = 0;
            return;
        }

        let mut counter = 0usize;
        let target = self.focused_index;
        if let Some(root) = self.root.take() {
            self.root = remove_leaf_at(root, target, &mut counter);
        }
        self.pane_count -= 1;
        if self.focused_index >= self.pane_count {
            self.focused_index = self.pane_count.saturating_sub(1);
        }
    }

    /// Resize the split containing the focused pane.
    pub fn resize_focused(&mut self, grow_first: bool) {
        if let Some(ref mut root) = self.root {
            let mut counter = 0usize;
            resize_at(root, self.focused_index, grow_first, &mut counter);
        }
    }

    /// Focus the next pane.
    pub fn focus_next(&mut self) {
        if self.pane_count > 0 {
            self.focused_index = (self.focused_index + 1) % self.pane_count;
        }
    }

    /// Focus the previous pane.
    pub fn focus_prev(&mut self) {
        if self.pane_count > 0 {
            self.focused_index = if self.focused_index == 0 {
                self.pane_count - 1
            } else {
                self.focused_index - 1
            };
        }
    }

    /// Get the root node for rendering.
    pub fn root(&self) -> Option<&TilingNode> {
        self.root.as_ref()
    }

    /// Get the focused pane index.
    pub fn focused_index(&self) -> usize {
        self.focused_index
    }

    /// Collect all leaf agent IDs in order (for rendering).
    pub fn leaves(&self) -> Vec<Option<AgentId>> {
        let mut result = Vec::new();
        if let Some(root) = &self.root {
            collect_leaves(root, &mut result);
        }
        result
    }

    /// Serialize the tiling layout to a JSON string for session persistence.
    pub fn serialize(&self) -> Option<String> {
        let root = self.root.as_ref()?;
        serde_json::to_string(&LayoutSnapshot {
            root: root,
            focused_index: self.focused_index,
        }).ok()
    }

    /// Deserialize a tiling layout from a JSON string (on re-attach).
    pub fn deserialize(s: &str) -> Option<Self> {
        let snapshot: OwnedLayoutSnapshot = serde_json::from_str(s).ok()?;
        let pane_count = count_leaves(&snapshot.root);
        let focused_index = if snapshot.focused_index < pane_count {
            snapshot.focused_index
        } else {
            0
        };
        Some(Self {
            root: Some(snapshot.root),
            focused_index,
            pane_count,
        })
    }
}

/// Split the leaf at the given index into a split with two leaves.
fn split_leaf_at(
    node: &mut TilingNode,
    target: usize,
    direction: SplitDirection,
    counter: &mut usize,
) -> bool {
    match node {
        TilingNode::Leaf { agent_id } => {
            if *counter == target {
                // Replace this leaf with a split
                let existing = TilingNode::Leaf {
                    agent_id: agent_id.take(),
                };
                *node = TilingNode::Split {
                    direction,
                    ratio: 0.5,
                    first: Box::new(existing),
                    second: Box::new(TilingNode::Leaf { agent_id: None }),
                };
                return true;
            }
            *counter += 1;
            false
        }
        TilingNode::Split { first, second, .. } => {
            if split_leaf_at(first, target, direction, counter) {
                return true;
            }
            split_leaf_at(second, target, direction, counter)
        }
    }
}

/// Remove the leaf at the given index, returning the restructured tree.
fn remove_leaf_at(node: TilingNode, target: usize, counter: &mut usize) -> Option<TilingNode> {
    match node {
        TilingNode::Leaf { .. } => {
            if *counter == target {
                *counter += 1;
                None // Remove this leaf
            } else {
                *counter += 1;
                Some(TilingNode::Leaf {
                    agent_id: match node {
                        TilingNode::Leaf { agent_id } => agent_id,
                        _ => unreachable!(),
                    },
                })
            }
        }
        TilingNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let first_result = remove_leaf_at(*first, target, counter);
            let second_result = remove_leaf_at(*second, target, counter);

            match (first_result, second_result) {
                (None, None) => None,
                (Some(node), None) | (None, Some(node)) => Some(node),
                (Some(f), Some(s)) => Some(TilingNode::Split {
                    direction,
                    ratio,
                    first: Box::new(f),
                    second: Box::new(s),
                }),
            }
        }
    }
}

/// Resize the split containing the focused leaf.
fn resize_at(node: &mut TilingNode, target: usize, grow_first: bool, counter: &mut usize) -> bool {
    match node {
        TilingNode::Leaf { .. } => {
            let is_target = *counter == target;
            *counter += 1;
            is_target
        }
        TilingNode::Split {
            ratio,
            first,
            second,
            ..
        } => {
            let first_contains = resize_at(first, target, grow_first, counter);
            let second_contains = resize_at(second, target, grow_first, counter);

            if first_contains || second_contains {
                if grow_first {
                    *ratio = (*ratio + RESIZE_STEP).min(MAX_RATIO);
                } else {
                    *ratio = (*ratio - RESIZE_STEP).max(MIN_RATIO);
                }
                true
            } else {
                false
            }
        }
    }
}

fn collect_leaves(node: &TilingNode, out: &mut Vec<Option<AgentId>>) {
    match node {
        TilingNode::Leaf { agent_id } => out.push(agent_id.clone()),
        TilingNode::Split { first, second, .. } => {
            collect_leaves(first, out);
            collect_leaves(second, out);
        }
    }
}

fn count_leaves(node: &TilingNode) -> usize {
    match node {
        TilingNode::Leaf { .. } => 1,
        TilingNode::Split { first, second, .. } => count_leaves(first) + count_leaves(second),
    }
}

/// Snapshot structure for serialization (borrows the tree).
#[derive(Serialize)]
struct LayoutSnapshot<'a> {
    root: &'a TilingNode,
    focused_index: usize,
}

/// Owned snapshot for deserialization.
#[derive(Deserialize)]
struct OwnedLayoutSnapshot {
    root: TilingNode,
    focused_index: usize,
}
