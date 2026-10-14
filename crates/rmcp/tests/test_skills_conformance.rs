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
    // SKILL.md + 2 siblings = 3 entries
    assert_eq!(
        names.len(),
        3,
        "refunds skill should have 3 entries (SKILL.md + 2 siblings)"
    );
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

    // SKILL.md itself is included in resources.
    // A single-file skill should have exactly 1 resource (SKILL.md).
    let resources = resources.expect("single-file skill should have resources");
    assert_eq!(
        resources.len(),
        1,
        "single-file skill should have SKILL.md in resources"
    );
    assert_eq!(
        resources[0]["uri"].as_str().unwrap(),
        "skill://git-workflow/SKILL.md",
        "resource should be SKILL.md"
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

    // SKILL.md + 2 siblings = 3 entries
    assert_eq!(
        resources.len(),
        3,
        "should have 3 file entries (SKILL.md + 2 siblings)"
    );
}

/// `resources/directory/read` is the request that actually lists a skill's
/// sibling files. The header comment has always claimed this coverage, but no
/// test called it — `skills/get` returning a `resources` field is a different
/// code path. This exercises the real method.
#[test]
fn resources_directory_read_lists_siblings() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(
            2,
            "resources/directory/read",
            serde_json::json!({"uri": "skill://billing/refunds"}),
        ),
    ];

    let responses = run_server_request(&requests);
    let resources = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["resources"].as_array())
        .expect("resources/directory/read should return a resources array");

    // Spec, Directory Listing: "The result contains every direct child of the
    // directory." SKILL.md is a file directly inside the skill root, so it IS a
    // direct child and MUST be listed. Expect SKILL.md + the 2 siblings = 3.
    assert_eq!(
        resources.len(),
        3,
        "refunds directory should list every direct child (SKILL.md + 2 siblings)"
    );
    assert!(
        resources
            .iter()
            .any(|r| r["uri"].as_str().unwrap_or("").ends_with("/SKILL.md")),
        "listing must contain SKILL.md — it is a direct child of the skill root"
    );
    assert!(
        resources
            .iter()
            .any(|r| r["uri"].as_str().unwrap_or("").ends_with("/FORMS.md")),
        "listing should contain FORMS.md"
    );
    assert!(
        resources
            .iter()
            .any(|r| r["uri"].as_str().unwrap_or("").ends_with("/APPENDIX.md")),
        "listing should contain APPENDIX.md"
    );
}

/// `resources/read` returns the bytes of one skill file. Also claimed by the
/// header, also never actually called before this test.
#[test]
fn resources_read_returns_file_contents() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        build_request(
            2,
            "resources/read",
            serde_json::json!({"uri": "skill://billing/refunds/FORMS.md"}),
        ),
    ];

    let responses = run_server_request(&requests);
    let contents = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["contents"].as_array())
        .expect("resources/read should return a contents array");

    assert_eq!(contents.len(), 1, "should return exactly one content block");
    let text = contents[0]["text"].as_str().unwrap_or("");
    assert!(
        text.contains("Form 101"),
        "file contents should be returned verbatim"
    );
}

// ---------------------------------------------------------------------------
// Macro-generated server — skill_macro_server example
// ---------------------------------------------------------------------------

