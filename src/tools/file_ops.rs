use anyhow::Result;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub fn execute_read_file(args: &Value) -> Result<Value> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;

    let path = Path::new(path_str);
    if !path.exists() {
        return Ok(json!({
            "error": format!("File not found: {}", path_str),
            "success": false
        }));
    }

    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return Ok(json!({
                "error": format!("Could not read file: {}", e),
                "success": false
            }));
        }
    };

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    let start_line = args.get("start_line").and_then(|v| v.as_u64()).map(|n| n.max(1) as usize);
    let end_line = args.get("end_line").and_then(|v| v.as_u64()).map(|n| (n as usize).min(total_lines));

    if let (Some(start), Some(end)) = (start_line, end_line) {
        if start > total_lines || start > end {
            return Ok(json!({
                "error": format!("Invalid line range {}-{} for file with {} lines", start, end, total_lines),
                "success": false
            }));
        }
        let slice = &lines[(start - 1)..end];
        let numbered_content: Vec<String> = slice
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{}: {}", start + i, line))
            .collect();

        Ok(json!({
            "path": path_str,
            "total_lines": total_lines,
            "start_line": start,
            "end_line": end,
            "content": numbered_content.join("\n"),
            "success": true
        }))
    } else {
        Ok(json!({
            "path": path_str,
            "total_lines": total_lines,
            "content": content,
            "success": true
        }))
    }
}

pub fn execute_write_file(args: &Value) -> Result<Value> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;

    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'content' argument"))?;

    let path = Path::new(path_str);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    fs::write(path, content)?;

    Ok(json!({
        "path": path_str,
        "bytes_written": content.len(),
        "success": true
    }))
}

pub fn execute_edit_file(args: &Value) -> Result<Value> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;

    let target_content = args
        .get("target_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'target_content' argument"))?;

    let replacement_content = args
        .get("replacement_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'replacement_content' argument"))?;

    let path = Path::new(path_str);
    if !path.exists() {
        return Ok(json!({
            "error": format!("File not found: {}", path_str),
            "success": false
        }));
    }

    let existing = fs::read_to_string(path)?;

    if !existing.contains(target_content) {
        return Ok(json!({
            "error": "Target content not found in file. Please verify the exact lines and whitespace.",
            "success": false
        }));
    }

    let count = existing.matches(target_content).count();
    if count > 1 {
        return Ok(json!({
            "error": format!("Target content matched {} times. Include more surrounding lines to make it unique.", count),
            "success": false
        }));
    }

    let updated = existing.replacen(target_content, replacement_content, 1);
    crate::ui::print_diff(&existing, &updated);
    fs::write(path, updated)?;

    Ok(json!({
        "path": path_str,
        "success": true,
        "message": "File successfully modified"
    }))
}
