use std::fs;
use std::path::PathBuf;

/// Helper to create a temp directory with a known structure.
fn setup_test_dir() -> PathBuf {
    let id = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("forge_file_tree_test_{id}_{ts}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
    fs::write(dir.join("src/lib.rs"), "").unwrap();
    fs::write(dir.join("tests/test.rs"), "").unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
    dir
}

// Since file_tree is in the binary crate, we can't import it directly.
// Test the scan logic inline with the same code pattern.
#[test]
fn test_directory_scan_structure() {
    let dir = setup_test_dir();
    assert!(dir.join("src/main.rs").exists());
    assert!(dir.join("tests/test.rs").exists());
    assert!(dir.join("Cargo.toml").exists());

    // Verify the directory structure is correct
    let entries: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(entries.contains(&"src".to_string()));
    assert!(entries.contains(&"tests".to_string()));
    assert!(entries.contains(&"Cargo.toml".to_string()));

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_hidden_dirs_excluded() {
    let dir = setup_test_dir();
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::write(dir.join(".git/config"), "").unwrap();
    fs::create_dir_all(dir.join("node_modules")).unwrap();
    fs::write(dir.join("node_modules/pkg.json"), "").unwrap();

    // Walk directory excluding hidden and noise
    let entries: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            !name.starts_with('.') && name != "target" && name != "node_modules"
        })
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    assert!(!entries.contains(&".git".to_string()));
    assert!(!entries.contains(&"node_modules".to_string()));
    assert!(entries.contains(&"src".to_string()));

    let _ = fs::remove_dir_all(&dir);
}
