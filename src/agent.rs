use anyhow::Result;
use colored::*;
use crate::client::{DeepSeekClient, Message};
use crate::prompt::get_system_prompt;
use crate::tools::{execute_tool, get_tool_definitions};
use crate::ui::{print_tool_result, print_tool_start};

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Thinking(bool),
    ToolStart { name: String, args: String },
    ToolResult { name: String, result: String, success: bool },
    AssistantResponse(String),
    TurnFinished(Vec<Message>),
    Error(String),
    Interrupted,
}

pub struct Agent {
    pub client: DeepSeekClient,
    pub messages: Vec<Message>,
    pub current_dir: String,
    pub auto_approve: bool,
    pub max_turns: usize,
    pub event_tx: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
    pub confirm_tx: Option<tokio::sync::mpsc::Sender<(String, tokio::sync::oneshot::Sender<bool>)>>,
    /// Flag compartilhado com a GUI: reflete o estado do toggle em tempo real.
    pub auto_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Agent {
    pub fn new(client: DeepSeekClient, current_dir: String, auto_approve: bool) -> Self {
        let mut agent = Self {
            client,
            messages: Vec::new(),
            current_dir,
            auto_approve,
            max_turns: 25,
            event_tx: None,
            confirm_tx: None,
            auto_flag: None,
        };
        agent.reset();
        agent
    }

    pub fn reset(&mut self) {
        self.messages.clear();
        self.messages.push(Message {
            role: "system".to_string(),
            content: Some(get_system_prompt(&self.current_dir)),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });
    }

    pub async fn process_user_input(&mut self, user_input: &str) -> Result<String> {
        self.messages.push(Message {
            role: "user".to_string(),
            content: Some(user_input.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });

        let tool_definitions = get_tool_definitions();
        let mut turns = 0;

        loop {
            turns += 1;
            if turns > self.max_turns {
                let err_msg = "Atingido o limite de iterações consecutivas para esta tarefa.".to_string();
                if let Some(tx) = &self.event_tx {
                    let _ = tx.send(AgentEvent::Error(err_msg.clone())).await;
                    let _ = tx.send(AgentEvent::TurnFinished(self.messages.clone())).await;
                }
                println!("{}", err_msg.red().bold());
                return Ok(err_msg);
            }

            if let Some(tx) = &self.event_tx {
                let _ = tx.send(AgentEvent::Thinking(true)).await;
            } else {
                print!("{}", "🤖 DeepGravity está pensando...".dimmed());
                std::io::Write::flush(&mut std::io::stdout())?;
            }

            let messages_to_send = prune_messages_for_request(&self.messages, 140_000);
            let response = match self
                .client
                .chat_completion(&messages_to_send, Some(tool_definitions.clone()))
                .await
            {
                Ok(resp) => {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(AgentEvent::Thinking(false)).await;
                    } else {
                        print!("\r{}\r", " ".repeat(40));
                        std::io::Write::flush(&mut std::io::stdout())?;
                    }
                    resp
                }
                Err(err) => {
                    if let Some(tx) = &self.event_tx {
                        let _ = tx.send(AgentEvent::Thinking(false)).await;
                        let _ = tx.send(AgentEvent::Error(err.to_string())).await;
                    } else {
                        print!("\r{}\r", " ".repeat(40));
                        std::io::Write::flush(&mut std::io::stdout())?;
                    }
                    return Err(err);
                }
            };

            let choice = response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("Received empty choices from DeepSeek API"))?;

            let assistant_msg = choice.message;
            let _finish_reason = choice.finish_reason.unwrap_or_default();

            let mut active_tool_calls = assistant_msg.tool_calls.clone();
            if active_tool_calls.as_ref().map_or(true, |c| c.is_empty()) {
                if let Some(content) = &assistant_msg.content {
                    active_tool_calls = extract_fallback_tool_calls(content);
                }
            }

            // Verifica se o modelo solicitou execução de ferramentas (nativo ou DSML/JSON)
            if let Some(tool_calls) = active_tool_calls {
                if !tool_calls.is_empty() {
                    let mut msg_to_store = assistant_msg.clone();
                    msg_to_store.tool_calls = Some(tool_calls.clone());
                    self.messages.push(msg_to_store);

                    for call in &tool_calls {
                        if let Some(tx) = &self.event_tx {
                            let _ = tx.send(AgentEvent::ToolStart {
                                name: call.function.name.clone(),
                                args: call.function.arguments.clone(),
                            }).await;
                        } else {
                            print_tool_start(&call.function.name, &call.function.arguments);
                        }

                        let effective_auto = self
                            .auto_flag
                            .as_ref()
                            .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
                            .unwrap_or(self.auto_approve);

                        let tool_result = execute_tool(
                            &call.function.name,
                            &call.function.arguments,
                            &self.current_dir,
                            effective_auto,
                            self.confirm_tx.as_ref(),
                        )
                        .await?;

                        let success = tool_result
                            .get("success")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);

                        let result_str = serde_json::to_string(&tool_result).unwrap_or_default();
                        let summary = if result_str.chars().count() > 140 {
                            let head: String = result_str.chars().take(140).collect();
                            format!("{}... [{} bytes]", head, result_str.len())
                        } else {
                            result_str.clone()
                        };

                        if let Some(tx) = &self.event_tx {
                            let _ = tx.send(AgentEvent::ToolResult {
                                name: call.function.name.clone(),
                                result: result_str.clone(),
                                success,
                            }).await;
                        } else {
                            print_tool_result(&call.function.name, success, &summary);
                        }

                        let stored_content = if result_str.len() > 18_000 {
                            let head: String = result_str.chars().take(12_000).collect();
                            let tail: String = result_str.chars().rev().take(4_000).collect::<String>().chars().rev().collect();
                            format!("{}\n\n[... resultado truncado pelo DeepGravity (total: {} bytes) ...]\n\n{}", head, result_str.len(), tail)
                        } else {
                            result_str
                        };

                        self.messages.push(Message {
                            role: "tool".to_string(),
                            content: Some(stored_content),
                            tool_calls: None,
                            tool_call_id: Some(call.id.clone()),
                            name: Some(call.function.name.clone()),
                        });
                    }

                    // Continua o loop para a IA analisar o resultado das ferramentas
                    continue;
                }
            }

            // Resposta textual final
            let final_text = assistant_msg.content.unwrap_or_default();
            self.messages.push(Message {
                role: "assistant".to_string(),
                content: Some(final_text.clone()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });

            if let Some(tx) = &self.event_tx {
                let _ = tx.send(AgentEvent::AssistantResponse(final_text.clone())).await;
                let _ = tx.send(AgentEvent::TurnFinished(self.messages.clone())).await;
            }

            return Ok(final_text);
        }
    }
}

