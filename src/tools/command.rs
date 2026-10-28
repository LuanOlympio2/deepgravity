use anyhow::Result;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

pub async fn execute_run_command(args: &Value, current_dir: &str) -> Result<Value> {
    let command_str = args
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;

    let timeout_secs = args
        .get("timeout_seconds")
        .and_then(|v| v.as_u64())
        .unwrap_or(60);

    let working_dir = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .unwrap_or(current_dir);

    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(command_str);
    cmd.current_dir(working_dir);
    // Se o usuário apertar "Parar", a task é descartada e o processo filho morre junto.
    cmd.kill_on_drop(true);

    let res = timeout(Duration::from_secs(timeout_secs), cmd.output()).await;

    match res {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code().unwrap_or(-1);

            Ok(json!({
                "exit_code": exit_code,
                "stdout": stdout,
                "stderr": stderr,
                "success": output.status.success()
            }))
        }
        Ok(Err(e)) => Ok(json!({
            "error": format!("Failed to spawn process: {}", e),
            "success": false
        })),
        Err(_) => Ok(json!({
            "error": format!("Command timed out after {} seconds", timeout_secs),
            "success": false
        })),
    }
}
