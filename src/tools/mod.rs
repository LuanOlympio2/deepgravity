pub mod command;
pub mod file_ops;
pub mod search_ops;

use anyhow::Result;
use colored::*;
use serde_json::{json, Value};
use std::io::{self, Write};

pub fn get_tool_definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "description": "Execute a shell command in the system terminal (bash) and return stdout, stderr, and exit code.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The exact shell command to execute."
                        },
                        "cwd": {
                            "type": "string",
                            "description": "Optional working directory for the command."
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Maximum execution time in seconds (default: 60)."
                        }
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read the contents of a file. Supports line range slicing.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Absolute or relative file path to read."
                        },
                        "start_line": {
                            "type": "integer",
                            "description": "Optional starting line number (1-based)."
                        },
                        "end_line": {
                            "type": "integer",
                            "description": "Optional ending line number (inclusive)."
                        }
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Create a new file or completely overwrite an existing file with the given content.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to create or overwrite."
                        },
                        "content": {
                            "type": "string",
                            "description": "Full text content to write into the file."
                        }
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace a single specific block of content inside an existing file.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the existing file."
                        },
                        "target_content": {
                            "type": "string",
                            "description": "Exact text chunk inside the file to be replaced (must be unique)."
                        },
                        "replacement_content": {
                            "type": "string",
                            "description": "New content to insert in place of target_content."
                        }
                    },
                    "required": ["path", "target_content", "replacement_content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files and subdirectories inside a directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Directory path to list (defaults to current working directory)."
                        }
                    }
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_files",
                "description": "Search for a regex or text pattern inside files in a directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Regex or string pattern to look for."
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search inside."
                        },
                        "case_sensitive": {
                            "type": "boolean",
                            "description": "Whether the search should be case sensitive (default: false)."
                        }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_files",
                "description": "Find files by filename regex/pattern in a directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Regex pattern matching file names."
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory path to start searching."
                        }
                    },
                    "required": ["pattern"]
                }
            }
        }
    ])
}

pub async fn execute_tool(
    name: &str,
    arguments_json: &str,
    current_dir: &str,
    auto_approve: bool,
    confirm_tx: Option<&tokio::sync::mpsc::Sender<(String, tokio::sync::oneshot::Sender<bool>)>>,
) -> Result<Value> {
    let args: Value = match serde_json::from_str(arguments_json) {
        Ok(v) => v,
        Err(e) => {
            return Ok(json!({
                "error": format!("Malformed tool arguments JSON: {}", e),
                "success": false
            }));
        }
    };

    match name {
        "run_command" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if !auto_approve {
                if let Some(tx) = confirm_tx {
                    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                    if tx.send((cmd.to_string(), resp_tx)).await.is_ok() {
                        let approved = resp_rx.await.unwrap_or(false);
                        if !approved {
                            return Ok(json!({
                                "error": "Command execution was rejected by the user in GUI.",
                                "success": false
                            }));
                        }
                    }
                } else {
                    println!(
                        "{} {}",
                        "⚠️  Executar comando no terminal:".yellow().bold(),
                        cmd.bright_cyan()
                    );
                    print!("{} [Y/n]: ", "Deseja autorizar?".bold());
                    io::stdout().flush()?;

                    let mut input = String::new();
                    io::stdin().read_line(&mut input)?;
                    let choice = input.trim().to_lowercase();
                    if choice == "n" || choice == "no" || choice == "nao" || choice == "não" {
                        return Ok(json!({
                            "error": "Command execution was rejected by the user.",
                            "success": false
                        }));
                    }
                }
            }

            command::execute_run_command(&args, current_dir).await
        }
        "read_file" => file_ops::execute_read_file(&args),
        "write_file" => file_ops::execute_write_file(&args),
        "edit_file" => file_ops::execute_edit_file(&args),
        "list_dir" => search_ops::execute_list_dir(&args, current_dir),
        "search_files" => search_ops::execute_search_files(&args, current_dir),
        "find_files" => search_ops::execute_find_files(&args, current_dir),
        unknown => Ok(json!({
            "error": format!("Unknown tool function '{}'", unknown),
            "success": false
        })),
    }
}
