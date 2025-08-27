use std::{future::Future, pin::Pin};

use crate::{error::ErrorData as McpError, model::*, service::RequestContext};

/// Trait for providing custom completion logic
///
/// Implement this trait to provide context-aware completion suggestions
/// for prompts and resource templates in your MCP server.
pub trait CompletionProvider {
    /// Provide completion suggestions for a prompt argument
    ///
    /// # Arguments
    /// * `prompt_name` - Name of the prompt being completed
    /// * `argument_name` - Name of the argument being completed
    /// * `current_value` - Current partial value of the argument
    /// * `context` - Previously resolved arguments that can inform completion
    ///
    /// # Returns
    /// CompletionInfo with suggestions, pagination info, and total count
    fn complete_prompt_argument<'a>(
        &'a self,
        prompt_name: &'a str,
        argument_name: &'a str,
        current_value: &'a str,
        context: Option<&'a CompletionContext>,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionInfo, McpError>> + Send + 'a>>;

    /// Provide completion suggestions for a resource template URI
    ///
    /// # Arguments  
    /// * `uri_template` - URI template pattern being completed
    /// * `argument_name` - Name of the URI parameter being completed
    /// * `current_value` - Current partial value of the parameter
    /// * `context` - Previously resolved arguments that can inform completion
    ///
    /// # Returns
    /// CompletionInfo with suggestions, pagination info, and total count
    fn complete_resource_argument<'a>(
        &'a self,
        uri_template: &'a str,
        argument_name: &'a str,
        current_value: &'a str,
        context: Option<&'a CompletionContext>,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionInfo, McpError>> + Send + 'a>>;
}

/// Default completion provider with optimized fuzzy matching
#[derive(Debug, Clone, Default)]
pub struct DefaultCompletionProvider {
    /// Maximum number of suggestions to return
    pub max_suggestions: usize,
}

impl DefaultCompletionProvider {
    /// Create a new default completion provider
    pub fn new() -> Self {
        Self {
            max_suggestions: CompletionInfo::MAX_VALUES,
        }
    }

    /// Create with custom max suggestions limit
    pub fn with_max_suggestions(max_suggestions: usize) -> Self {
        Self {
            max_suggestions: max_suggestions.min(CompletionInfo::MAX_VALUES),
        }
    }

    /// Perform optimized fuzzy string matching
    pub fn fuzzy_match(&self, query: &str, candidates: &[String]) -> Vec<String> {
        if query.is_empty() {
            return candidates
                .iter()
                .take(self.max_suggestions)
                .cloned()
                .collect();
        }

        // Pre-allocate with capacity to avoid reallocations
        let mut scored_indices: Vec<(usize, usize)> =
            Vec::with_capacity(candidates.len().min(self.max_suggestions * 2));

        for (idx, candidate) in candidates.iter().enumerate() {
            if let Some(score) = self.calculate_match_score(query, candidate) {
                scored_indices.push((idx, score));
            }
        }

        // Use partial sort for top-k selection instead of full sort
        if scored_indices.len() > self.max_suggestions {
            scored_indices.select_nth_unstable_by(self.max_suggestions, |a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| candidates[a.0].cmp(&candidates[b.0]))
            });
            scored_indices.truncate(self.max_suggestions);
        }

        // Sort the selected top elements by score and name
        scored_indices.sort_unstable_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| candidates[a.0].cmp(&candidates[b.0]))
        });

        // Return cloned strings only for the final result set
        scored_indices
            .into_iter()
            .map(|(idx, _)| candidates[idx].clone())
            .collect()
    }

    /// Calculate match score without string allocations
    fn calculate_match_score(&self, query: &str, candidate: &str) -> Option<usize> {
        // Case-insensitive matching using char comparison to avoid allocations
        let query_chars: Vec<char> = query.chars().map(|c| c.to_ascii_lowercase()).collect();
        let candidate_chars: Vec<char> =
            candidate.chars().map(|c| c.to_ascii_lowercase()).collect();

        // Check if query matches candidate
        if !self.contains_subsequence(&candidate_chars, &query_chars) {
            return None;
        }

        // Calculate score based on match quality
        let score = if candidate_chars.len() == query_chars.len() && candidate_chars == query_chars
        {
            // Exact match gets highest score
            1000
        } else if candidate_chars.len() >= query_chars.len()
            && candidate_chars[..query_chars.len()] == query_chars
        {
            // Prefix match gets high score, penalized by query length
            500 - query_chars.len()
        } else {
            // Substring match gets lower score, bonus for early position
            if let Some(pos) = self.find_subsequence_position(&candidate_chars, &query_chars) {
                100 - pos.min(100)
            } else {
                // Fuzzy match (characters present but not contiguous)
                10
            }
        };

        Some(score)
    }

    /// Check if candidate contains query as subsequence (case-insensitive)
    fn contains_subsequence(&self, candidate_chars: &[char], query_chars: &[char]) -> bool {
        if query_chars.is_empty() {
            return true;
        }
        if candidate_chars.len() < query_chars.len() {
            return false;
        }

        let mut query_idx = 0;
        for &candidate_char in candidate_chars {
            if query_idx < query_chars.len() && candidate_char == query_chars[query_idx] {
                query_idx += 1;
                if query_idx == query_chars.len() {
                    return true;
                }
            }
        }
        false
    }

    /// Find position of contiguous subsequence match
    fn find_subsequence_position(
        &self,
        candidate_chars: &[char],
        query_chars: &[char],
    ) -> Option<usize> {
        if query_chars.is_empty() {
            return Some(0);
        }
        if candidate_chars.len() < query_chars.len() {
            return None;
        }

        (0..=(candidate_chars.len() - query_chars.len()))
            .find(|&i| candidate_chars[i..i + query_chars.len()] == *query_chars)
    }
}

