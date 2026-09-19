//! Lightweight MCP client that validates the FileSystemSkillServer.
//!
//! Usage:
//!   cargo run --example skill_fs_client --features "client,transport-child-process" -- \
//!     --server-cmd "cargo run --example skill_fs_server --features server,transport-io -- --root ./fixtures/skills-dir"

use rmcp::{
    ServiceExt,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use tokio::process::Command as TokioCommand;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse arguments: --server-cmd <program> [-- <args>...]
    let mut args = std::env::args().skip(1);
    let mut server_program = String::new();
    let mut server_args: Vec<String> = Vec::new();
    let mut parsing_program = false;

    while let Some(arg) = args.next() {
        if arg == "--server-cmd" {
            server_program = args.next().ok_or("--server-cmd requires a value")?;
            parsing_program = true;
        } else if parsing_program {
            server_args.push(arg);
        }
    }

    if server_program.is_empty() {
        server_program = "cargo".to_string();
        server_args = vec![
            "run".to_string(),
            "--example".to_string(),
            "skill_fs_server".to_string(),
            "--features".to_string(),
            "server,transport-io".to_string(),
            "--".to_string(),
            "--root".to_string(),
            "./fixtures/skills-dir".to_string(),
        ];
    }

    // Spawn the server as a child process without shell
    let service = ()
        .serve(TokioChildProcess::new(
            TokioCommand::new(&server_program).configure(|cmd| {
                for arg in &server_args {
                    cmd.arg(arg);
                }
            }),
        )?)
        .await?;

    let server_info = service.peer_info();
    println!("Connected to server: {server_info:#?}");

    // 1. List all skills
    println!("\n=== skills/list ===");
    let skills = service.list_all_skills().await?;
    for skill in &skills {
        println!("  - {}", skill.uri);
        println!(
            "    frontmatter: {}",
            serde_json::to_string(&skill.frontmatter).unwrap_or_default()
        );
        if let Some(resources) = &skill.resources {
            println!("    resources: {:?}", resources);
        }
    }
    println!("Total skills: {}", skills.len());

    // 2. Get a specific skill
    println!("\n=== skills/get (billing/refunds/SKILL.md) ===");
    let single = service
        .skills_get("skill://billing/refunds/SKILL.md")
        .await?;
    println!("  Got: {}", single.skill.uri);
    println!(
        "    frontmatter: {}",
        serde_json::to_string(&single.skill.frontmatter).unwrap_or_default()
    );

    // 3. Try an unknown skill
    println!("\n=== skills/get (unknown) ===");
    let unknown = service.skills_get("skill://nonexistent/SKILL.md").await;
    match unknown {
        Err(e) => println!("  Expected error: {e}"),
        Ok(_) => println!("  Unexpected success!"),
    }

    println!("\n=== Validation complete ===");
    Ok(())
}
