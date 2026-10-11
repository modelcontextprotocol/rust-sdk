//! Example server using `#[skill]` + `#[skill_router]` macros.
//!
//! This server demonstrates the macro-generated `skills_list`, `skills_get`,
//! and `resources_directory_read` methods. It uses a simple in-memory skill
//! registry (no filesystem) so it can be tested without fixture files.

use rmcp::ServiceExt;

#[derive(Clone)]
struct MacroServer;

#[rmcp::skill_router(server_handler = true)]
impl MacroServer {
    #[rmcp::skill(uri = "skill://alpha/SKILL.md", frontmatter = serde_json::json!({
        "name": "alpha",
        "description": "Alpha skill"
    }))]
    async fn alpha(&self) -> Result<(), rmcp::ErrorData> {
        Ok(())
    }

    #[rmcp::skill(uri = "skill://beta/nested/SKILL.md", frontmatter = serde_json::json!({
        "name": "nested",
        "description": "Beta skill with nested path"
    }))]
    async fn beta(&self) -> Result<(), rmcp::ErrorData> {
        Ok(())
    }

    #[rmcp::skill(
        uri = "skill://gamma/SKILL.md",
        frontmatter = serde_json::json!({
            "name": "gamma",
            "description": "Gamma skill with dynamic resources"
        }),
        dynamic = true
    )]
    async fn gamma(&self) -> Result<(), rmcp::ErrorData> {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = MacroServer;
    let transport = rmcp::transport::stdio();
    // Hold the `RunningService` for the life of the process. Dropping it
    // immediately (as `server.serve(transport).await?;` alone would) cancels
    // the serve loop via its DropGuard and the server exits before answering.
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}
