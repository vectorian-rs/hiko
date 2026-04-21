//! LLM client for OpenAI and Anthropic chat APIs with SSE streaming.

use crate::config::ApiFormat;
use serde::{Deserialize, Serialize};

// ── Shared types (internal message representation) ──────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

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

// ── Chat request (format-agnostic) ──────────────────────────────────

pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Option<Vec<ToolDef>>,
    pub max_tokens: Option<u32>,
    pub stream: bool,
}

// ── Accumulator (shared result) ─────────────────────────────────────

#[derive(Debug, Default)]
pub struct StreamAccumulator {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

impl StreamAccumulator {
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls
            .iter()
            .any(|tc| !tc.id.is_empty() && !tc.function.name.is_empty())
    }
}

// ── Client ──────────────────────────────────────────────────────────

pub struct LlmClient {
    base_url: String,
    api_key: String,
    format: ApiFormat,
    agent: ureq::Agent,
}

impl LlmClient {
    pub fn new(base_url: String, api_key: String, format: ApiFormat) -> Self {
        let agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder()
                .timeout_global(Some(std::time::Duration::from_secs(120)))
                .build(),
        );
        Self {
            base_url,
            api_key,
            format,
            agent,
        }
    }

    pub fn chat(
        &self,
        request: &ChatRequest,
        on_text: impl FnMut(&str),
    ) -> Result<StreamAccumulator, String> {
        match self.format {
            ApiFormat::Openai => self.chat_openai(request, on_text),
            ApiFormat::Anthropic => self.chat_anthropic(request, on_text),
        }
    }

    // ── OpenAI ──────────────────────────────────────────────────────

    fn chat_openai(
        &self,
        request: &ChatRequest,
        mut on_text: impl FnMut(&str),
    ) -> Result<StreamAccumulator, String> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "stream": request.stream,
            "max_tokens": request.max_tokens,
            "tools": request.tools,
        });
        let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;

        let response = self
            .agent
            .post(&url)
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
            if line.is_empty() || !line.starts_with("data: ") {
                continue;
            }
            let data = &line[6..];
            if data == "[DONE]" {
                break;
            }
            let chunk: OpenAiChunk =
                serde_json::from_str(data).map_err(|e| format!("stream parse error: {e}"))?;
            for choice in &chunk.choices {
                if let Some(ref content) = choice.delta.content {
                    on_text(content);
                }
                if let Some(ref reason) = choice.finish_reason {
                    acc.finish_reason = Some(reason.clone());
                }
                if let Some(ref content) = choice.delta.content {
                    acc.text.push_str(content);
                }
                if let Some(ref calls) = choice.delta.tool_calls {
                    for tc in calls {
                        while acc.tool_calls.len() <= tc.index {
                            acc.tool_calls.push(ToolCall {
                                id: String::new(),
                                kind: "function".into(),
                                function: FunctionCall {
                                    name: String::new(),
                                    arguments: String::new(),
                                },
                            });
                        }
                        let entry = &mut acc.tool_calls[tc.index];
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

        Ok(acc)
    }

    // ── Anthropic ───────────────────────────────────────────────────

    fn chat_anthropic(
        &self,
        request: &ChatRequest,
        mut on_text: impl FnMut(&str),
    ) -> Result<StreamAccumulator, String> {
        let url = format!("{}/messages", self.base_url);

        // Extract system prompt from messages (Anthropic uses a top-level field)
        let system: Option<String> = request
            .messages
            .iter()
            .find(|m| m.role == "system")
            .and_then(|m| m.content.clone());

        // Convert messages to Anthropic format
        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| self.message_to_anthropic(m))
            .collect();

        // Build tools in Anthropic format
        let tools: Option<Vec<serde_json::Value>> = request.tools.as_ref().map(|defs| {
            defs.iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.function.name,
                        "description": t.function.description,
                        "input_schema": t.function.parameters,
                    })
                })
                .collect()
        });

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": messages,
            "stream": request.stream,
            "max_tokens": request.max_tokens.unwrap_or(4096),
        });
        if let Some(sys) = system {
            body["system"] = serde_json::Value::String(sys);
        }
        if let Some(t) = tools {
            body["tools"] = serde_json::Value::Array(t);
        }

        let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;

        let response = self
            .agent
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .send(body_str.as_bytes())
            .map_err(|e| format!("LLM request failed: {e}"))?;

        let reader =
            std::io::BufRead::lines(std::io::BufReader::new(response.into_body().into_reader()));
        let mut acc = StreamAccumulator::default();
        // Track current content block for tool_use streaming
        let mut current_tool_index: Option<usize> = None;

        for line in reader {
            let line = line.map_err(|e| format!("stream read error: {e}"))?;
            let line = line.trim_start();
            if line.is_empty() {
                continue;
            }
            // Anthropic SSE has "event: ..." and "data: ..." lines
            if !line.starts_with("data: ") {
                continue;
            }
            let data = &line[6..];
            let event: AnthropicEvent = match serde_json::from_str(data) {
                Ok(e) => e,
                Err(_) => continue,
            };

            match event.event_type.as_str() {
                "content_block_start" => {
                    if let Some(ref block) = event.content_block {
                        if block.block_type == "tool_use" {
                            let idx = acc.tool_calls.len();
                            acc.tool_calls.push(ToolCall {
                                id: block.id.clone().unwrap_or_default(),
                                kind: "function".into(),
                                function: FunctionCall {
                                    name: block.name.clone().unwrap_or_default(),
                                    arguments: String::new(),
                                },
                            });
                            current_tool_index = Some(idx);
                        } else {
                            current_tool_index = None;
                        }
                    }
                }
                "content_block_delta" => {
                    if let Some(ref delta) = event.delta {
                        match delta.delta_type.as_str() {
                            "text_delta" => {
                                if let Some(ref text) = delta.text {
                                    on_text(text);
                                    acc.text.push_str(text);
                                }
                            }
                            "input_json_delta" => {
                                if let Some(idx) = current_tool_index
                                    && let Some(ref json) = delta.partial_json
                                {
                                    acc.tool_calls[idx].function.arguments.push_str(json);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "content_block_stop" => {
                    current_tool_index = None;
                }
                "message_delta" => {
                    if let Some(ref delta) = event.delta
                        && let Some(ref reason) = delta.stop_reason
                    {
                        acc.finish_reason = Some(reason.clone());
                    }
                }
                "message_stop" => break,
                _ => {}
            }
        }

        Ok(acc)
    }

    /// Convert an internal Message to Anthropic wire format.
    fn message_to_anthropic(&self, msg: &Message) -> serde_json::Value {
        // Tool results → user message with tool_result content blocks
        if msg.role == "tool" {
            return serde_json::json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": msg.tool_call_id,
                    "content": msg.content,
                }],
            });
        }

        // Assistant messages with tool calls → content blocks
        if msg.role == "assistant"
            && let Some(ref calls) = msg.tool_calls
        {
            let mut content: Vec<serde_json::Value> = Vec::new();
            if let Some(ref text) = msg.content
                && !text.is_empty()
            {
                content.push(serde_json::json!({"type": "text", "text": text}));
            }
            for tc in calls {
                let input: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.function.name,
                    "input": input,
                }));
            }
            return serde_json::json!({"role": "assistant", "content": content});
        }

        // Regular user/assistant messages
        serde_json::json!({
            "role": msg.role,
            "content": msg.content,
        })
    }
}

// ── OpenAI streaming types ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct OpenAiChunk {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    delta: OpenAiDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolCall {
    index: usize,
    id: Option<String>,
    function: Option<OpenAiStreamFunctionCall>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamFunctionCall {
    name: Option<String>,
    arguments: Option<String>,
}

// ── Anthropic streaming types ───────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AnthropicEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(default)]
    delta: Option<AnthropicDelta>,
    #[serde(default)]
    content_block: Option<AnthropicContentBlock>,
}

#[derive(Debug, Deserialize)]
struct AnthropicDelta {
    #[serde(rename = "type", default)]
    delta_type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}
