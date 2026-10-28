//! Ajudantes de mídia: seletor de arquivos, gravação de microfone e transcrição de áudio.
//!
//! Tudo é feito com ferramentas já presentes no sistema (`zenity`, `arecord`, `ffmpeg`, `curl`),
//! sem adicionar dependências nativas pesadas ao aplicativo.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Endpoint público de reconhecimento de voz (o mesmo usado pelo Chrome).
const STT_URL: &str = "https://www.google.com/speech-api/v2/recognize?output=json&lang=pt-BR&key=AIzaSyBOti4mM-6x9WDnZIjIeyEU21OpBXqWBgw";

pub const AUDIO_EXTS: &[&str] = &["mp3", "wav", "ogg", "oga", "m4a", "flac", "aac", "opus", "webm", "wma"];

pub fn is_audio_path(path: &str) -> bool {
    let p = path.to_lowercase();
    AUDIO_EXTS.iter().any(|e| p.ends_with(&format!(".{}", e)))
}

fn unique_id() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

/// Abre o seletor de arquivos nativo (zenity). Retorna lista vazia se cancelado.
pub fn pick_files() -> Vec<String> {
    let out = Command::new("zenity")
        .args([
            "--file-selection",
            "--multiple",
            "--separator=\n",
            "--title=Anexar imagens, documentos ou áudio",
            "--file-filter=Mídia e documentos | *.png *.jpg *.jpeg *.webp *.gif *.pdf *.txt *.md *.mp3 *.wav *.ogg *.m4a *.flac *.opus *.aac",
            "--file-filter=Todos os arquivos | *",
        ])
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Gravação de microfone em andamento.
pub struct Recorder {
    child: Child,
    pub wav: PathBuf,
}

impl Recorder {
    pub fn start() -> Result<Self, String> {
        let wav = std::env::temp_dir().join(format!("deepgravity_rec_{}.wav", unique_id()));
        let child = Command::new("arecord")
            .args(["-q", "-f", "S16_LE", "-r", "16000", "-c", "1", "-t", "wav"])
            .arg(&wav)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Não foi possível iniciar o microfone (arecord): {}", e))?;
        Ok(Self { child, wav })
    }

    /// Finaliza a gravação (SIGINT para o arecord fechar o cabeçalho WAV corretamente).
    pub fn stop(mut self) -> PathBuf {
        let _ = Command::new("kill").args(["-INT", &self.child.id().to_string()]).status();
        let _ = self.child.wait();
        self.wav.clone()
    }

    #[allow(dead_code)]
    pub fn cancel(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.wav);
    }
}

/// Transcreve qualquer arquivo de áudio para texto (pt-BR). Divide em trechos de 25 s.
pub fn transcribe(path: &Path) -> Result<String, String> {
    let id = unique_id();
    let dir = std::env::temp_dir();
    let pattern = dir.join(format!("dg_stt_{}_%03d.flac", id));
    let status = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "segment", "-segment_time", "25", "-c:a", "flac"])
        .arg(&pattern)
        .status()
        .map_err(|e| format!("ffmpeg indisponível: {}", e))?;
    if !status.success() {
        return Err("Não consegui ler esse áudio (ffmpeg falhou).".into());
    }

    let mut transcript = String::new();
    for i in 0..400 {
        let seg = dir.join(format!("dg_stt_{}_{:03}.flac", id, i));
        if !seg.exists() {
            break;
        }
        let res = Command::new("curl")
            .args(["-s", "-m", "40", "-X", "POST", "-H", "Content-Type: audio/x-flac; rate=16000", "--data-binary"])
            .arg(format!("@{}", seg.display()))
            .arg(STT_URL)
            .output();
        let _ = std::fs::remove_file(&seg);
        let out = res.map_err(|e| format!("curl indisponível: {}", e))?;
        let body = String::from_utf8_lossy(&out.stdout).to_string();
        if body.trim().is_empty() {
            return Err("Sem resposta do serviço de reconhecimento de voz (verifique a internet).".into());
        }
        for line in body.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(t) = v["result"][0]["alternative"][0]["transcript"].as_str() {
                    if !transcript.is_empty() {
                        transcript.push(' ');
                    }
                    transcript.push_str(t.trim());
                    break;
                }
            }
        }
    }
    if transcript.trim().is_empty() {
        Err("Não consegui entender nenhuma fala nesse áudio.".into())
    } else {
        Ok(transcript)
    }
}
