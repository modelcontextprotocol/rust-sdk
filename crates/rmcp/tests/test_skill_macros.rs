//! Compile and behavior tests for the `#[skill]`, `#[skill_router]`, and
//! `#[skill_handler]` proc-macros.
//!
//! These tests verify that the macros expand to valid code and that the
//! generated `SkillEntry` fields are correct.

use rmcp::model::skills::SkillResources;

// ─── Test skill with static frontmatter ──────────────────────────────────────

#[derive(Clone)]
struct TestServer;

#[rmcp::skill_router(server_handler = true)]
impl TestServer {
    #[rmcp::skill(uri = "skill://test-skill/SKILL.md", frontmatter = serde_json::json!({
        "name": "test-skill",
        "description": "A test skill"
    }))]
    async fn test_skill(&self) -> Result<(), rmcp::ErrorData> {
        Ok(())
    }
}

// ─── Test skill with dynamic resources ───────────────────────────────────────

#[derive(Clone)]
struct DynamicServer;

#[rmcp::skill_router(server_handler = true)]
impl DynamicServer {
    #[rmcp::skill(
        uri = "skill://dynamic-skill/SKILL.md",
        frontmatter = serde_json::json!({
            "name": "dynamic-skill",
            "description": "A dynamic skill"
        }),
        dynamic = true
    )]
    async fn dynamic_skill(&self) -> Result<(), rmcp::ErrorData> {
        Ok(())
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn skill_attr_generates_correct_entry() {
    let entry = TestServer::test_skill_skill_attr();
    assert_eq!(entry.uri, "skill://test-skill/SKILL.md");
    assert_eq!(entry.frontmatter["name"], "test-skill");
    assert_eq!(entry.frontmatter["description"], "A test skill");
    // Non-dynamic skill with no file list should have None resources
    assert!(entry.resources.is_none());
}

#[test]
fn dynamic_skill_has_dynamic_resources() {
    let entry = DynamicServer::dynamic_skill_skill_attr();
    assert_eq!(entry.uri, "skill://dynamic-skill/SKILL.md");
    assert_eq!(entry.frontmatter["name"], "dynamic-skill");
    assert!(matches!(entry.resources, Some(SkillResources::Dynamic)));
}

#[test]
fn skill_router_lists_all_skills() {
    let router = TestServer::skill_router();
    let skills = router.list_all();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].uri, "skill://test-skill/SKILL.md");
}

#[test]
fn skill_router_get_by_uri() {
    let router = TestServer::skill_router();
    let route = router.get_by_uri("skill://test-skill/SKILL.md");
    assert!(route.is_some());
    assert_eq!(route.unwrap().attr.uri, "skill://test-skill/SKILL.md");
}

#[test]
fn skill_router_get_by_unknown_uri_returns_none() {
    let router = TestServer::skill_router();
    let route = router.get_by_uri("skill://unknown/SKILL.md");
    assert!(route.is_none());
}
