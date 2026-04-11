//! Agent loop: LLM conversation cycle with tool dispatch.

use crate::llm::{ChatRequest, LlmClient, Message};
use crate::tools::ToolRegistry;

pub struct AgentConfig {
    pub model: String,
    pub system_prompt: String,
    pub max_turns: usize,
    pub max_tokens: u32,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "gpt-4o".into(),
            system_prompt: String::new(),
            max_turns: 50,
            max_tokens: 4096,
        }
    }
}

pub struct Agent {
    client: LlmClient,
    tools: ToolRegistry,
    config: AgentConfig,
    messages: Vec<Message>,
}

impl Agent {
    pub fn new(client: LlmClient, tools: ToolRegistry, config: AgentConfig) -> Self {
        let mut messages = Vec::new();

        if !config.system_prompt.is_empty() {
            messages.push(Message {
                role: "system".into(),
                content: Some(config.system_prompt.clone()),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        Self {
            client,
            tools,
            config,
            messages,
        }
    }

    /// Run a single user prompt through the agent loop.
    /// Returns the final text response.
    pub fn run(&mut self, user_message: &str) -> Result<String, String> {
        self.messages.push(Message {
            role: "user".into(),
            content: Some(user_message.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });

        let tool_defs = self.tools.tool_defs();

        for _turn in 0..self.config.max_turns {
            let request = ChatRequest {
                model: self.config.model.clone(),
                messages: self.messages.clone(),
                tools: if tool_defs.is_empty() {
                    None
                } else {
                    Some(tool_defs.clone())
                },
                max_tokens: Some(self.config.max_tokens),
                stream: true,
            };

            // Stream text to stdout as it arrives
            let result = self.client.chat(&request, |text| {
                print!("{text}");
            })?;

            if result.has_tool_calls() {
                // Append assistant message with tool calls
                self.messages.push(Message {
                    role: "assistant".into(),
                    content: if result.text.is_empty() {
                        None
                    } else {
                        Some(result.text.clone())
                    },
                    tool_calls: Some(
                        result
                            .tool_calls
                            .iter()
                            .map(|tc| crate::llm::ToolCall {
                                id: tc.id.clone(),
                                kind: "function".into(),
                                function: crate::llm::FunctionCall {
                                    name: tc.function.name.clone(),
                                    arguments: tc.function.arguments.clone(),
                                },
                            })
                            .collect(),
                    ),
                    tool_call_id: None,
                });

                // Execute each tool call
                for tc in &result.tool_calls {
                    eprintln!(
                        "\n[tool: {} args: {}]",
                        tc.function.name,
                        truncate(&tc.function.arguments, 100)
                    );

                    let tool_result = match self
                        .tools
                        .execute(&tc.function.name, &tc.function.arguments)
                    {
                        Ok(output) => output,
                        Err(e) => format!("Error: {e}"),
                    };

                    eprintln!("[result: {}]", truncate(&tool_result, 200));

                    self.messages.push(Message {
                        role: "tool".into(),
                        content: Some(tool_result),
                        tool_calls: None,
                        tool_call_id: Some(tc.id.clone()),
                    });
                }
            } else {
                // No tool calls — we have a final text response
                if !result.text.is_empty() {
                    println!();
                }
                self.messages.push(Message {
                    role: "assistant".into(),
                    content: Some(result.text.clone()),
                    tool_calls: None,
                    tool_call_id: None,
                });
                return Ok(result.text);
            }
        }

        Err(format!(
            "agent exceeded max turns ({})",
            self.config.max_turns
        ))
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s.char_indices()
            .take_while(|(i, _)| *i < max)
            .map(|(_, c)| c)
            .collect::<String>()
            + "..."
    }
}
