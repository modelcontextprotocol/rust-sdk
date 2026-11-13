//! Opt-in, in-memory BM25 Okapi search over tool definitions.
//!
//! Matching is case-insensitive, splitting on punctuation (including underscores).
//! Scores use `k1 = 1.5` and `b = 0.75`; only positive matches are returned.
//! Equal scores retain catalog order. This is discovery, not access control.

use std::{
    collections::{BTreeSet, HashMap},
    sync::OnceLock,
};

use crate::model::Tool;

#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
pub use server::{ToolSearchConfig, ToolSearchServer};

/// A matching tool, including its full definition and BM25 score.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ToolMatch {
    /// The complete matching definition, including schemas and metadata.
    pub tool: Tool,
    /// Positive BM25 Okapi relevance score.
    pub score: f64,
}

/// A lazily built search index usable by servers, clients, or gateways.
#[derive(Debug)]
pub struct ToolSearchIndex {
    tools: Vec<Tool>,
    index_parameters: bool,
    index: OnceLock<Bm25>,
}

#[derive(Debug)]
struct Bm25 {
    documents: Vec<HashMap<String, usize>>,
    lengths: Vec<usize>,
    frequencies: HashMap<String, usize>,
    average_length: f64,
}

impl ToolSearchIndex {
    /// Index names, titles, descriptions, and parameter names/descriptions.
    pub fn new(tools: Vec<Tool>) -> Self {
        Self::with_parameter_indexing(tools, true)
    }

    /// Choose whether to include input-schema parameter names/descriptions.
    pub fn with_parameter_indexing(tools: Vec<Tool>, index_parameters: bool) -> Self {
        Self {
            tools,
            index_parameters,
            index: OnceLock::new(),
        }
    }

    /// Replace the catalog and invalidate the index, e.g. after `tools/list_changed`.
    pub fn replace_tools(&mut self, tools: Vec<Tool>) {
        self.tools = tools;
        self.index.take();
    }

    /// The full catalog backing this index.
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// Return up to `max_results` ranked tools. Empty/unmatched queries return no hits.
    pub fn search(&self, query: &str, max_results: usize) -> Vec<ToolMatch> {
        let terms: BTreeSet<_> = tokenize(query).collect();
        if terms.is_empty() || max_results == 0 || self.tools.is_empty() {
            return Vec::new();
        }
        let index = self.index.get_or_init(|| self.build());
        let count = self.tools.len() as f64;
        let mut scores: Vec<_> = index
            .documents
            .iter()
            .enumerate()
            .filter_map(|(i, document)| {
                let mut score = 0.0;
                for term in &terms {
                    if let Some(&frequency) = document.get(term) {
                        let df = index.frequencies[term] as f64;
                        let idf = (1.0 + (count - df + 0.5) / (df + 0.5)).ln();
                        let tf = frequency as f64;
                        let length = index.lengths[i] as f64 / index.average_length;
                        score += idf * tf * 2.5 / (tf + 1.5 * (0.25 + 0.75 * length));
                    }
                }
                (score > 0.0).then_some((i, score))
            })
            .collect();
        scores.sort_by(|(a, sa), (b, sb)| sb.total_cmp(sa).then_with(|| a.cmp(b)));
        scores
            .into_iter()
            .take(max_results)
            .map(|(i, score)| ToolMatch {
                tool: self.tools[i].clone(),
                score,
            })
            .collect()
    }

    fn build(&self) -> Bm25 {
        let mut documents = Vec::with_capacity(self.tools.len());
        let mut lengths = Vec::with_capacity(self.tools.len());
        let mut frequencies = HashMap::new();
        for tool in &self.tools {
            let mut text = tool.name.to_string();
            for field in [tool.title.as_deref(), tool.description.as_deref()]
                .into_iter()
                .flatten()
            {
                text.push(' ');
                text.push_str(field);
            }
            if self.index_parameters {
                // Iterative traversal handles nested schemas without recursive stack growth.
                let mut pending: Vec<&serde_json::Value> = tool.input_schema.values().collect();
                if let Some(properties) = tool
                    .input_schema
                    .get("properties")
                    .and_then(|v| v.as_object())
                {
                    for name in properties.keys() {
                        text.push(' ');
                        text.push_str(name);
                    }
                }
                while let Some(value) = pending.pop() {
                    match value {
                        serde_json::Value::Object(object) => {
                            if let Some(description) =
                                object.get("description").and_then(|v| v.as_str())
                            {
                                text.push(' ');
                                text.push_str(description);
                            }
                            if let Some(properties) =
                                object.get("properties").and_then(|v| v.as_object())
                            {
                                for name in properties.keys() {
                                    text.push(' ');
                                    text.push_str(name);
                                }
                            }
                            pending.extend(object.values());
                        }
                        serde_json::Value::Array(array) => pending.extend(array),
                        _ => {}
                    }
                }
            }
            let mut document = HashMap::new();
            let mut length = 0;
            for term in tokenize(&text) {
                *document.entry(term).or_insert(0) += 1;
                length += 1;
            }
            for term in document.keys() {
                *frequencies.entry(term.clone()).or_insert(0) += 1;
            }
            documents.push(document);
            lengths.push(length);
        }
        let average_length =
            (lengths.iter().sum::<usize>() as f64 / self.tools.len().max(1) as f64).max(1.0);
        Bm25 {
            documents,
            lengths,
            frequencies,
            average_length,
        }
    }
}

fn tokenize(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
}