fn extract_fallback_tool_calls(text: &str) -> Option<Vec<crate::client::ToolCall>> {
    use regex::Regex;

    // Formato DSML: <｜｜DSML｜｜ calls> ... </｜｜DSML｜｜ calls>
    if (text.contains("calls>") || text.contains("invoke")) && (text.contains("DSML") || text.contains("<｜")) {
        let calls_re = Regex::new(r"(?s)<[｜|]{2}DSML[｜|]{2}\s*calls>(.*?)</[｜|]{2}DSML[｜|]{2}\s*calls>").ok()?;
        let invoke_re = Regex::new(r#"(?s)<[｜|]{2}DSML[｜|]{2}\s*invoke\s+name=["']([\w.-]+)["']>(.*?)</[｜|]{2}DSML[｜|]{2}\s*invoke>"#).ok()?;
        let param_re = Regex::new(r#"(?s)<[｜|]{2}DSML[｜|]{2}\s*parameter\s+name=["']([\w.-]+)["'][^>]*>(.*?)</[｜|]{2}DSML[｜|]{2}\s*parameter>"#).ok()?;

        let body = if let Some(calls_cap) = calls_re.captures(text) {
            calls_cap[1].to_string()
        } else {
            text.to_string()
        };

        let mut calls = Vec::new();
        for inv in invoke_re.captures_iter(&body) {
            let name = inv[1].to_string();
            let inv_body = &inv[2];
            let mut args_map = serde_json::Map::new();

            for p in param_re.captures_iter(inv_body) {
                let pname = p[1].to_string();
                let pval = p[2].trim();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(pval) {
                    args_map.insert(pname, v);
                } else {
                    args_map.insert(pname, serde_json::Value::String(pval.to_string()));
                }
            }

            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);

            calls.push(crate::client::ToolCall {
                id: format!("call_{:x}", nanos),
                call_type: "function".to_string(),
                function: crate::client::FunctionCall {
                    name,
                    arguments: serde_json::Value::Object(args_map).to_string(),
                },
            });
        }
        if !calls.is_empty() {
            return Some(calls);
        }
    }

    None
}

/// Limita o tamanho do histórico enviado à API para não ultrapassar limites de prompt (ex: 200k chars no gateway).
/// Preserva a mensagem de sistema e as mensagens mais recentes.
fn prune_messages_for_request(messages: &[Message], max_chars: usize) -> Vec<Message> {
    if messages.is_empty() {
        return Vec::new();
    }
    let total_chars: usize = messages
        .iter()
        .map(|m| m.content.as_ref().map_or(0, |c| c.len()))
        .sum();
    if total_chars <= max_chars {
        return messages.to_vec();
    }

    let has_system = messages.first().map_or(false, |m| m.role == "system");
    let system_msg = if has_system { Some(messages[0].clone()) } else { None };
    let non_system = if has_system { &messages[1..] } else { messages };

    let mut budget = max_chars.saturating_sub(
        system_msg
            .as_ref()
            .and_then(|m| m.content.as_ref())
            .map_or(0, |c| c.len()),
    );
    let mut kept_rev = Vec::new();

    for msg in non_system.iter().rev() {
        let msg_len = msg.content.as_ref().map_or(0, |c| c.len());
        if msg_len <= budget || kept_rev.is_empty() {
            budget = budget.saturating_sub(msg_len);
            kept_rev.push(msg.clone());
        } else {
            break;
        }
    }

    let mut out = Vec::new();
    if let Some(sys) = system_msg {
        out.push(sys);
    }
    out.extend(kept_rev.into_iter().rev());
    out
}
