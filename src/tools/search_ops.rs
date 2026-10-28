use anyhow::Result;
use regex::RegexBuilder;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

pub fn execute_list_dir(args: &Value, current_dir: &str) -> Result<Value> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(current_dir);

    let path = Path::new(path_str);
    if !path.exists() {
        return Ok(json!({
            "error": format!("Directory not found: {}", path_str),
            "success": false
        }));
    }

    if !path.is_dir() {
        return Ok(json!({
            "error": format!("Path is not a directory: {}", path_str),
            "success": false
        }));
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        let file_type = if meta.is_dir() {
            "directory"
        } else if meta.is_symlink() {
            "symlink"
        } else {
            "file"
        };

        entries.push(json!({
            "name": entry.file_name().to_string_lossy().to_string(),
            "type": file_type,
            "size_bytes": meta.len()
        }));
    }

    Ok(json!({
        "path": path_str,
        "entries": entries,
        "total_entries": entries.len(),
        "success": true
    }))
}

pub fn execute_search_files(args: &Value, current_dir: &str) -> Result<Value> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'query' argument"))?;

    let search_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(current_dir);

    let case_sensitive = args
        .get("case_sensitive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(50) as usize;

    let re = RegexBuilder::new(query)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|e| anyhow::anyhow!("Invalid regex pattern: {}", e))?;

    let mut matches = Vec::new();

    for entry in WalkDir::new(search_path)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.') && name != "target" && name != "node_modules" && name != ".venv"
        })
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            if let Ok(content) = fs::read_to_string(entry.path()) {
                for (line_idx, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        matches.push(json!({
                            "file": entry.path().to_string_lossy().to_string(),
                            "line_number": line_idx + 1,
                            "line_content": line.trim()
                        }));

                        if matches.len() >= max_results {
                            break;
                        }
                    }
                }
            }
        }
        if matches.len() >= max_results {
            break;
        }
    }

    Ok(json!({
        "query": query,
        "path": search_path,
        "matches": matches,
        "total_matches": matches.len(),
        "success": true
    }))
}

pub fn execute_find_files(args: &Value, current_dir: &str) -> Result<Value> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' argument"))?;

    let search_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or(current_dir);

    let re = RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|e| anyhow::anyhow!("Invalid pattern regex: {}", e))?;

    let mut matches = Vec::new();

    for entry in WalkDir::new(search_path)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.') && name != "target" && name != "node_modules" && name != ".venv"
        })
        .filter_map(|e| e.ok())
    {
        let file_name = entry.file_name().to_string_lossy();
        if re.is_match(&file_name) {
            matches.push(json!({
                "path": entry.path().to_string_lossy().to_string(),
                "is_dir": entry.file_type().is_dir()
            }));

            if matches.len() >= 50 {
                break;
            }
        }
    }

    Ok(json!({
        "pattern": pattern,
        "matches": matches,
        "total_matches": matches.len(),
        "success": true
    }))
}
