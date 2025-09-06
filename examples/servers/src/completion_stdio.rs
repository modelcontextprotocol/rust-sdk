//! MCP Server demonstrating code review completion functionality
//!
//! This example shows how to implement completion support for MCP prompts
//! with intelligent fuzzy matching for code review parameters.
//!
//! Run with MCP Inspector:
//! ```bash
//! npx @modelcontextprotocol/inspector cargo run -p mcp-server-examples --example servers_completion_stdio
//! ```

use anyhow::Result;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::prompt::PromptRouter, wrapper::Parameters},
    model::*,
    prompt, prompt_handler, prompt_router,
    schemars::JsonSchema,
    service::RequestContext,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use tracing_subscriber::{self, EnvFilter};

/// Arguments for the SQL query builder prompt
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "SQL query builder with progressive completion")]
pub struct SqlQueryArgs {
    #[schemars(description = "SQL operation type (SELECT, INSERT, UPDATE, DELETE)")]
    pub operation: String,
    #[schemars(description = "Database table name")]
    pub table: String,
    #[schemars(description = "Columns to select/update (only for SELECT/UPDATE)")]
    pub columns: Option<String>,
    #[schemars(description = "WHERE clause condition (optional for all operations)")]
    pub where_clause: Option<String>,
    #[schemars(description = "Values to insert (only for INSERT)")]
    pub values: Option<String>,
}

/// SQL query builder server with progressive completion
#[derive(Clone)]
pub struct SqlQueryServer {
    prompt_router: PromptRouter<SqlQueryServer>,
}

impl SqlQueryServer {
    pub fn new() -> Self {
        Self {
            prompt_router: Self::prompt_router(),
        }
    }
}

impl Default for SqlQueryServer {
    fn default() -> Self {
        Self::new()
    }
}

impl SqlQueryServer {
    /// Fuzzy matching with scoring for completion suggestions
    fn fuzzy_match(&self, query: &str, candidates: &[&str]) -> Vec<String> {
        if query.is_empty() {
            return candidates.iter().take(10).map(|s| s.to_string()).collect();
        }

        let query_lower = query.to_lowercase();
        let mut scored_matches = Vec::new();

        for candidate in candidates {
            let candidate_lower = candidate.to_lowercase();

            let score = if candidate_lower == query_lower {
                1000 // Exact match
            } else if candidate_lower.starts_with(&query_lower) {
                900 // Prefix match  
            } else if candidate_lower.contains(&query_lower) {
                800 // Contains substring
            } else if self.is_acronym_match(&query_lower, candidate) {
                700 // Acronym match (e.g., "js" → "JavaScript")
            } else if self.is_subsequence_match(&query_lower, &candidate_lower) {
                680 // Subsequence match (e.g., "rs" → "Rust")
            } else if self.is_single_letter_match(&query_lower, candidate) {
                650 // Single letter match (e.g., "j" → "Java")
            } else {
                continue; // No match
            };

            scored_matches.push((candidate.to_string(), score));
        }

        // Sort by score (desc) then alphabetically
        scored_matches.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        scored_matches
            .into_iter()
            .take(10)
            .map(|(name, _)| name)
            .collect()
    }

    /// Check if query matches as acronym (first letters of words or camelCase)
    fn is_acronym_match(&self, query: &str, candidate: &str) -> bool {
        let query_chars: Vec<char> = query.chars().collect();

        // Extract first letters from words (split by whitespace) or uppercase letters (camelCase)
        let mut first_chars: Vec<char>;

        // Split by whitespace first
        let words: Vec<&str> = candidate.split_whitespace().collect();
        if words.len() > 1 {
            // Multi-word case (e.g., "Memory Safety" -> "MS")
            first_chars = words
                .into_iter()
                .filter_map(|word| word.chars().next())
                .map(|c| c.to_lowercase().next().unwrap_or('\0'))
                .collect();
        } else {
            // Single word case - extract uppercase letters for camelCase (e.g., "JavaScript" -> "JS")
            first_chars = candidate
                .chars()
                .filter(|c| c.is_uppercase())
                .map(|c| c.to_lowercase().next().unwrap_or('\0'))
                .collect();

            // If no uppercase letters found, just use first letter
            if first_chars.is_empty() && !candidate.is_empty() {
                if let Some(first) = candidate.chars().next() {
                    first_chars.push(first.to_lowercase().next().unwrap_or('\0'));
                }
            }
        }

        // Special case: if query is 2 chars and we only got 1 char, try matching first 2 letters
        if query_chars.len() == 2 && first_chars.len() == 1 {
            if let Some(first) = candidate.chars().nth(0) {
                if let Some(second) = candidate.chars().nth(1) {
                    first_chars = vec![
                        first.to_lowercase().next().unwrap_or('\0'),
                        second.to_lowercase().next().unwrap_or('\0'),
                    ];
                }
            }
        }

        if query_chars.len() != first_chars.len() {
            return false;
        }

        query_chars
            .iter()
            .zip(first_chars.iter())
            .all(|(q, c)| q.to_lowercase().next().unwrap_or('\0') == *c)
    }

