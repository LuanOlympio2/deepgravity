use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::client::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChatItem {
    User(String),
    Assistant(String),
    Tool {
        name: String,
        args: String,
        result: Option<String>,
        success: bool,
        expanded: bool,
    },
    Error(String),
    Notice(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSession {
    pub id: String,
    pub title: String,
    pub project_path: String,
    pub model: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub items: Vec<ChatItem>,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMeta {
    pub id: String,
    pub title: String,
    pub project_path: String,
    pub model: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub item_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStore {
    pub current: String,
    pub recent: Vec<String>,
}

impl Default for ProjectStore {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        Self {
            current: home.clone(),
            recent: vec![home],
        }
    }
}

pub fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn format_relative_time(timestamp: u64) -> String {
    let now = now_timestamp();
    if timestamp == 0 || timestamp > now {
        return "agora".to_string();
    }
    let diff = now - timestamp;
    if diff < 60 {
        "agora mesmo".to_string()
    } else if diff < 3600 {
        let m = diff / 60;
        format!("há {} min", m)
    } else if diff < 86400 {
        let h = diff / 3600;
        if h == 1 { "há 1 hora".to_string() } else { format!("há {} horas", h) }
    } else {
        let d = diff / 86400;
        if d == 1 { "ontem".to_string() } else { format!("há {} dias", d) }
    }
}

pub fn generate_session_id() -> String {
    let now = now_timestamp();
    let counter: u32 = (std::process::id() ^ (now as u32)) % 9999;
    format!("chat_{}_{:04}", now, counter)
}

pub fn title_from_prompt(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let first_line = trimmed.lines().next().unwrap_or(trimmed);
    let clean = first_line.replace(['\n', '\r', '\t'], " ");
    let clean = clean.trim();
    if clean.chars().count() > 36 {
        let s: String = clean.chars().take(34).collect();
        format!("{}…", s)
    } else if clean.is_empty() {
        "Nova conversa".to_string()
    } else {
        clean.to_string()
    }
}

pub fn get_data_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local/share/deepgravity")
    } else {
        PathBuf::from(".deepgravity")
    }
}

fn sessions_dir() -> PathBuf {
    get_data_dir().join("sessions")
}

fn projects_file() -> PathBuf {
    get_data_dir().join("projects.json")
}

pub fn ensure_storage_dirs() -> Result<()> {
    let sdir = sessions_dir();
    if !sdir.exists() {
        fs::create_dir_all(&sdir).context("Falha ao criar diretório de sessões")?;
    }
    Ok(())
}

pub fn save_session(session: &ConversationSession) -> Result<()> {
    ensure_storage_dirs()?;
    let path = sessions_dir().join(format!("{}.json", session.id));
    let json = serde_json::to_string_pretty(session)?;
    fs::write(&path, json)?;
    Ok(())
}

pub fn load_session(id: &str) -> Result<ConversationSession> {
    let path = sessions_dir().join(format!("{}.json", id));
    let data = fs::read_to_string(&path)
        .with_context(|| format!("Sessão {} não encontrada", id))?;
    let session = serde_json::from_str(&data)?;
    Ok(session)
}

pub fn delete_session(id: &str) -> Result<()> {
    let path = sessions_dir().join(format!("{}.json", id));
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

pub fn list_sessions() -> Result<Vec<ConversationMeta>> {
    ensure_storage_dirs()?;
    let sdir = sessions_dir();
    let mut list = Vec::new();

    if let Ok(entries) = fs::read_dir(sdir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(data) = fs::read_to_string(&path) {
                    if let Ok(sess) = serde_json::from_str::<ConversationSession>(&data) {
                        list.push(ConversationMeta {
                            id: sess.id,
                            title: sess.title,
                            project_path: sess.project_path,
                            model: sess.model,
                            created_at: sess.created_at,
                            updated_at: sess.updated_at,
                            item_count: sess.items.len(),
                        });
                    }
                }
            }
        }
    }

    // Ordenar pelas mais recentes primeiro
    list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(list)
}

pub fn load_projects(default_dir: &str) -> ProjectStore {
    let _ = ensure_storage_dirs();
    let pfile = projects_file();
    if let Ok(data) = fs::read_to_string(pfile) {
        if let Ok(mut store) = serde_json::from_str::<ProjectStore>(&data) {
            if store.current.is_empty() {
                store.current = default_dir.to_string();
            }
            if !store.recent.iter().any(|r| r == &store.current) {
                store.recent.insert(0, store.current.clone());
            }
            return store;
        }
    }

    let store = ProjectStore {
        current: default_dir.to_string(),
        recent: vec![default_dir.to_string()],
    };
    let _ = save_projects(&store);
    store
}

pub fn save_projects(store: &ProjectStore) -> Result<()> {
    ensure_storage_dirs()?;
    let pfile = projects_file();
    let json = serde_json::to_string_pretty(store)?;
    fs::write(pfile, json)?;
    Ok(())
}

pub fn add_recent_project(store: &mut ProjectStore, new_path: &str) -> Result<()> {
    let canonical = Path::new(new_path)
        .canonicalize()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| new_path.to_string());

    store.current = canonical.clone();
    store.recent.retain(|p| p != &canonical);
    store.recent.insert(0, canonical);
    if store.recent.len() > 15 {
        store.recent.truncate(15);
    }
    save_projects(store)?;
    Ok(())
}
