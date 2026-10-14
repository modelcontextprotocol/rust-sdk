//! The published crates must ship the repository LICENSE (see #1335).
//! `cargo package` only picks up files inside the crate directory, so each
//! crate keeps a LICENSE symlink to the workspace root file.

use std::path::Path;

fn assert_crate_license(crate_dir: &Path) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let expected = std::fs::read_to_string(root.join("LICENSE")).expect("root LICENSE");
    let actual = std::fs::read_to_string(crate_dir.join("LICENSE"))
        .unwrap_or_else(|e| panic!("{} has no LICENSE: {e}", crate_dir.display()));
    assert_eq!(
        actual,
        expected,
        "{}/LICENSE differs from root",
        crate_dir.display()
    );
}

#[test]
fn rmcp_crate_ships_license() {
    assert_crate_license(Path::new(env!("CARGO_MANIFEST_DIR")));
}

#[test]
fn rmcp_macros_crate_ships_license() {
    assert_crate_license(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../rmcp-macros"));
}