impl CompletionProvider for DefaultCompletionProvider {
    fn complete_prompt_argument<'a>(
        &'a self,
        _prompt_name: &'a str,
        _argument_name: &'a str,
        current_value: &'a str,
        _context: Option<&'a CompletionContext>,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionInfo, McpError>> + Send + 'a>> {
        Box::pin(async move {
            // Default implementation provides basic completion examples
            let candidates = vec![
                "example_value".to_string(),
                "sample_input".to_string(),
                "test_data".to_string(),
                "placeholder".to_string(),
            ];

            let matches = self.fuzzy_match(current_value, &candidates);

            CompletionInfo::with_all_values(matches).map_err(|e| McpError::internal_error(e, None))
        })
    }

    fn complete_resource_argument<'a>(
        &'a self,
        _uri_template: &'a str,
        _argument_name: &'a str,
        current_value: &'a str,
        _context: Option<&'a CompletionContext>,
    ) -> Pin<Box<dyn Future<Output = Result<CompletionInfo, McpError>> + Send + 'a>> {
        Box::pin(async move {
            // Default implementation provides basic URI completion examples
            let candidates = vec![
                "file://path/to/resource".to_string(),
                "http://example.com/api".to_string(),
                "memory://cache/key".to_string(),
                "db://table/record".to_string(),
            ];

            let matches = self.fuzzy_match(current_value, &candidates);

            CompletionInfo::with_all_values(matches).map_err(|e| McpError::internal_error(e, None))
        })
    }
}

/// Completion handler that delegates to a CompletionProvider
pub async fn handle_completion<P: CompletionProvider>(
    provider: &P,
    request: &CompleteRequestParam,
    _context: &RequestContext<crate::service::RoleServer>,
) -> Result<CompleteResult, McpError> {
    // Validate request parameters
    if request.argument.name.is_empty() {
        return Err(McpError::invalid_params(
            "Argument name cannot be empty",
            None,
        ));
    }

    // Route to appropriate completion handler based on reference type
    let completion = match &request.r#ref {
        Reference::Prompt(prompt_ref) => {
            provider
                .complete_prompt_argument(
                    &prompt_ref.name,
                    &request.argument.name,
                    &request.argument.value,
                    request.context.as_ref(),
                )
                .await?
        }
        Reference::Resource(resource_ref) => {
            provider
                .complete_resource_argument(
                    &resource_ref.uri,
                    &request.argument.name,
                    &request.argument.value,
                    request.context.as_ref(),
                )
                .await?
        }
    };

    // Validate completion response
    completion
        .validate()
        .map_err(|e| McpError::internal_error(e, None))?;

    Ok(CompleteResult { completion })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[tokio::test]
    async fn test_default_completion_provider() {
        let provider = DefaultCompletionProvider::new();

        let result = provider
            .complete_prompt_argument("test_prompt", "arg", "ex", None)
            .await
            .unwrap();

        assert!(!result.values.is_empty());
        assert!(result.values.iter().any(|v| v.contains("example")));
    }

    #[tokio::test]
    async fn test_completion_with_context() {
        let provider = DefaultCompletionProvider::new();

        let mut args = HashMap::new();
        args.insert("prev_arg".to_string(), "some_value".to_string());
        let context = CompletionContext::with_arguments(args);

        let result = provider
            .complete_prompt_argument("test_prompt", "arg", "test", Some(&context))
            .await
            .unwrap();

        assert!(!result.values.is_empty());
    }

    #[tokio::test]
    async fn test_fuzzy_matching() {
        let provider = DefaultCompletionProvider::new();
        let candidates = vec![
            "hello_world".to_string(),
            "hello_rust".to_string(),
            "world_peace".to_string(),
            "rust_lang".to_string(),
        ];

        let matches = provider.fuzzy_match("hello", &candidates);
        assert_eq!(matches.len(), 2);
        assert!(matches.contains(&"hello_world".to_string()));
        assert!(matches.contains(&"hello_rust".to_string()));
    }
}
