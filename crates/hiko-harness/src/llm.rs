//! LLM client for OpenAI-compatible chat completions API with SSE streaming.

use serde::{Deserialize, Serialize};

// ── Request types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default = "default_true_val")]
    pub stream: bool,
}

fn default_true_val() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionDef,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// ── Response types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

// ── Streaming SSE types ──────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct StreamChunk {
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamChoice {
    pub delta: StreamDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamDelta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<StreamToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamToolCall {
    pub index: usize,
    pub id: Option<String>,
    pub function: Option<StreamFunctionCall>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamFunctionCall {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

// ── Accumulator ──────────────────────────────────────────────────────

/// Accumulates streaming SSE deltas into complete tool calls and text.
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

impl StreamAccumulator {
    pub fn feed(&mut self, chunk: &StreamChunk) {
        for choice in &chunk.choices {
            if let Some(ref reason) = choice.finish_reason {
                self.finish_reason = Some(reason.clone());
            }
            if let Some(ref content) = choice.delta.content {
                self.text.push_str(content);
            }
            if let Some(ref calls) = choice.delta.tool_calls {
                for tc in calls {
                    // Grow tool_calls vec if needed
                    while self.tool_calls.len() <= tc.index {
                        self.tool_calls.push(ToolCall {
                            id: String::new(),
                            kind: "function".into(),
                            function: FunctionCall {
                                name: String::new(),
                                arguments: String::new(),
                            },
                        });
                    }
                    let entry = &mut self.tool_calls[tc.index];
                    if let Some(ref id) = tc.id {
                        entry.id = id.clone();
                    }
                    if let Some(ref f) = tc.function {
                        if let Some(ref name) = f.name {
                            entry.function.name = name.clone();
                        }
                        if let Some(ref args) = f.arguments {
                            entry.function.arguments.push_str(args);
                        }
                    }
                }
            }
        }
    }

    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

// ── Client ───────────────────────────────────────────────────────────

pub struct LlmClient {
    base_url: String,
    api_key: String,
}

impl LlmClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self { base_url, api_key }
    }

    /// Send a streaming chat completion request. Calls `on_text` for each
    /// text delta as it arrives. Returns the accumulated result.
    pub fn chat(
        &self,
        request: &ChatRequest,
        mut on_text: impl FnMut(&str),
    ) -> Result<StreamAccumulator, String> {
        let url = format!("{}/chat/completions", self.base_url);

        let body_str = serde_json::to_string(request).map_err(|e| e.to_string())?;
        let response = ureq::post(&url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .send(body_str.as_bytes())
            .map_err(|e| format!("LLM request failed: {e}"))?;

        let reader =
            std::io::BufRead::lines(std::io::BufReader::new(response.into_body().into_reader()));
        let mut acc = StreamAccumulator::default();

        for line in reader {
            let line = line.map_err(|e| format!("stream read error: {e}"))?;
            let line = line.trim_start();
            if !line.starts_with("data: ") {
                continue;
            }
            let data = &line[6..];
            if data == "[DONE]" {
                break;
            }
            let chunk: StreamChunk =
                serde_json::from_str(data).map_err(|e| format!("stream parse error: {e}"))?;

            // Stream text deltas to the caller
            for choice in &chunk.choices {
                if let Some(ref content) = choice.delta.content {
                    on_text(content);
                }
            }

            acc.feed(&chunk);
        }

        Ok(acc)
    }
}
