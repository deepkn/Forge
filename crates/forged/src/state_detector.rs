//! Agent state detection via output heuristics.
//!
//! Analyzes agent stdout to infer state transitions (Running → Waiting,
//! Running → Error, etc.) without requiring explicit MCP reporting.

use forge_core::types::AgentState;

/// Analyze a line of agent output and return a state transition if one is detected.
///
/// Returns `None` if the line doesn't match any known pattern (no state change).
/// Only returns high-confidence matches to avoid false positives.
pub fn detect_state(line: &str) -> Option<AgentState> {
    let trimmed = line.trim();

    // Skip empty lines and ANSI-only sequences
    if trimmed.is_empty() {
        return None;
    }

    // ─── Error detection ────────────────────────────────────────────
    if is_error_line(trimmed) {
        return Some(AgentState::Error);
    }

    // ─── Done detection ─────────────────────────────────────────────
    if is_done_line(trimmed) {
        return Some(AgentState::Done);
    }

    // ─── Waiting / prompt detection ─────────────────────────────────
    if is_waiting_line(trimmed) {
        return Some(AgentState::Waiting);
    }

    None
}

/// Check if a line indicates an error state.
fn is_error_line(line: &str) -> bool {
    let lower = line.to_lowercase();

    // Fatal/panic patterns (high confidence)
    if lower.starts_with("fatal:") || lower.starts_with("panic:") {
        return true;
    }

    // "error:" at start of line (common in compiler/tool output)
    if lower.starts_with("error:") || lower.starts_with("error[") {
        return true;
    }

    // Thread panic messages
    if lower.contains("thread '") && lower.contains("panicked at") {
        return true;
    }

    false
}

/// Check if a line indicates the agent is done.
fn is_done_line(line: &str) -> bool {
    let lower = line.to_lowercase();

    // Common exit/completion patterns
    if lower.contains("process exited") || lower.contains("exited with") {
        return true;
    }

    false
}

/// Check if a line indicates the agent is waiting for input.
fn is_waiting_line(line: &str) -> bool {
    // Strip ANSI escape sequences for prompt detection
    let clean = strip_ansi(line);
    let trimmed = clean.trim();

    // Common prompt endings (must be at end of line)
    let prompt_endings = ["> ", "$ ", "% ", "? ", ">>> ", "... "];
    for ending in &prompt_endings {
        if trimmed.ends_with(ending) || trimmed.ends_with(ending.trim()) && trimmed.len() < 200 {
            // Only match short-ish lines (prompts are typically short)
            if trimmed.len() < 200 {
                return true;
            }
        }
    }

    // "Y/n", "y/N" prompt patterns
    if trimmed.contains("[Y/n]") || trimmed.contains("[y/N]") || trimmed.contains("(y/n)") {
        return true;
    }

    false
}

/// Strip ANSI escape sequences from a string for pattern matching.
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip ESC [ ... <letter> sequences
            if let Some(next) = chars.next() {
                if next == '[' {
                    // CSI sequence: skip until we find a letter
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() || c == '~' {
                            break;
                        }
                    }
                } else if next == ']' {
                    // OSC sequence: skip until BEL or ST
                    for c in chars.by_ref() {
                        if c == '\x07' {
                            break;
                        }
                        if c == '\x1b' {
                            // Check for ST (\x1b\\)
                            let _ = chars.next();
                            break;
                        }
                    }
                }
                // Other ESC sequences: skip the next char
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detects_error_patterns() {
        assert_eq!(detect_state("error: could not compile"), Some(AgentState::Error));
        assert_eq!(detect_state("error[E0308]: mismatched types"), Some(AgentState::Error));
        assert_eq!(detect_state("fatal: not a git repository"), Some(AgentState::Error));
        assert_eq!(detect_state("panic: runtime error"), Some(AgentState::Error));
        assert_eq!(
            detect_state("thread 'main' panicked at 'index out of bounds'"),
            Some(AgentState::Error)
        );
    }

    #[test]
    fn test_detects_done_patterns() {
        assert_eq!(detect_state("process exited with code 0"), Some(AgentState::Done));
        assert_eq!(detect_state("Process exited successfully"), Some(AgentState::Done));
    }

    #[test]
    fn test_detects_waiting_patterns() {
        assert_eq!(detect_state("> "), Some(AgentState::Waiting));
        assert_eq!(detect_state("$ "), Some(AgentState::Waiting));
        assert_eq!(detect_state(">>> "), Some(AgentState::Waiting));
        assert_eq!(detect_state("Continue? [Y/n]"), Some(AgentState::Waiting));
        assert_eq!(detect_state("Proceed? (y/n)"), Some(AgentState::Waiting));
    }

    #[test]
    fn test_no_false_positives() {
        // Normal output should not trigger state changes
        assert_eq!(detect_state("Compiling forge-core v0.1.0"), None);
        assert_eq!(detect_state("   Downloading crate"), None);
        assert_eq!(detect_state("warning: unused variable"), None);
        assert_eq!(detect_state(""), None);
        // Long lines with > should not false-positive as prompts
        assert_eq!(
            detect_state("This is a very long line that mentions something > something else and is not a prompt at all, it just contains the > character somewhere in the middle"),
            None
        );
    }

    #[test]
    fn test_ansi_stripping() {
        // ANSI-wrapped prompt
        assert_eq!(
            detect_state("\x1b[32m>\x1b[0m "),
            Some(AgentState::Waiting)
        );
    }
}
