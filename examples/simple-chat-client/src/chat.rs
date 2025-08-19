use std::{
    io::{self, Write},
    sync::Arc,
};

use anyhow::Result;
use serde_json;

use crate::{
    client::ChatClient,
    model::{CompletionRequest, Message, ToolFunction},
    tool::{Tool as ToolTrait, ToolSet},
};

pub struct ChatSession {
    client: Arc<dyn ChatClient>,
    tool_set: ToolSet,
    model: String,
    messages: Vec<Message>,
}

impl ChatSession {
    pub fn new(client: Arc<dyn ChatClient>, tool_set: ToolSet, model: String) -> Self {
        Self {
            client,
            tool_set,
            model,
            messages: Vec::new(),
        }
    }

    pub fn add_system_prompt(&mut self, prompt: impl ToString) {
        self.messages.push(Message::system(prompt));
    }

    pub fn get_tools(&self) -> Vec<Arc<dyn ToolTrait>> {
        self.tool_set.tools()
    }

    pub async fn analyze_tool_call(&mut self, response: &Message) {
        let mut tool_calls_func = Vec::new();
        if let Some(tool_calls) = response.tool_calls.as_ref() {
            for tool_call in tool_calls {
                if tool_call._type == "function" {
                    tool_calls_func.push(tool_call.function.clone());
                }
            }
        } else {
            // check if message contains tool call
            if response.content.contains("Tool:") {
                let lines: Vec<&str> = response.content.split('\n').collect();
                // simple parse tool call
                let mut tool_name = None;
                let mut args_text = Vec::new();
                let mut parsing_args = false;

                for line in lines {
                    if line.starts_with("Tool:") {
                        tool_name = line.strip_prefix("Tool:").map(|s| s.trim().to_string());
                        parsing_args = false;
                    } else if line.starts_with("Inputs:") {
                        parsing_args = true;
                    } else if parsing_args {
                        args_text.push(line.trim());
                    }
                }
                if let Some(name) = tool_name {
                    tool_calls_func.push(ToolFunction {
                        name,
                        arguments: args_text.join("\n"),
                    });
                }
            }
        }
        // call tool
        for tool_call in tool_calls_func {
            println!("tool call: {:?}", tool_call);
            let tool = self.tool_set.get_tool(&tool_call.name);
            if let Some(tool) = tool {
                // call tool
                let args = serde_json::from_str::<serde_json::Value>(&tool_call.arguments)
                    .unwrap_or_default();
                match tool.call(args).await {
                    Ok(result) => {
                        if result.is_error.is_some_and(|b| b) {
                            self.messages
                                .push(Message::user("tool call failed, mcp call error"));
                        } else {
                            result.content.iter().for_each(|content| {
                                if let Some(content_text) = content.as_text() {
                                    let json_result = serde_json::from_str::<serde_json::Value>(
                                        &content_text.text,
                                    )
                                    .unwrap_or_default();
                                    let pretty_result =
                                        serde_json::to_string_pretty(&json_result).unwrap();
                                    println!("call tool result: {}", pretty_result);
                                    self.messages.push(Message::user(format!(
                                        "call tool result: {}",
                                        pretty_result
                                    )));
                                }
                            });
                        }
                    }
                    Err(e) => {
                        println!("tool call failed: {}", e);
                        self.messages
                            .push(Message::user(format!("tool call failed: {}", e)));
                    }
                }
            } else {
                println!("tool not found: {}", tool_call.name);
            }
        }
    }
    pub async fn chat(&mut self, support_tool: bool) -> Result<()> {
        println!("welcome to use simple chat client, use 'exit' to quit");

        loop {
            print!("> ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            input = input.trim().to_string();

            if input.is_empty() {
                continue;
            }

            if input == "exit" {
                break;
            }

            self.messages.push(Message::user(&input));
            let tool_definitions = if support_tool {
                // prepare tool list
                let tools = self.tool_set.tools();
                if !tools.is_empty() {
                    Some(
                        tools
                            .iter()
                            .map(|tool| crate::model::Tool {
                                name: tool.name(),
                                description: tool.description(),
                                parameters: tool.parameters(),
                            })
                            .collect(),
                    )
                } else {
                    None
                }
            } else {
                None
            };

            // create request
            let request = CompletionRequest {
                model: self.model.clone(),
                messages: self.messages.clone(),
                temperature: Some(0.7),
                tools: tool_definitions,
            };

            // send request
            let response = self.client.complete(request).await?;
            // get choice
            let choice = response.choices.first().unwrap();
            println!("AI > {}", choice.message.content);
            // analyze tool call
            self.analyze_tool_call(&choice.message).await;
        }

        Ok(())
    }
}
