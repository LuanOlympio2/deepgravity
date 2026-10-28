use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Choice {
    pub index: usize,
    pub message: Message,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ChatCompletionResponse {
    pub id: Option<String>,
    pub choices: Vec<Choice>,
}

#[derive(Clone)]
pub struct DeepSeekClient {
    client: Client,
    pub base_url: String,
    pub model: String,
}

impl DeepSeekClient {
    pub fn new(base_url: &str, model: &str) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_default(),
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
        }
    }

    pub async fn chat_completion(
        &self,
        messages: &[Message],
        tools: Option<Value>,
    ) -> Result<ChatCompletionResponse> {
        let url = format!("{}/chat/completions", self.base_url);

        let req = ChatCompletionRequest {
            model: self.model.clone(),
            messages: build_messages_json(messages),
            tools,
            tool_choice: Some(serde_json::json!("auto")),
        };

        let resp = self
            .client
            .post(&url)
            .json(&req)
            .send()
            .await
            .with_context(|| format!("Failed to connect to DeepSeek API at {}", url))?;

        let status = resp.status();
        let body_text = resp
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read response body>".to_string());

        if !status.is_success() {
            anyhow::bail!(
                "API returned HTTP error {}: {}",
                status,
                body_text
            );
        }

        let parsed: ChatCompletionResponse = serde_json::from_str(&body_text)
            .with_context(|| format!("Failed to parse response JSON: {}", body_text))?;

        Ok(parsed)
    }
}

// ---------------------------------------------------------------------------
// Anexos (imagens / documentos)
// ---------------------------------------------------------------------------

/// Marcador de anexo embutido no texto da mensagem do usuário: "[[anexo:/caminho/arquivo.png]]".
/// O caminho é lido do disco e convertido em data URI somente na hora de enviar à API,
/// o que mantém as sessões salvas leves.
const ATTACH_OPEN: &str = "[[anexo:";
const ATTACH_CLOSE: &str = "]]";

pub fn attach_marker(path: &str) -> String {
    format!("{}{}{}", ATTACH_OPEN, path, ATTACH_CLOSE)
}

/// Separa o texto limpo dos caminhos de anexos.
pub fn split_attachments(text: &str) -> (String, Vec<String>) {
    let mut clean = String::new();
    let mut paths = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(ATTACH_OPEN) {
        clean.push_str(&rest[..start]);
        let after = &rest[start + ATTACH_OPEN.len()..];
        if let Some(end) = after.find(ATTACH_CLOSE) {
            paths.push(after[..end].to_string());
            rest = &after[end + ATTACH_CLOSE.len()..];
        } else {
            clean.push_str(&rest[start..]);
            rest = "";
            break;
        }
    }
    clean.push_str(rest);
    (clean.trim().to_string(), paths)
}

pub fn is_image_path(path: &str) -> bool {
    let p = path.to_lowercase();
    [".png", ".jpg", ".jpeg", ".webp", ".gif"].iter().any(|e| p.ends_with(e))
}

fn mime_for(path: &str) -> &'static str {
    let p = path.to_lowercase();
    if p.ends_with(".png") {
        "image/png"
    } else if p.ends_with(".jpg") || p.ends_with(".jpeg") {
        "image/jpeg"
    } else if p.ends_with(".webp") {
        "image/webp"
    } else if p.ends_with(".gif") {
        "image/gif"
    } else if p.ends_with(".pdf") {
        "application/pdf"
    } else if p.ends_with(".txt") || p.ends_with(".md") || p.ends_with(".log") {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Converte o histórico em JSON da API. Somente a ÚLTIMA mensagem do usuário tem os anexos
/// realmente enviados; nas anteriores o marcador vira uma nota textual.
fn build_messages_json(messages: &[Message]) -> Vec<Value> {
    use base64::Engine;
    let last_user = messages.iter().rposition(|m| m.role == "user");
    messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut v = serde_json::to_value(m).unwrap_or(Value::Null);
            if m.role != "user" {
                return v;
            }
            let Some(content) = m.content.as_deref() else { return v };
            if !content.contains(ATTACH_OPEN) {
                return v;
            }
            let (text, paths) = split_attachments(content);
            if Some(i) != last_user {
                let names: Vec<String> = paths.iter().map(|p| file_name_of(p)).collect();
                let note = format!("{}\n[anexos enviados anteriormente: {}]", text, names.join(", "));
                v["content"] = Value::String(note.trim().to_string());
                return v;
            }
            let mut parts: Vec<Value> = Vec::new();
            let mut text_out = text;
            for p in &paths {
                match std::fs::read(p) {
                    Ok(bytes) if bytes.len() <= 12 * 1024 * 1024 => {
                        let uri = format!(
                            "data:{};base64,{}",
                            mime_for(p),
                            base64::engine::general_purpose::STANDARD.encode(&bytes)
                        );
                        if is_image_path(p) {
                            parts.push(serde_json::json!({"type": "image_url", "image_url": {"url": uri}}));
                        } else {
                            parts.push(serde_json::json!({
                                "type": "file",
                                "file": {"filename": file_name_of(p), "file_data": uri}
                            }));
                        }
                    }
                    Ok(_) => text_out.push_str(&format!("\n[anexo ignorado (maior que 12 MB): {}]", file_name_of(p))),
                    Err(e) => text_out.push_str(&format!("\n[anexo ilegível {}: {}]", file_name_of(p), e)),
                }
            }
            if text_out.trim().is_empty() {
                text_out = "Analise o(s) anexo(s).".to_string();
            }
            let mut content_parts = vec![serde_json::json!({"type": "text", "text": text_out})];
            content_parts.extend(parts);
            v["content"] = Value::Array(content_parts);
            v
        })
        .collect()
}