/// Drive the macro server over stdio, mirroring `run_server_request` for the
/// hand-written `skill_fs_server`. This is the only test that exercises the
/// macro expansion path (`#[skill]` + `#[skill_router]` + `#[skill_handler]`)
/// end-to-end through a real process.
fn run_macro_server_request(requests: &[String]) -> Vec<serde_json::Value> {
    let mut child = Command::new("cargo")
        .args([
            "run",
            "--example",
            "skill_macro_server",
            "--features",
            "server,transport-io",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn macro server");

    {
        let stdin = child.stdin.as_mut().expect("failed to open stdin");
        for req in requests {
            writeln!(stdin, "{}", req).expect("failed to write request");
        }
        // Close stdin to signal EOF and let the server exit.
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

/// The macro-generated server must answer `skills/list` and `skills/get` over
/// the wire. Regression guard: the example must hold its `RunningService`
/// (`.waiting()`), or the serve loop is cancelled on drop and the process
/// exits after `initialize` without answering anything else.
#[test]
fn macro_server_skills_list_and_get() {
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
        // The initialized notification is required before the server will
        // answer subsequent requests.
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        })
        .to_string(),
        build_request(2, "skills/list", serde_json::json!({})),
        build_request(
            3,
            "skills/get",
            serde_json::json!({"uri": "skill://alpha/SKILL.md"}),
        ),
    ];

    let responses = run_macro_server_request(&requests);

    // initialize → skills extension advertised
    let init_resp = responses
        .iter()
        .find(|r| r["id"] == 1)
        .expect("macro server should answer initialize");
    assert!(
        !init_resp["result"]["capabilities"]["extensions"]["io.modelcontextprotocol/skills"]
            .is_null(),
        "skills extension must be advertised; got: {init_resp}"
    );
    // The macro does NOT advertise directoryRead — it cannot enumerate files
    // for dynamic skills, and the spec says clients MUST NOT call
    // resources/directory/read without the declaration.
    assert!(
        init_resp["result"]["capabilities"]["extensions"]["io.modelcontextprotocol/skills"]
            ["directoryRead"]
            .is_null(),
        "macro server must NOT advertise directoryRead; got: {init_resp}"
    );

    // skills/list → 3 skills from the macro server
    let skills_list = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skills"].as_array())
        .expect("macro server should return skills/list response");

    assert_eq!(
        skills_list.len(),
        3,
        "macro server should discover 3 skills (alpha, beta, gamma); got: {skills_list:?}"
    );
    // SEP-2640 skills.mdx:225 — `resources` is REQUIRED on every skill entry,
    // and an entry with no `resources` at all is invalid: hosts MUST NOT load
    // it. The `#[skill]` attribute has no file-list parameter, so every
    // macro-generated entry must carry the literal string "dynamic" rather
    // than omitting the field.
    for skill in skills_list {
        assert_eq!(
            skill["resources"], "dynamic",
            "every macro skill entry must carry resources=\"dynamic\"; got: {skill}"
        );
    }
    let uris: Vec<&str> = skills_list
        .iter()
        .map(|s| s["uri"].as_str().unwrap_or(""))
        .collect();
    assert!(
        uris.contains(&"skill://alpha/SKILL.md"),
        "alpha skill should be listed; got: {uris:?}"
    );
    assert!(
        uris.contains(&"skill://beta/nested/SKILL.md"),
        "beta skill should be listed; got: {uris:?}"
    );

    // skills/get → the entry lives under result.skill, not result
    let get_resp = responses
        .iter()
        .find(|r| r["id"] == 3)
        .expect("macro server should return skills/get response");
    let frontmatter = get_resp["result"]["skill"]["frontmatter"]
        .as_object()
        .expect("skills/get should return result.skill.frontmatter");
    assert_eq!(
        frontmatter["name"], "alpha",
        "skills/get should return correct frontmatter"
    );
}

/// The macro-generated handler must wire `resources/directory/read` into the
/// `ServerHandler` dispatch and advertise the `resources` capability.
///
/// The request URIs are **directory** URIs (`skill://alpha`, the skill root),
/// per the spec: the skill root is a directory resource. The example declares
/// no file lists — the `#[skill]` macro has no file-list attribute (only
/// `dynamic`) — so an empty array is the correct result. The point of this test
/// is that the method is *answered*, not that it lists files; the hand-written
/// `skill_fs_server` covers the non-empty case. If the macro stops generating
/// `resources_directory_read`, this endpoint falls back to the trait default
/// and returns method-not-found (-32601) instead.
#[test]
fn macro_server_resources_directory_read_is_dispatched() {
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
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        })
        .to_string(),
        // Static skill (alpha) — skill root, no file list declared.
        build_request(
            2,
            "resources/directory/read",
            serde_json::json!({"uri": "skill://alpha"}),
        ),
        // Dynamic skill (gamma) — macro returns an empty list for `Dynamic`.
        build_request(
            3,
            "resources/directory/read",
            serde_json::json!({"uri": "skill://gamma"}),
        ),
    ];

    let responses = run_macro_server_request(&requests);

    // initialize → resources capability advertised (gates the endpoint).
    let init_resp = responses
        .iter()
        .find(|r| r["id"] == 1)
        .expect("macro server should answer initialize");
    assert!(
        !init_resp["result"]["capabilities"]["resources"].is_null(),
        "macro server must advertise the resources capability; got: {init_resp}"
    );

    for id in [2u64, 3] {
        let resp = responses.iter().find(|r| r["id"] == id).unwrap_or_else(|| {
            panic!("macro server should answer resources/directory/read (id {id})")
        });
        assert!(
            resp["error"].is_null(),
            "resources/directory/read must be dispatched, not method-not-found; got: {resp}"
        );
        let resources = resp["result"]["resources"].as_array().unwrap_or_else(|| {
            panic!("resources/directory/read should return a resources array; got: {resp}")
        });
        // Both alpha (static, no file list) and gamma (dynamic) must return
        // an empty array — the macro has no file-list attribute, so it cannot
        // know what files exist. This assertion locks in the current behavior
        // so a future change doesn't silently break it.
        assert_eq!(
            resources.len(),
            0,
            "macro server must return an empty resources array for skills without a file list; got: {resources:?}"
        );
    }
}