    /// Check if query is a subsequence of candidate (e.g., "rs" in "rust")
    fn is_subsequence_match(&self, query: &str, candidate_lower: &str) -> bool {
        let query_chars: Vec<char> = query.chars().collect();
        let candidate_chars: Vec<char> = candidate_lower.chars().collect();

        let mut query_idx = 0;

        for &candidate_char in &candidate_chars {
            if query_idx < query_chars.len() && query_chars[query_idx] == candidate_char {
                query_idx += 1;
            }
        }

        query_idx == query_chars.len()
    }

    /// Check if query matches first letter of single word
    fn is_single_letter_match(&self, query: &str, candidate: &str) -> bool {
        if query.len() != 1 {
            return false;
        }

        let query_char = query
            .chars()
            .next()
            .unwrap()
            .to_lowercase()
            .next()
            .unwrap_or('\0');
        let first_char = candidate
            .chars()
            .next()
            .unwrap_or('\0')
            .to_lowercase()
            .next()
            .unwrap_or('\0');

        query_char == first_char
    }
}

#[prompt_router]
impl SqlQueryServer {
    #[prompt(name = "sql_query", description = "Smart SQL query builder")]
    async fn sql_query(
        &self,
        Parameters(args): Parameters<SqlQueryArgs>,
    ) -> Result<GetPromptResult, McpError> {
        let messages = if args.operation.is_empty() {
            vec![
                PromptMessage::new_text(
                    PromptMessageRole::User,
                    "I need help building a SQL query. Where should I start?",
                ),
                PromptMessage::new_text(
                    PromptMessageRole::Assistant,
                    "I'll help you build a SQL query step by step. First, what type of operation do you want to perform? \
                     Choose from: SELECT (to read data), INSERT (to add data), UPDATE (to modify data), or DELETE (to remove data).",
                ),
            ]
        } else if args.table.is_empty() {
            vec![
                PromptMessage::new_text(
                    PromptMessageRole::User,
                    format!("I want to {} data. What's next?", args.operation),
                ),
                PromptMessage::new_text(
                    PromptMessageRole::Assistant,
                    format!(
                        "Great! For a {} operation, I need to know which table you want to work with. \
                            What's the name of your database table?",
                        args.operation
                    ),
                ),
            ]
        } else {
            // Build the SQL query based on filled arguments
            let query = match args.operation.to_uppercase().as_str() {
                "SELECT" => {
                    let cols = args
                        .columns
                        .as_ref()
                        .filter(|c| !c.is_empty())
                        .map(|c| c.as_str())
                        .unwrap_or("*");
                    let where_part = args
                        .where_clause
                        .as_ref()
                        .map(|w| format!(" WHERE {}", w))
                        .unwrap_or_default();
                    format!("SELECT {} FROM {}{}", cols, args.table, where_part)
                }
                "INSERT" => match &args.values {
                    Some(vals) if !vals.is_empty() => {
                        format!("INSERT INTO {} VALUES ({})", args.table, vals)
                    }
                    _ => format!("INSERT INTO {} (...) VALUES (...)", args.table),
                },
                "UPDATE" => {
                    let set_part = args
                        .columns
                        .as_ref()
                        .filter(|c| !c.is_empty())
                        .map(|c| c.as_str())
                        .unwrap_or("...");
                    let where_part = args
                        .where_clause
                        .as_ref()
                        .map(|w| format!(" WHERE {}", w))
                        .unwrap_or_default();
                    format!("UPDATE {} SET {}{}", args.table, set_part, where_part)
                }
                "DELETE" => {
                    let where_part = args
                        .where_clause
                        .as_ref()
                        .map(|w| format!(" WHERE {}", w))
                        .unwrap_or_default();
                    format!("DELETE FROM {}{}", args.table, where_part)
                }
                _ => format!("{} FROM {}", args.operation, args.table),
            };

            vec![
                PromptMessage::new_text(
                    PromptMessageRole::User,
                    "Generate the SQL query based on my parameters and explain what it does.",
                ),
                PromptMessage::new_text(
                    PromptMessageRole::Assistant,
                    format!(
                        "Here's your SQL query:\n\n```sql\n{}\n```\n\nThis query will {} the {} table.",
                        query,
                        args.operation.to_lowercase(),
                        args.table
                    ),
                ),
            ]
        };

        Ok(GetPromptResult {
            description: Some(format!(
                "SQL Query: {} on {}",
                if args.operation.is_empty() {
                    "Unknown"
                } else {
                    &args.operation
                },
                if args.table.is_empty() {
                    "table"
                } else {
                    &args.table
                }
            )),
            messages,
        })
    }
}

