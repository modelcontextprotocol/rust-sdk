//! Conformance tests for the skills feature.
//!
//! Validates FileSystemSkillServer against the MCP spec vectors:
//! - skills/list returns all discovered SKILL.md files sorted by URI
//! - skills/get returns a single entry by URI
//! - skills/get returns an error for unknown URIs
//! - resources/directory/read lists sibling files (multi-file skills)
//! - frontmatter is parsed correctly
//!
//! The tests spawn the server binary as a child process and communicate
//! via stdio JSON-RPC. This validates the actual binary, not just the types.

use std::{
    io::Write,
    process::{Command, Stdio},
};

fn skills_dir() -> std::path::PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let crate_root = std::path::PathBuf::from(manifest_dir);
    let workspace_root = crate_root
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(&crate_root);
    workspace_root.join("fixtures/skills-dir")
}

fn build_request(id: u64, method: &str, params: serde_json::Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    })
    .to_string()
}

fn run_server_request(requests: &[String]) -> Vec<serde_json::Value> {
    let mut child = Command::new("cargo")
        .args([
            "run",
            "--example",
            "skill_fs_server",
            "--features",
            "server,transport-io",
            "--",
            "--root",
        ])
        .arg(skills_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn server");

    {
        let stdin = child.stdin.as_mut().expect("failed to open stdin");
        for req in requests {
            writeln!(stdin, "{}", req).expect("failed to write request");
        }
        // Close stdin to signal EOF and let the server exit
    }

    let output = child.wait_with_output().expect("failed to wait for server");
    let stdout = String::from_utf8_lossy(&output.stdout);

    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            serde_json::from_str::<serde_json::Value>(line).ok()
        })
        .collect()
}

#[test]
fn skills_list_returns_all_discovered() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-03-26",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(2, "skills/list", serde_json::json!({})),
    ];

    let responses = run_server_request(&requests);
    let skills_list = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skills"].as_array())
        .expect("should have skills/list response");

    assert_eq!(skills_list.len(), 3, "expected 3 skills from fixtures");
    let uris: Vec<&str> = skills_list
        .iter()
        .map(|s| s["uri"].as_str().unwrap_or(""))
        .collect();
    let mut sorted = uris.clone();
    sorted.sort();
    assert_eq!(uris, sorted, "skills should be sorted by URI");
}

#[test]
fn skills_list_uris_are_valid() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-03-26",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(2, "skills/list", serde_json::json!({})),
    ];

    let responses = run_server_request(&requests);
    let skills_list = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skills"].as_array())
        .expect("should have skills/list response");

    for skill in skills_list {
        let uri = skill["uri"].as_str().unwrap_or("");
        assert!(
            uri.starts_with("skill://"),
            "URI must start with skill://: {}",
            uri
        );
        assert!(
            uri.ends_with("/SKILL.md"),
            "URI must end with /SKILL.md: {}",
            uri
        );
    }
}

#[test]
fn skills_get_returns_correct_entry() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-03-26",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(
            2,
            "skills/get",
            serde_json::json!({"uri": "skill://billing/refunds/SKILL.md"}),
        ),
    ];

    let responses = run_server_request(&requests);
    let skill = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skill"].as_object())
        .expect("should have skills/get response");

    assert_eq!(skill["uri"], "skill://billing/refunds/SKILL.md");
    assert_eq!(
        skill["frontmatter"]["name"], "refunds",
        "frontmatter name should match"
    );
}

#[test]
fn skills_get_returns_error_for_unknown() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-03-26",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(
            2,
            "skills/get",
            serde_json::json!({"uri": "skill://nonexistent/SKILL.md"}),
        ),
    ];

    let responses = run_server_request(&requests);
    let error = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["error"].as_object())
        .expect("should have error response for unknown skill");

    assert!(error["code"].is_number(), "error should have a code");
}

#[test]
fn multi_file_skill_lists_siblings() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-03-26",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(
            2,
            "skills/get",
            serde_json::json!({"uri": "skill://billing/refunds/SKILL.md"}),
        ),
    ];

    let responses = run_server_request(&requests);
    let resources = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skill"]["resources"].as_array())
        .expect("should have resources for multi-file skill");

    let names: Vec<&str> = resources
        .iter()
        .map(|r| r["uri"].as_str().unwrap_or(""))
        .filter(|uri| uri.ends_with(".md"))
        .collect();
    assert_eq!(names.len(), 2, "refunds skill should have 2 siblings");
    assert!(
        names.iter().any(|uri| uri.ends_with("/FORMS.md")),
        "should contain FORMS.md"
    );
    assert!(
        names.iter().any(|uri| uri.ends_with("/APPENDIX.md")),
        "should contain APPENDIX.md"
    );
}

#[test]
fn single_file_skill_has_no_resources() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-03-26",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(
            2,
            "skills/get",
            serde_json::json!({"uri": "skill://git-workflow/SKILL.md"}),
        ),
    ];

    let responses = run_server_request(&requests);
    let resources = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skill"]["resources"].as_array());

    assert!(
        resources.is_none() || resources.unwrap().is_empty(),
        "single-file skill should have no resources"
    );
}

#[test]
fn frontmatter_has_required_fields() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-03-26",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(2, "skills/list", serde_json::json!({})),
    ];

    let responses = run_server_request(&requests);
    let skills_list = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skills"].as_array())
        .expect("should have skills/list response");

    for skill in skills_list {
        let fm = skill["frontmatter"]
            .as_object()
            .expect("should have frontmatter");
        assert!(fm.get("name").is_some(), "skill must have a name");
        assert!(
            fm.get("description").is_some(),
            "skill must have a description"
        );
    }
}

#[test]
fn doc_workflow_skill_exists() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-03-26",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(
            2,
            "skills/get",
            serde_json::json!({"uri": "skill://doc-workflow/SKILL.md"}),
        ),
    ];

    let responses = run_server_request(&requests);
    let skill = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skill"].as_object())
        .expect("doc-workflow skill should exist");

    assert_eq!(skill["uri"], "skill://doc-workflow/SKILL.md");
}

#[test]
fn billing_refunds_frontmatter_has_version() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-03-26",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(
            2,
            "skills/get",
            serde_json::json!({"uri": "skill://billing/refunds/SKILL.md"}),
        ),
    ];

    let responses = run_server_request(&requests);
    let version = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skill"]["frontmatter"]["version"].as_str())
        .expect("should have version field");

    assert_eq!(
        version, "1.0.0",
        "version should be parsed from frontmatter"
    );
}

#[test]
fn multi_file_skill_has_resources_field() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-03-26",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(
            2,
            "skills/get",
            serde_json::json!({"uri": "skill://billing/refunds/SKILL.md"}),
        ),
    ];

    let responses = run_server_request(&requests);
    let resources = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skill"]["resources"].as_array())
        .expect("multi-file skill should have resources");

    assert_eq!(resources.len(), 2, "should have 2 file entries");
}
