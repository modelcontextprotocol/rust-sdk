use rmcp::{
    handler::server::{ServerHandler, prompt::Arguments, router::Router},
    model::{GetPromptResult, PromptMessage, PromptMessageRole},
    service::{RequestContext, RoleServer},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Test prompt arguments for code review
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct CodeReviewArgs {
    /// The file path to review
    file_path: String,
    /// Focus areas for the review
    #[serde(skip_serializing_if = "Option::is_none")]
    focus_areas: Option<Vec<String>>,
}

/// Test prompt arguments for debugging
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
struct DebugAssistantArgs {
    /// The error message to debug
    error_message: String,
    /// The programming language
    language: String,
    /// Additional context
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

struct TestPromptServer;

impl ServerHandler for TestPromptServer {}

/// A simple code review prompt
#[rmcp::prompt(
    name = "code_review",
    description = "Reviews code for best practices and potential issues"
)]
async fn code_review_prompt(
    _server: &TestPromptServer,
    Arguments(args): Arguments<CodeReviewArgs>,
    _ctx: RequestContext<RoleServer>,
) -> Result<Vec<PromptMessage>, rmcp::Error> {
    let mut messages = vec![PromptMessage::new_text(
        PromptMessageRole::User,
        format!("Please review the code in file: {}", args.file_path),
    )];

    if let Some(focus_areas) = args.focus_areas {
        messages.push(PromptMessage::new_text(
            PromptMessageRole::User,
            format!("Focus on these areas: {}", focus_areas.join(", ")),
        ));
    }

    messages.push(PromptMessage::new_text(
        PromptMessageRole::Assistant,
        "I'll help you review this code. Let me analyze it for best practices, potential bugs, and improvement opportunities.",
    ));

    Ok(messages)
}

/// A debugging assistant prompt
#[rmcp::prompt(name = "debug_assistant")]
async fn debug_assistant_prompt(
    _server: &TestPromptServer,
    Arguments(args): Arguments<DebugAssistantArgs>,
    _ctx: RequestContext<RoleServer>,
) -> Result<GetPromptResult, rmcp::Error> {
    let mut messages = vec![PromptMessage::new_text(
        PromptMessageRole::User,
        format!(
            "I'm getting this error in my {} code: {}",
            args.language, args.error_message
        ),
    )];

    if let Some(context) = args.context {
        messages.push(PromptMessage::new_text(
            PromptMessageRole::User,
            format!("Additional context: {}", context),
        ));
    }

    messages.push(PromptMessage::new_text(
        PromptMessageRole::Assistant,
        format!(
            "I'll help you debug this {} error. Let me analyze the error message and provide solutions.",
            args.language
        ),
    ));

    Ok(GetPromptResult {
        description: Some("Helps debug programming errors with detailed analysis".to_string()),
        messages,
    })
}

/// A simple greeting prompt without arguments
#[rmcp::prompt]
async fn greeting_prompt(
    _server: &TestPromptServer,
    _ctx: RequestContext<RoleServer>,
) -> Result<Vec<PromptMessage>, rmcp::Error> {
    Ok(vec![
        PromptMessage::new_text(
            PromptMessageRole::User,
            "Hello! I'd like to start a conversation.",
        ),
        PromptMessage::new_text(
            PromptMessageRole::Assistant,
            "Hello! I'm here to help. What would you like to discuss today?",
        ),
    ])
}

#[tokio::test]
async fn test_prompt_macro_basic() {
    // Test that the prompt attribute functions are generated
    let greeting = greeting_prompt_prompt_attr();
    assert_eq!(greeting.name, "greeting_prompt");
    assert_eq!(
        greeting.description.as_deref(),
        Some("A simple greeting prompt without arguments")
    );
    assert!(greeting.arguments.is_none());

    let code_review = code_review_prompt_prompt_attr();
    assert_eq!(code_review.name, "code_review");
    assert_eq!(
        code_review.description.as_deref(),
        Some("Reviews code for best practices and potential issues")
    );
    assert!(code_review.arguments.is_some());

    let debug_assistant = debug_assistant_prompt_prompt_attr();
    assert_eq!(debug_assistant.name, "debug_assistant");
    assert!(debug_assistant.arguments.is_some());
}

#[tokio::test]
async fn test_prompt_router() {
    // Create prompt routes manually
    let greeting_route = rmcp::handler::server::router::prompt::PromptRoute::new(
        greeting_prompt_prompt_attr(),
        greeting_prompt,
    );
    let code_review_route = rmcp::handler::server::router::prompt::PromptRoute::new(
        code_review_prompt_prompt_attr(),
        code_review_prompt,
    );
    let debug_assistant_route = rmcp::handler::server::router::prompt::PromptRoute::new(
        debug_assistant_prompt_prompt_attr(),
        debug_assistant_prompt,
    );

    let server = Router::new(TestPromptServer)
        .with_prompt(greeting_route)
        .with_prompt(code_review_route)
        .with_prompt(debug_assistant_route);

    // Test list prompts
    let prompts = server.prompt_router.list_all();
    assert_eq!(prompts.len(), 3);

    let prompt_names: Vec<_> = prompts.iter().map(|p| p.name.as_str()).collect();
    assert!(prompt_names.contains(&"greeting_prompt"));
    assert!(prompt_names.contains(&"code_review"));
    assert!(prompt_names.contains(&"debug_assistant"));
}

#[tokio::test]
async fn test_prompt_arguments_schema() {
    let code_review = code_review_prompt_prompt_attr();
    let args = code_review.arguments.unwrap();

    // Should have two arguments: file_path (required) and focus_areas (optional)
    assert_eq!(args.len(), 2);

    let file_path_arg = args.iter().find(|a| a.name == "file_path").unwrap();
    assert_eq!(file_path_arg.required, Some(true));
    assert_eq!(
        file_path_arg.description.as_deref(),
        Some("The file path to review")
    );

    let focus_areas_arg = args.iter().find(|a| a.name == "focus_areas").unwrap();
    assert_eq!(focus_areas_arg.required, Some(false));
    assert_eq!(
        focus_areas_arg.description.as_deref(),
        Some("Focus areas for the review")
    );
}

#[tokio::test]
async fn test_prompt_route_creation() {
    // Test that prompt routes can be created
    let route = rmcp::handler::server::router::prompt::PromptRoute::new(
        code_review_prompt_prompt_attr(),
        code_review_prompt,
    );

    assert_eq!(route.name(), "code_review");
}

// Additional integration tests would require a full server setup
// These tests demonstrate the basic functionality of the prompt system