#[prompt_handler]
impl ServerHandler for SqlQueryServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_completions()
                .enable_prompts()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Smart SQL query builder with progressive completion that adapts based on your choices:\n\n\
                 Step 1: Choose operation type ('sel' → SELECT, 'ins' → INSERT, 'upd' → UPDATE, 'del' → DELETE)\n\
                 Step 2: Specify table name ('users', 'orders', 'products')\n\
                 Step 3: Add relevant fields based on operation type:\n\
                 • SELECT/UPDATE: columns ('name', 'email', 'id')\n\
                 • INSERT: values to insert\n\
                 • All: optional WHERE clause\n\n\
                 The completion adapts - only relevant fields appear based on your SQL operation!"
                    .to_string(),
            ),
            ..Default::default()
        }
    }

    async fn complete(
        &self,
        request: CompleteRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<CompleteResult, McpError> {
        let candidates = match &request.r#ref {
            Reference::Prompt(prompt_ref) if prompt_ref.name == "sql_query" => {
                let filled_fields: Vec<&str> = request
                    .context
                    .as_ref()
                    .map(|ctx| ctx.argument_names().collect())
                    .unwrap_or_default();

                tracing::debug!(
                    "SQL completion - filled fields: {:?}, completing: {}",
                    filled_fields,
                    request.argument.name
                );

                match request.argument.name.as_str() {
                    "operation" => vec!["SELECT", "INSERT", "UPDATE", "DELETE"],
                    "table" => vec!["users", "orders", "products", "categories", "reviews"],
                    "columns" => {
                        // Only show columns completion if operation is SELECT or UPDATE
                        if let Some(context) = &request.context {
                            if let Some(operation) = context.get_argument("operation") {
                                match operation.to_uppercase().as_str() {
                                    "SELECT" | "UPDATE" => {
                                        vec!["id", "name", "email", "created_at", "updated_at", "*"]
                                    }
                                    _ => vec!["Not applicable for this operation"],
                                }
                            } else {
                                vec!["Choose operation first"]
                            }
                        } else {
                            vec!["Choose operation first"]
                        }
                    }
                    "values" => {
                        // Only show values completion for INSERT
                        if let Some(context) = &request.context {
                            if let Some(operation) = context.get_argument("operation") {
                                match operation.to_uppercase().as_str() {
                                    "INSERT" => {
                                        vec!["'John Doe'", "'jane@example.com'", "123", "NOW()"]
                                    }
                                    _ => vec!["Not applicable for this operation"],
                                }
                            } else {
                                vec!["Choose operation first"]
                            }
                        } else {
                            vec!["Choose operation first"]
                        }
                    }
                    "where_clause" => {
                        // WHERE clause suggestions based on filled fields count
                        match filled_fields.len() {
                            0..=1 => vec!["Complete operation and table first"],
                            _ => vec![
                                "id = 1",
                                "name = 'example'",
                                "created_at > '2023-01-01'",
                                "status = 'active'",
                            ],
                        }
                    }
                    _ => vec![],
                }
            }
            _ => vec![],
        };

        let suggestions = self.fuzzy_match(&request.argument.value, &candidates);

        let completion = CompletionInfo {
            values: suggestions,
            total: None,
            has_more: Some(false),
        };

        Ok(CompleteResult { completion })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    println!("MCP SQL Query Builder with Smart Completion");
    println!("==========================================");
    println!();
    println!("This server demonstrates argument_names() value with progressive completion:");
    println!("1. Start with operation type (SELECT, INSERT, UPDATE, DELETE)");
    println!("2. Choose table name (users, orders, products)");
    println!("3. Only relevant fields appear based on your operation!");
    println!("   • SELECT/UPDATE: shows columns field");
    println!("   • INSERT: shows values field");
    println!("   • All operations: optional WHERE clause after step 2");
    println!();
    println!("To test with MCP Inspector:");
    println!(
        "npx @modelcontextprotocol/inspector cargo run -p mcp-server-examples --example servers_completion_stdio"
    );
    println!();

    let server = SqlQueryServer::new();
    let service = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("Server error: {:?}", e);
    })?;

    service.waiting().await?;
    Ok(())
}