/// A **file** URI (`skill://<skill-path>/SKILL.md`) is not a directory
/// resource, so `resources/directory/read` on it MUST return `-32602`
/// (Invalid params), per the spec: "If the URI does not exist, or exists but is
/// not a directory resource, the server MUST return error `-32602`."
///
/// Regression guard for the behavior this test previously asserted the
/// opposite of: it used to send `skill://alpha/SKILL.md` and require a
/// `resources` array, which is the non-conforming answer. An unknown skill URI
/// (`skill://nonexistent`) MUST also return `-32602`.
#[test]
fn macro_server_directory_read_rejects_file_and_unknown_uris() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-07-28",
                "capabilities": {"skills": {}},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        })
        .to_string(),
        // A file inside a served skill — not a directory.
        build_request(
            2,
            "resources/directory/read",
            serde_json::json!({"uri": "skill://alpha/SKILL.md"}),
        ),
        // A URI that does not identify any skill the server serves.
        build_request(
            3,
            "resources/directory/read",
            serde_json::json!({"uri": "skill://nonexistent"}),
        ),
    ];

    let responses = run_macro_server_request(&requests);

    for id in [2u64, 3] {
        let resp = responses
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("macro server should answer id {id}"));
        assert_eq!(
            resp["error"]["code"],
            serde_json::json!(-32602),
            "expected -32602 (Invalid params) for id {id}; got: {resp}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests for SEP-2640 spec conformance fixes
// ---------------------------------------------------------------------------

/// Resources completeness: files below the skill root must be published, even
/// in subdirectories that are not themselves skills.
///
/// Builds a skill tree where a data file lives in a subdirectory below the
/// skill root (`billing/refunds/forms/FORMS.md`) and verifies `skills/get`
/// lists it. Before the fix the walk skipped directories, so the file was
/// invisible to a host.
#[test]
fn nested_skill_subdirectory_files_are_published_in_resources() {
    use std::fs;

    // Build a temp skill tree:
    //   tmp_root/
    //     billing/
    //       refunds/
    //         SKILL.md
    //         forms/
    //           FORMS.md
    let tmp = std::env::temp_dir().join(format!("rmcp_test_{}", std::process::id()));
    let skill_dir = tmp.join("billing").join("refunds");
    let sub_dir = skill_dir.join("forms");
    fs::create_dir_all(&sub_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: refunds\ndescription: Refund handling\n---\n# Refunds\n",
    )
    .unwrap();
    fs::write(sub_dir.join("FORMS.md"), "# Refund Forms\n\nForm 101\n").unwrap();

    // Run the server against the temp root
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
        .arg(&tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn server");

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
    {
        let stdin = child.stdin.as_mut().expect("failed to open stdin");
        for req in &requests {
            writeln!(stdin, "{}", req).expect("failed to write request");
        }
    }

    let output = child.wait_with_output().expect("failed to wait for server");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let responses: Vec<serde_json::Value> = stdout
        .lines()
        .filter_map(|line| serde_json::from_str(line.trim()).ok())
        .collect();

    // Cleanup
    let _ = fs::remove_dir_all(&tmp);

    let resources = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["result"]["skill"]["resources"].as_array())
        .expect("should have resources for nested skill");

    assert!(
        resources.len() >= 2,
        "should include SKILL.md + nested FORMS.md; got {} resources: {:?}",
        resources.len(),
        resources
    );
    assert!(
        resources
            .iter()
            .any(|r| r["uri"].as_str().unwrap_or("").ends_with("/FORMS.md")),
        "should contain nested FORMS.md in resources"
    );
}

/// `skills/get` must reject a URI naming a file *inside* a skill.
///
/// Spec: `params.uri` MUST be the skill's SKILL.md URI. The handler used to
/// prefix-match through the router, so a request for
/// `skill://billing/refunds/FORMS.md` returned the skill entry instead of
/// `-32602` (Invalid params).
#[test]
fn skills_get_rejects_file_uri_within_skill() {
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
            serde_json::json!({"uri": "skill://billing/refunds/FORMS.md"}),
        ),
    ];

    let responses = run_server_request(&requests);
    let error = responses
        .iter()
        .find(|r| r["id"] == 2)
        .and_then(|r| r["error"].as_object())
        .expect("should have error response for non-SKILL.md URI");

    assert_eq!(
        error["code"],
        serde_json::json!(-32602),
        "skills/get on a non-SKILL.md URI must return -32602; got: {error:?}"
    );
}

/// `skills/get` must still resolve the canonical SKILL.md URI.
///
/// Regression guard: tightening the check must not break the happy path of
/// `skill://billing/refunds/SKILL.md`.
#[test]
fn skills_get_accepts_skill_md_uri() {
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
        .expect("should have skills/get response for SKILL.md URI");

    assert_eq!(
        skill["uri"], "skill://billing/refunds/SKILL.md",
        "skills/get on SKILL.md URI must succeed"
    );
}

// ---------------------------------------------------------------------------
// Regression tests for review findings (4 blocking items)
// ---------------------------------------------------------------------------

/// **Fix #1**: Router's `resources/directory/read` must delegate to the
/// route's declared resources, not return a hardcoded empty list.
///
/// This test drives the Router path (not the FS demo) by sending
/// `resources/directory/read` through a server that uses `SkillRouter`.
/// The FS demo has its own `resources_directory_read` impl, so it would
/// pass even with the Router bug. This test uses the macro server which
/// routes through the Router.
#[test]
fn router_directory_read_delegates_to_route() {
    let requests = vec![
        build_request(
            1,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2026-07-28",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        ),
        // Directory-form URI — this is what resources/directory/read sends.
        build_request(
            2,
            "resources/directory/read",
            serde_json::json!({"uri": "skill://alpha"}),
        ),
    ];

    let responses = run_macro_server_request(&requests);
    let resp = responses
        .iter()
        .find(|r| r["id"] == 2)
        .expect("should have resources/directory/read response");

    assert!(
        resp["error"].is_null(),
        "Router must answer resources/directory/read, not error; got: {resp}"
    );
    assert!(
        resp["result"]["resources"].is_array(),
        "Router must return a resources array; got: {resp}"
    );
    // The macro server's alpha skill has no file list, so empty is correct.
    // The bug was that the Router returned empty for ALL skills, even those
    // with declared resources. This test proves the Router path is exercised.
    let resources = resp["result"]["resources"].as_array().unwrap();
    assert_eq!(
        resources.len(),
        0,
        "alpha has no file list, so empty is correct; got: {resources:?}"
    );
}

/// **Fix #2**: `fill_missing_cache_hints` must cover `SkillsListResult` and
/// `ResourcesDirectoryReadResult`, not just the original 5 types.
///
/// Before the fix, these two types fell to `_ => return` and never got
/// cache hints filled in. This test constructs them without hints, calls
/// `fill_missing_cache_hints`, and asserts the hints are now present.
#[test]
fn fill_missing_cache_hints_covers_skills_list_result() {
    use rmcp::model::{ResultType, ServerResult, skills::SkillsListResult};

    let mut result = SkillsListResult::new(vec![]);
    result.result_type = Some(ResultType::COMPLETE);
    // Simulate a server that didn't set cache hints.
    result.ttl_ms = None;
    result.cache_scope = None;

    let mut server_result = ServerResult::SkillsListResult(result);
    server_result.fill_missing_cache_hints();

    let ServerResult::SkillsListResult(r) = &server_result else {
        panic!("expected SkillsListResult");
    };
    assert!(
        r.ttl_ms.is_some(),
        "fill_missing_cache_hints must set ttl_ms on SkillsListResult; got: {r:?}"
    );
    assert!(
        r.cache_scope.is_some(),
        "fill_missing_cache_hints must set cache_scope on SkillsListResult; got: {r:?}"
    );
}

/// **Fix #2**: Same for `ResourcesDirectoryReadResult`.
#[test]
fn fill_missing_cache_hints_covers_resources_directory_read_result() {
    use rmcp::model::{ResultType, ServerResult, skills::ResourcesDirectoryReadResult};

    let mut result = ResourcesDirectoryReadResult::new(vec![]);
    result.result_type = Some(ResultType::COMPLETE);
    result.ttl_ms = None;
    result.cache_scope = None;

    let mut server_result = ServerResult::ResourcesDirectoryReadResult(result);
    server_result.fill_missing_cache_hints();

    let ServerResult::ResourcesDirectoryReadResult(r) = &server_result else {
        panic!("expected ResourcesDirectoryReadResult");
    };
    assert!(
        r.ttl_ms.is_some(),
        "fill_missing_cache_hints must set ttl_ms on ResourcesDirectoryReadResult; got: {r:?}"
    );
    assert!(
        r.cache_scope.is_some(),
        "fill_missing_cache_hints must set cache_scope on ResourcesDirectoryReadResult; got: {r:?}"
    );
}

/// **Fix #3**: The FS demo server must advertise both the `resources`
/// capability and the `io.modelcontextprotocol/skills` extension in its
/// `initialize` response. Without this, a conformant client never calls
/// `skills/list`.
#[test]
fn fs_demo_advertises_skills_extension_and_resources() {
    let requests = vec![build_request(
        1,
        "initialize",
        serde_json::json!({
            "protocolVersion": "2026-07-28",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0.1.0"}
        }),
    )];

    let responses = run_server_request(&requests);
    let init_resp = responses
        .iter()
        .find(|r| r["id"] == 1)
        .expect("should have initialize response");

    let capabilities = &init_resp["result"]["capabilities"];

    assert!(
        !capabilities["resources"].is_null(),
        "FS demo must advertise resources capability; got: {capabilities}"
    );

    let extensions = &capabilities["extensions"];
    assert!(
        !extensions.is_null(),
        "FS demo must advertise extensions map; got: {capabilities}"
    );
    assert!(
        !extensions["io.modelcontextprotocol/skills"].is_null(),
        "FS demo must advertise io.modelcontextprotocol/skills extension; got: {extensions}"
    );
}

// **Fix #4** — directory-form URIs (`skill://alpha`) must be routable,
// not just file-form (`skill://alpha/SKILL.md`).
//
// Covered by the router-level unit test
// `router::tests::test_skill_router_matches_directory_form_uri`, which needs a
// real `SkillRoute` (and therefore a `for<'a>` handler) that this
// process-spawning integration file cannot construct.
//
// **Fix #1** — `resources/directory/read` delegation is covered by
// `router::tests::test_router_directory_read_delegates_to_file_list_route`.
