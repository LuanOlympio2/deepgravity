pub mod media;
pub mod theme;

use crate::agent::{Agent, AgentEvent};
use crate::client::{self, DeepSeekClient, Message};
use crate::storage::{self, ChatItem, ConversationMeta, ConversationSession, ProjectStore};
use eframe::egui::{
    self, text::LayoutJob, Align, Align2, Color32, FontFamily, FontId, Frame,
    Key, Layout, Margin, Rect, RichText, Rounding, Sense, Stroke, TextFormat,
    TextureHandle, TextureOptions, Ui, Vec2,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use theme as t;
use tokio::sync::{oneshot, Notify};

const WHALE_PNG: &[u8] = include_bytes!("../../assets/deepseek_logo_wordmark.png");
const ICON_PNG: &[u8] = include_bytes!("../../assets/deepgravity_icon.png");
const COLUMN_WIDTH: f32 = 820.0;
const SIDEBAR_WIDTH: f32 = 280.0;

pub fn get_memory_usage_mb() -> f32 {
    if let Ok(content) = std::fs::read_to_string("/proc/self/statm") {
        let parts: Vec<&str> = content.split_whitespace().collect();
        if let Some(resident_pages) = parts.get(1).and_then(|p| p.parse::<usize>().ok()) {
            return (resident_pages * 4) as f32 / 1024.0;
        }
    }
    0.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTab {
    Chats,
    Projects,
    Settings,
}

#[derive(Debug, Clone)]
pub enum WorkerCommand {
    Prompt(String),
    Reset,
    SetModel(String),
    SetDir(String),
    LoadSession {
        messages: Vec<Message>,
        current_dir: String,
    },
}

pub struct PendingConfirmation {
    pub command: String,
    pub tx: oneshot::Sender<bool>,
}

pub struct DeepGravityApp {
    // Sessão ativa
    current_session_id: String,
    current_session_title: String,
    items: Vec<ChatItem>,
    messages: Vec<Message>,
    input_text: String,
    is_thinking: bool,
    auto_flag: Arc<AtomicBool>,
    selected_model: String,
    endpoint: String,
    current_dir: String,
    ram_mb: f32,
    last_ram_check: Instant,
    online: Arc<AtomicBool>,
    whale: TextureHandle,
    wordmark: TextureHandle,
    worker_tx: tokio::sync::mpsc::Sender<WorkerCommand>,
    event_rx: Receiver<AgentEvent>,
    confirm_rx: Receiver<(String, oneshot::Sender<bool>)>,
    pending_confirm: Option<PendingConfirmation>,
    want_focus: bool,

    // Histórico de Conversas e Projetos
    sidebar_tab: SidebarTab,
    sessions_meta: Vec<ConversationMeta>,
    project_store: ProjectStore,
    new_project_path: String,
    show_new_project_dialog: bool,
    project_path_error: Option<String>,
    filter_by_current_project: bool,
    sidebar_open: bool,

    // Parada de emergência + mídia
    cancel: Arc<Notify>,
    attachments: Vec<String>,
    thumbs: HashMap<String, Option<TextureHandle>>,
    recorder: Option<media::Recorder>,
    rec_started: Instant,
    busy_stt: usize,
    media_tx: Sender<MediaEvent>,
    media_rx: Receiver<MediaEvent>,
    status_note: Option<(String, Instant)>,
    token_input: String,
    token_status: Option<(String, bool)>,
}

/// Resultados vindos das threads de mídia (seletor de arquivos / transcrição).
enum MediaEvent {
    Picked(Vec<String>),
    Transcribed(Result<String, String>),
}

// ---------------------------------------------------------------------------
// Construção / workers
// ---------------------------------------------------------------------------

fn load_textures(ctx: &egui::Context) -> (TextureHandle, TextureHandle, egui::IconData) {
    let img = image::load_from_memory(WHALE_PNG)
        .expect("logo PNG inválido")
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    let word = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
    // A baleia ocupa as primeiras 225 colunas do wordmark; centraliza num canvas quadrado
    // para não cortar nem distorcer ao desenhar como ícone.
    let crop_w = 225u32.min(w);
    let whale_crop = image::imageops::crop_imm(&img, 0, 0, crop_w, h).to_image();
    let sq = crop_w.max(h);
    let mut canvas = image::RgbaImage::new(sq, sq);
    image::imageops::overlay(&mut canvas, &whale_crop, ((sq - crop_w) / 2) as i64, ((sq - h) / 2) as i64);
    let whale = egui::ColorImage::from_rgba_unmultiplied(
        [canvas.width() as usize, canvas.height() as usize],
        canvas.as_raw(),
    );

    // Ícone da janela/taskbar: usa a baleia oficial do avatar, já recortada e
    // centralizada num canvas quadrado com padding uniforme (assets/deepgravity_icon.png).
    // Isso evita o efeito "esticado/grande" causado pelo crop horizontal do wordmark.
    let icon = {
        let icon_img = image::load_from_memory(ICON_PNG)
            .expect("ícone PNG inválido")
            .to_rgba8();
        egui::IconData {
            rgba: icon_img.as_raw().clone(),
            width: icon_img.width(),
            height: icon_img.height(),
        }
    };

    (
        ctx.load_texture("whale", whale, TextureOptions::LINEAR),
        ctx.load_texture("wordmark", word, TextureOptions::LINEAR),
        icon,
    )
}

fn spawn_health_check(endpoint: &str, flag: Arc<AtomicBool>) {
    let hostport = endpoint
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("127.0.0.1:8000")
        .to_string();
    std::thread::spawn(move || {
        use std::net::{TcpStream, ToSocketAddrs};
        loop {
            let ok = hostport
                .to_socket_addrs()
                .ok()
                .and_then(|mut a| a.next())
                .map(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(800)).is_ok())
                .unwrap_or(false);
            flag.store(ok, Ordering::Relaxed);
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}

impl DeepGravityApp {
    pub fn new(
        ctx: &egui::Context,
        endpoint: String,
        model: String,
        default_dir: String,
        auto_approve: bool,
    ) -> Self {
        let (whale, wordmark, _) = load_textures(ctx);

        let (worker_tx, mut worker_rx) = tokio::sync::mpsc::channel::<WorkerCommand>(100);

        let (event_sync_tx, event_rx) = channel::<AgentEvent>();
        let (confirm_sync_tx, confirm_rx) = channel::<(String, oneshot::Sender<bool>)>();

        let (gui_event_tx, mut gui_event_rx) = tokio::sync::mpsc::channel::<AgentEvent>(100);
        let (gui_confirm_tx, mut gui_confirm_rx) =
            tokio::sync::mpsc::channel::<(String, oneshot::Sender<bool>)>(10);

        let auto_flag = Arc::new(AtomicBool::new(auto_approve));
        let online = Arc::new(AtomicBool::new(false));
        spawn_health_check(&endpoint, online.clone());

        // Inicializar armazenamento de projetos
        let project_store = storage::load_projects(&default_dir);
        let current_dir = project_store.current.clone();

        // Ponte Tokio -> std::mpsc, acordando a GUI a cada evento.
        let repaint_ctx = ctx.clone();
        let repaint_ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to build tokio runtime");
            rt.block_on(async move {
                loop {
                    tokio::select! {
                        Some(ev) = gui_event_rx.recv() => {
                            let _ = event_sync_tx.send(ev);
                            repaint_ctx.request_repaint();
                        }
                        Some(conf) = gui_confirm_rx.recv() => {
                            let _ = confirm_sync_tx.send(conf);
                            repaint_ctx2.request_repaint();
                        }
                        else => break,
                    }
                }
            });
        });

        // Worker assíncrono do Agente.
        let ep = endpoint.clone();
        let m = model.clone();
        let dir = current_dir.clone();
        let flag = auto_flag.clone();
        let cancel = Arc::new(Notify::new());
        let cancel_w = cancel.clone();
        let (media_tx, media_rx) = channel::<MediaEvent>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to build agent runtime");
            rt.block_on(async move {
                let client = DeepSeekClient::new(&ep, &m);
                let mut agent = Agent::new(client, dir, auto_approve);
                agent.event_tx = Some(gui_event_tx);
                agent.confirm_tx = Some(gui_confirm_tx);
                agent.auto_flag = Some(flag);

                while let Some(cmd) = worker_rx.recv().await {
                    match cmd {
                        WorkerCommand::Prompt(prompt) => {
                            // Descarta um sinal de "parar" antigo que tenha sobrado.
                            let _ = tokio::time::timeout(Duration::ZERO, cancel_w.notified()).await;
                            let snapshot = agent.messages.len();
                            let cancelled = {
                                let fut = agent.process_user_input(&prompt);
                                tokio::pin!(fut);
                                tokio::select! {
                                    _ = &mut fut => false,
                                    _ = cancel_w.notified() => true,
                                }
                            };
                            if cancelled {
                                // Mantém o histórico consistente (sem tool_calls órfãs).
                                agent.messages.truncate(snapshot);
                                agent.messages.push(Message {
                                    role: "user".to_string(),
                                    content: Some(prompt.clone()),
                                    tool_calls: None,
                                    tool_call_id: None,
                                    name: None,
                                });
                                agent.messages.push(Message {
                                    role: "assistant".to_string(),
                                    content: Some("(Execução interrompida pelo usuário.)".to_string()),
                                    tool_calls: None,
                                    tool_call_id: None,
                                    name: None,
                                });
                                if let Some(tx) = &agent.event_tx {
                                    let _ = tx.send(AgentEvent::Interrupted).await;
                                    let _ = tx.send(AgentEvent::TurnFinished(agent.messages.clone())).await;
                                }
                            }
                            if let Some(tx) = &agent.event_tx {
                                let _ = tx.send(AgentEvent::Thinking(false)).await;
                            }
                        }
                        WorkerCommand::Reset => agent.reset(),
                        WorkerCommand::SetModel(new_model) => agent.client.model = new_model,
                        WorkerCommand::SetDir(new_dir) => {
                            agent.current_dir = new_dir;
                            agent.reset();
                        }
                        WorkerCommand::LoadSession { messages, current_dir } => {
                            agent.current_dir = current_dir;
                            agent.messages = messages;
                        }
                    }
                }
            });
        });

        let sessions_meta = storage::list_sessions().unwrap_or_default();
        let current_session_id = storage::generate_session_id();

        Self {
            current_session_id,
            current_session_title: "Nova conversa".to_string(),
            items: Vec::new(),
            messages: Vec::new(),
            input_text: String::new(),
            is_thinking: false,
            auto_flag,
            selected_model: model,
            endpoint,
            current_dir,
            ram_mb: get_memory_usage_mb(),
            last_ram_check: Instant::now(),
            online,
            whale,
            wordmark,
            worker_tx,
            event_rx,
            confirm_rx,
            pending_confirm: None,
            want_focus: true,

            sidebar_tab: SidebarTab::Chats,
            sessions_meta,
            project_store,
            new_project_path: String::new(),
            show_new_project_dialog: false,
            project_path_error: None,
            filter_by_current_project: false,
            sidebar_open: true,

            cancel,
            attachments: Vec::new(),
            thumbs: HashMap::new(),
            recorder: None,
            rec_started: Instant::now(),
            busy_stt: 0,
            media_tx,
            media_rx,
            status_note: None,
            token_input: String::new(),
            token_status: None,
        }
    }

    fn check_events(&mut self) {
        if self.last_ram_check.elapsed().as_secs() >= 1 {
            self.ram_mb = get_memory_usage_mb();
            self.last_ram_check = Instant::now();
        }

        while let Ok(conf) = self.confirm_rx.try_recv() {
            if self.auto_flag.load(Ordering::Relaxed) {
                let _ = conf.1.send(true);
            } else {
                self.pending_confirm = Some(PendingConfirmation { command: conf.0, tx: conf.1 });
            }
        }

        if self.auto_flag.load(Ordering::Relaxed) {
            if let Some(p) = self.pending_confirm.take() {
                let _ = p.tx.send(true);
            }
        }

        while let Ok(ev) = self.event_rx.try_recv() {
            match ev {
                AgentEvent::Thinking(th) => self.is_thinking = th,
                AgentEvent::ToolStart { name, args } => {
                    self.items.push(ChatItem::Tool {
                        name,
                        args,
                        result: None,
                        success: true,
                        expanded: false,
                    });
                    self.save_current_session();
                }
                AgentEvent::ToolResult { name, result, success } => {
                    if let Some(ChatItem::Tool { result: r, success: s, .. }) =
                        self.items.iter_mut().rev().find(|item| {
                            matches!(item, ChatItem::Tool { name: n, result: None, .. } if n == &name)
                        })
                    {
                        *r = Some(result);
                        *s = success;
                    }
                    self.save_current_session();
                }
                AgentEvent::AssistantResponse(text) => {
                    self.items.push(ChatItem::Assistant(text));
                    self.is_thinking = false;
                    self.save_current_session();
                    self.refresh_sessions_meta();
                }
                AgentEvent::TurnFinished(msgs) => {
                    self.messages = msgs;
                    self.save_current_session();
                    self.refresh_sessions_meta();
                }
                AgentEvent::Error(err) => {
                    self.items.push(ChatItem::Error(err));
                    self.is_thinking = false;
                    self.save_current_session();
                    self.refresh_sessions_meta();
                }
                AgentEvent::Interrupted => {
                    self.items.push(ChatItem::Notice("Execução interrompida pelo usuário.".to_string()));
                    self.is_thinking = false;
                    self.pending_confirm = None;
                    self.save_current_session();
                    self.refresh_sessions_meta();
                }
            }
        }

        // Eventos de mídia (seletor de arquivos / transcrição de áudio)
        while let Ok(ev) = self.media_rx.try_recv() {
            match ev {
                MediaEvent::Picked(paths) => {
                    for p in paths {
                        self.add_media(p);
                    }
                }
                MediaEvent::Transcribed(res) => {
                    self.busy_stt = self.busy_stt.saturating_sub(1);
                    match res {
                        Ok(text) => {
                            if !self.input_text.trim().is_empty() && !self.input_text.ends_with(' ') && !self.input_text.ends_with('\n') {
                                self.input_text.push(' ');
                            }
                            self.input_text.push_str(&text);
                            self.want_focus = true;
                        }
                        Err(e) => self.note(e),
                    }
                }
            }
        }
        if let Some((_, at)) = &self.status_note {
            if at.elapsed().as_secs() >= 6 {
                self.status_note = None;
            }
        }
    }

    fn note(&mut self, msg: impl Into<String>) {
        self.status_note = Some((msg.into(), Instant::now()));
    }

    /// Botão "Parar": interrompe a requisição e qualquer comando em andamento.
    fn stop_agent(&mut self) {
        if self.is_thinking {
            self.cancel.notify_one();
            self.pending_confirm = None; // descarta o pedido de confirmação (o sender é fechado)
        }
    }

    fn open_picker(&mut self) {
        let tx = self.media_tx.clone();
        std::thread::spawn(move || {
            let files = media::pick_files();
            let _ = tx.send(MediaEvent::Picked(files));
        });
    }

    /// Áudio vira texto transcrito; imagens/documentos viram anexos da mensagem.
    fn add_media(&mut self, path: String) {
        if media::is_audio_path(&path) {
            self.transcribe_async(std::path::PathBuf::from(path));
        } else if !self.attachments.contains(&path) {
            self.attachments.push(path);
        }
    }

    fn transcribe_async(&mut self, path: std::path::PathBuf) {
        self.busy_stt += 1;
        let tx = self.media_tx.clone();
        std::thread::spawn(move || {
            let res = media::transcribe(&path);
            let _ = tx.send(MediaEvent::Transcribed(res));
        });
    }

    fn toggle_recording(&mut self) {
        if let Some(rec) = self.recorder.take() {
            let wav = rec.stop();
            self.transcribe_async(wav);
        } else {
            match media::Recorder::start() {
                Ok(r) => {
                    self.recorder = Some(r);
                    self.rec_started = Instant::now();
                }
                Err(e) => self.note(e),
            }
        }
    }

    fn save_current_session(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let session = ConversationSession {
            id: self.current_session_id.clone(),
            title: self.current_session_title.clone(),
            project_path: self.current_dir.clone(),
            model: self.selected_model.clone(),
            created_at: storage::now_timestamp(),
            updated_at: storage::now_timestamp(),
            items: self.items.clone(),
            messages: self.messages.clone(),
        };
        let _ = storage::save_session(&session);
    }

    fn refresh_sessions_meta(&mut self) {
        if let Ok(list) = storage::list_sessions() {
            self.sessions_meta = list;
        }
    }

    fn switch_session(&mut self, session_id: &str) {
        if session_id == self.current_session_id {
            return;
        }
        self.save_current_session();
        if let Ok(sess) = storage::load_session(session_id) {
            self.current_session_id = sess.id;
            self.current_session_title = sess.title;
            self.current_dir = sess.project_path.clone();
            self.selected_model = sess.model.clone();
            self.items = sess.items;
            self.messages = sess.messages.clone();
            self.pending_confirm = None;
            let _ = self.worker_tx.try_send(WorkerCommand::LoadSession {
                messages: self.messages.clone(),
                current_dir: sess.project_path,
            });
            let _ = self.worker_tx.try_send(WorkerCommand::SetModel(self.selected_model.clone()));
        }
        self.refresh_sessions_meta();
        self.want_focus = true;
    }

    fn delete_session(&mut self, session_id: &str) {
        let _ = storage::delete_session(session_id);
        if self.current_session_id == session_id {
            self.items.clear();
            self.messages.clear();
            self.current_session_id = storage::generate_session_id();
            self.current_session_title = "Nova conversa".to_string();
            let _ = self.worker_tx.try_send(WorkerCommand::Reset);
        }
        self.refresh_sessions_meta();
    }

    fn switch_project(&mut self, new_path: String) {
        let p = std::path::Path::new(&new_path);
        if !p.is_dir() {
            self.project_path_error = Some("Diretório não existe".to_string());
            return;
        }
        let canonical = p
            .canonicalize()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(new_path);

        self.current_dir = canonical.clone();
        let _ = storage::add_recent_project(&mut self.project_store, &canonical);
        let _ = self.worker_tx.try_send(WorkerCommand::SetDir(canonical));
        self.project_path_error = None;
        self.show_new_project_dialog = false;
        self.new_chat();
    }

    fn send_text(&mut self, msg: String) {
        let msg = msg.trim().to_string();
        if (msg.is_empty() && self.attachments.is_empty()) || self.is_thinking {
            return;
        }
        if self.items.is_empty() {
            let title_src = if msg.is_empty() { "Anexo".to_string() } else { msg.clone() };
            self.current_session_title = storage::title_from_prompt(&title_src);
        }
        let mut msg = msg;
        for p in self.attachments.drain(..) {
            msg.push('\n');
            msg.push_str(&client::attach_marker(&p));
        }
        let msg = msg.trim().to_string();
        self.items.push(ChatItem::User(msg.clone()));
        self.input_text.clear();
        self.is_thinking = true;
        self.save_current_session();
        self.refresh_sessions_meta();
        let _ = self.worker_tx.try_send(WorkerCommand::Prompt(msg));
    }

    fn new_chat(&mut self) {
        self.save_current_session();
        self.refresh_sessions_meta();

        self.current_session_id = storage::generate_session_id();
        self.current_session_title = "Nova conversa".to_string();
        self.items.clear();
        self.messages.clear();
        self.pending_confirm = None;
        let _ = self.worker_tx.try_send(WorkerCommand::Reset);
        self.want_focus = true;
    }
}

// ---------------------------------------------------------------------------
// Widgets reutilizáveis
// ---------------------------------------------------------------------------

fn bold(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).family(t::bold_family())
}

fn toggle_switch(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let size = Vec2::new(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let how_on = ui.ctx().animate_bool_with_time(resp.id, *on, 0.15);
    let bg = lerp_color(t::SURFACE_HI, t::ACCENT, how_on);
    ui.painter().rect_filled(rect, Rounding::same(11.0), bg);
    ui.painter().rect_stroke(
        rect,
        Rounding::same(11.0),
        Stroke::new(1.0_f32, if *on { t::ACCENT_HI } else { t::BORDER }),
    );
    let knob_x = rect.min.x + 11.0 + how_on * (rect.width() - 22.0);
    ui.painter().circle_filled(
        egui::pos2(knob_x, rect.center().y),
        8.0,
        Color32::WHITE,
    );
    resp
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    Color32::from_rgba_premultiplied(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t) as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t) as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t) as u8,
        (a.a() as f32 + (b.a() as f32 - a.a() as f32) * t) as u8,
    )
}

fn choice_card(
    ui: &mut Ui,
    width: f32,
    height: f32,
    selected: bool,
    title: &str,
    sub: &str,
) -> egui::Response {
    let size = Vec2::new(width, height);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let bg = if selected {
        t::ACCENT_SOFT
    } else if resp.hovered() {
        t::SURFACE_HI
    } else {
        t::SURFACE
    };
    ui.painter().rect_filled(rect, Rounding::same(12.0), bg);
    let stroke_col = if selected {
        t::ACCENT
    } else if resp.hovered() {
        t::BORDER.linear_multiply(1.6)
    } else {
        t::BORDER
    };
    ui.painter().rect_stroke(rect, Rounding::same(12.0), Stroke::new(1.0_f32, stroke_col));

    let pad = 12.0;
    let title_pos = egui::pos2(rect.min.x + pad, rect.min.y + 10.0);
    let sub_pos = egui::pos2(rect.min.x + pad, rect.min.y + 28.0);
    ui.painter().text(
        title_pos,
        egui::Align2::LEFT_TOP,
        title,
        FontId::new(14.0, t::bold_family()),
        if selected { Color32::WHITE } else { t::TEXT },
    );
    ui.painter().text(
        sub_pos,
        egui::Align2::LEFT_TOP,
        sub,
        FontId::new(11.5, FontFamily::Proportional),
        t::MUTED,
    );
    resp
}

fn draw_chevron(ui: &mut Ui, rect: Rect, open: bool, color: Color32) {
    let c = rect.center();
    let st = Stroke::new(1.6_f32, color);
    let pts = if open {
        [egui::pos2(c.x - 4.0, c.y - 2.0), egui::pos2(c.x, c.y + 2.0), egui::pos2(c.x + 4.0, c.y - 2.0)]
    } else {
        [egui::pos2(c.x - 2.0, c.y - 4.0), egui::pos2(c.x + 2.0, c.y), egui::pos2(c.x - 2.0, c.y + 4.0)]
    };
    ui.painter().line_segment([pts[0], pts[1]], st);
    ui.painter().line_segment([pts[1], pts[2]], st);
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).size(11.0).color(t::FAINT).family(t::bold_family()));
}

fn pill(ui: &mut Ui, color: Color32, text: &str) {
    Frame::none()
        .fill(color.linear_multiply(0.14))
        .rounding(Rounding::same(20.0))
        .inner_margin(Margin::symmetric(9.0, 3.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).color(color).family(t::bold_family()));
        });
}

/// Botão circular com fundo que clareia ao passar o mouse; o ícone é desenhado pelo chamador.
fn icon_button(ui: &mut Ui, size: f32, fill: Color32) -> (Rect, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let fill = if resp.hovered() { lerp_color(fill, Color32::WHITE, 0.12) } else { fill };
    ui.painter().circle_filled(rect.center(), size / 2.0, fill);
    (rect, resp)
}

/// Carrega (e guarda em cache) a miniatura de uma imagem do disco.
fn load_thumb(
    ctx: &egui::Context,
    cache: &mut HashMap<String, Option<TextureHandle>>,
    path: &str,
    max_px: u32,
) -> Option<TextureHandle> {
    if let Some(entry) = cache.get(path) {
        return entry.clone();
    }
    let tex = (|| {
        let img = image::open(path).ok()?;
        let thumb = img.thumbnail(max_px, max_px).to_rgba8();
        let ci = egui::ColorImage::from_rgba_unmultiplied(
            [thumb.width() as usize, thumb.height() as usize],
            thumb.as_raw(),
        );
        Some(ctx.load_texture(format!("thumb:{}", path), ci, TextureOptions::LINEAR))
    })();
    cache.insert(path.to_string(), tex.clone());
    tex
}

fn avatar(ui: &mut Ui, whale: &TextureHandle) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 15.0, t::ACCENT_SOFT);
    ui.painter()
        .circle_stroke(rect.center(), 15.0, Stroke::new(1.0_f32, t::ACCENT.linear_multiply(0.4)));
    let inner = Rect::from_center_size(rect.center(), Vec2::splat(19.0));
    ui.put(inner, egui::Image::new((whale.id(), Vec2::splat(19.0))));
}

// ---------------------------------------------------------------------------
// Markdown leve
// ---------------------------------------------------------------------------

fn inline_job(text: &str, size: f32, color: Color32, max_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap = egui::text::TextWrapping {
        max_width: max_width.max(60.0),
        ..Default::default()
    };
    let normal = TextFormat {
        font_id: FontId::new(size, FontFamily::Proportional),
        color,
        line_height: Some(size * 1.6),
        ..Default::default()
    };
    let bold_fmt = TextFormat {
        font_id: FontId::new(size, t::bold_family()),
        color: Color32::WHITE,
        line_height: Some(size * 1.6),
        ..Default::default()
    };
    let code_fmt = TextFormat {
        font_id: FontId::new(size - 1.0, FontFamily::Monospace),
        color: Color32::from_rgb(157, 180, 255),
        background: Color32::from_rgb(24, 30, 48),
        line_height: Some(size * 1.6),
        ..Default::default()
    };

    let mut rest = text;
    let mut is_bold = false;
    while !rest.is_empty() {
        let next_bold = rest.find("**");
        let next_code = rest.find('`');
        let (pos, kind) = match (next_bold, next_code) {
            (Some(b), Some(c)) if b < c => (b, 'b'),
            (Some(_), Some(c)) => (c, 'c'),
            (Some(b), None) => (b, 'b'),
            (None, Some(c)) => (c, 'c'),
            (None, None) => {
                job.append(rest, 0.0, if is_bold { bold_fmt.clone() } else { normal.clone() });
                break;
            }
        };

        if pos > 0 {
            job.append(&rest[..pos], 0.0, if is_bold { bold_fmt.clone() } else { normal.clone() });
        }

        if kind == 'b' {
            is_bold = !is_bold;
            rest = &rest[pos + 2..];
        } else {
            let after = &rest[pos + 1..];
            if let Some(end) = after.find('`') {
                job.append(&after[..end], 0.0, code_fmt.clone());
                rest = &after[end + 1..];
            } else {
                job.append("`", 0.0, normal.clone());
                rest = after;
            }
        }
    }

    job
}

fn render_table(ui: &mut Ui, lines: &[&str], wrap_w: f32) {
    if lines.is_empty() {
        return;
    }
    let rows: Vec<Vec<String>> = lines
        .iter()
        .filter_map(|l| {
            let trimmed = l.trim();
            if !trimmed.starts_with('|') {
                return None;
            }
            let cells: Vec<String> = trimmed
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect();
            // Ignora linhas separadoras como |---|---|
            if cells.iter().all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')) {
                None
            } else {
                Some(cells)
            }
        })
        .collect();

    if rows.is_empty() {
        return;
    }

    ui.add_space(4.0);
    Frame::none()
        .fill(t::SURFACE)
        .stroke(Stroke::new(1.0_f32, t::BORDER))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::same(8.0))
        .show(ui, |ui| {
            ui.set_max_width(wrap_w);
            egui::ScrollArea::horizontal().show(ui, |ui| {
                egui::Grid::new(ui.next_auto_id())
                    .striped(true)
                    .spacing(Vec2::new(16.0, 7.0))
                    .show(ui, |ui| {
                        for (r_idx, row) in rows.iter().enumerate() {
                            for cell in row {
                                if r_idx == 0 {
                                    let job = inline_job(cell, 13.0, Color32::WHITE, 300.0);
                                    ui.add(egui::Label::new(job).selectable(true));
                                } else {
                                    let job = inline_job(cell, 13.0, t::TEXT, 300.0);
                                    ui.add(egui::Label::new(job).selectable(true));
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        });
    ui.add_space(4.0);
}

fn render_markdown(ui: &mut Ui, md: &str, wrap_w: f32) {
    let mut in_code_block = false;
    let mut code_buf = String::new();
    let mut code_lang = String::new();
    let mut table_buf: Vec<&str> = Vec::new();

    let flush_table = |ui: &mut Ui, buf: &mut Vec<&str>| {
        if !buf.is_empty() {
            render_table(ui, buf, wrap_w);
            buf.clear();
        }
    };

    for line in md.lines() {
        if line.starts_with("```") {
            flush_table(ui, &mut table_buf);
            if in_code_block {
                let code_to_copy = code_buf.trim_end_matches('\n').to_string();
                let lang_display = if code_lang.is_empty() { "código" } else { &code_lang };
                Frame::none()
                    .fill(t::CODE_BG)
                    .stroke(Stroke::new(1.0_f32, t::BORDER))
                    .rounding(Rounding::same(10.0))
                    .inner_margin(Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.set_max_width(wrap_w);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(lang_display).size(11.0).color(t::FAINT).family(t::bold_family()));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .button(RichText::new("Copiar").size(11.5).color(t::ACCENT_HI))
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .clicked()
                                {
                                    ui.ctx().copy_text(code_to_copy.clone());
                                }
                            });
                        });
                        ui.add_space(4.0);
                        egui::ScrollArea::horizontal().show(ui, |ui| {
                            ui.label(
                                RichText::new(&code_to_copy)
                                    .font(FontId::new(13.0, FontFamily::Monospace))
                                    .color(Color32::from_rgb(220, 226, 240)),
                            );
                        });
                    });
                code_buf.clear();
                code_lang.clear();
                in_code_block = false;
            } else {
                in_code_block = true;
                code_lang = line.trim_start_matches("```").trim().to_string();
            }
            continue;
        }

        if in_code_block {
            code_buf.push_str(line);
            code_buf.push('\n');
            continue;
        }

        let trimmed = line.trim();
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            table_buf.push(line);
            continue;
        }
        flush_table(ui, &mut table_buf);

        if trimmed.is_empty() {
            ui.add_space(6.0);
            continue;
        }

        // Citações / Blockquotes
        if let Some(quote) = trimmed.strip_prefix("> ") {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(3.0, 20.0), Sense::hover());
                ui.painter().rect_filled(rect, Rounding::same(1.5), t::ACCENT);
                ui.add_space(6.0);
                let sub_w = (wrap_w - 18.0).max(60.0);
                ui.add(egui::Label::new(inline_job(quote, 14.0, t::MUTED, sub_w)).selectable(true));
            });
            continue;
        }

        // Cabeçalhos com suporte a formatação inline (negrito, código)
        if let Some(h) = trimmed.strip_prefix("### ") {
            ui.add_space(8.0);
            let job = inline_job(h, 16.0, Color32::WHITE, wrap_w);
            ui.add(egui::Label::new(job).selectable(true));
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("## ") {
            ui.add_space(10.0);
            let job = inline_job(h, 18.0, Color32::WHITE, wrap_w);
            ui.add(egui::Label::new(job).selectable(true));
            continue;
        }
        if let Some(h) = trimmed.strip_prefix("# ") {
            ui.add_space(12.0);
            let job = inline_job(h, 22.0, Color32::WHITE, wrap_w);
            ui.add(egui::Label::new(job).selectable(true));
            continue;
        }
        if trimmed.starts_with("---") || trimmed.starts_with("***") {
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            continue;
        }

        if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            ui.horizontal_top(|ui| {
                ui.label(RichText::new("•").size(16.0).color(t::ACCENT));
                let sub_w = (wrap_w - 24.0).max(80.0);
                ui.add(egui::Label::new(inline_job(item, 14.5, t::TEXT, sub_w)).selectable(true));
            });
            continue;
        }

        if trimmed.chars().next().map_or(false, |c| c.is_ascii_digit()) && trimmed.contains(". ") {
            if let Some(idx) = trimmed.find(". ") {
                let num = &trimmed[..=idx];
                let rest = &trimmed[idx + 2..];
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new(num).size(13.0).color(t::ACCENT).family(t::bold_family()));
                    let sub_w = (wrap_w - 28.0).max(80.0);
                    ui.add(egui::Label::new(inline_job(rest, 14.5, t::TEXT, sub_w)).selectable(true));
                });
                continue;
            }
        }

        ui.add(egui::Label::new(inline_job(line, 14.5, t::TEXT, wrap_w)).selectable(true));
    }

    flush_table(ui, &mut table_buf);

    if in_code_block && !code_buf.is_empty() {
        Frame::none()
            .fill(t::CODE_BG)
            .stroke(Stroke::new(1.0_f32, t::BORDER))
            .rounding(Rounding::same(10.0))
            .inner_margin(Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_max_width(wrap_w);
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.label(
                        RichText::new(&code_buf)
                            .font(FontId::new(13.0, FontFamily::Monospace))
                            .color(Color32::from_rgb(220, 226, 240)),
                    );
                });
            });
    }
}

// ---------------------------------------------------------------------------
// Card de execução de ferramenta
// ---------------------------------------------------------------------------

fn tool_card(
    ui: &mut Ui,
    name: &str,
    args: &str,
    result: &Option<String>,
    success: bool,
    expanded: &mut bool,
    time: f64,
    max_w: f32,
) {
    let is_running = result.is_none();
    let border_color = if is_running {
        t::ACCENT.linear_multiply(0.4)
    } else if success {
        t::BORDER
    } else {
        t::DANGER.linear_multiply(0.5)
    };

    Frame::none()
        .fill(t::SURFACE)
        .stroke(Stroke::new(1.0_f32, border_color))
        .rounding(Rounding::same(12.0))
        .inner_margin(Margin::symmetric(14.0, 10.0))
        .show(ui, |ui| {
            let inner_w = (max_w - 28.0).max(60.0);
            ui.set_width(inner_w);
            ui.set_max_width(inner_w);
            let header_resp = ui.horizontal(|ui| {
                let dot_col = if is_running {
                    let a = ((time * 5.0).sin() * 0.5 + 0.5) as f32;
                    lerp_color(t::ACCENT, t::ACCENT_HI, a)
                } else if success {
                    t::SUCCESS
                } else {
                    t::DANGER
                };
                let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, dot_col);

                ui.label(RichText::new("ferramenta").size(11.0).color(t::FAINT).family(t::bold_family()));
                ui.label(bold(name).size(13.0).color(Color32::WHITE));

                let badge_text = if is_running { "executando..." } else if success { "concluído" } else { "falhou" };
                let badge_color = if is_running { t::ACCENT } else if success { t::SUCCESS } else { t::DANGER };
                pill(ui, badge_color, badge_text);

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (chev, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                    draw_chevron(ui, chev, *expanded, t::MUTED);
                });
            });

            if header_resp.response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                *expanded = !*expanded;
            }

            if *expanded {
                ui.add_space(8.0);
                ui.label(RichText::new("Argumentos:").size(11.0).color(t::FAINT));
                Frame::none()
                    .fill(t::CODE_BG)
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.set_max_width((inner_w - 16.0).max(60.0));
                        egui::ScrollArea::horizontal().show(ui, |ui| {
                            ui.label(
                                RichText::new(args)
                                    .font(FontId::new(12.0, FontFamily::Monospace))
                                    .color(Color32::from_rgb(180, 192, 215)),
                            );
                        });
                    });

                if let Some(res) = result {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Resultado:").size(11.0).color(t::FAINT));
                    Frame::none()
                        .fill(t::CODE_BG)
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.set_max_width((inner_w - 16.0).max(60.0));
                            let show = if res.chars().count() > 1800 {
                                let head: String = res.chars().take(1800).collect();
                                format!("{}...\n[truncado, {} bytes]", head, res.len())
                            } else {
                                res.clone()
                            };
                            egui::ScrollArea::horizontal().show(ui, |ui| {
                                ui.label(
                                    RichText::new(show)
                                        .font(FontId::new(12.0, FontFamily::Monospace))
                                        .color(if success { Color32::from_rgb(200, 210, 230) } else { Color32::from_rgb(254, 180, 180) }),
                                );
                            });
                        });
                }
            }
        });
}

// ---------------------------------------------------------------------------
// Telas principais
// ---------------------------------------------------------------------------

impl DeepGravityApp {
    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sidebar")
            .exact_width(SIDEBAR_WIDTH)
            .resizable(false)
            .frame(
                Frame::none()
                    .fill(t::SIDEBAR)
                    .stroke(Stroke::new(1.0_f32, t::BORDER))
                    .inner_margin(Margin::symmetric(16.0, 18.0)),
            )
            .show(ctx, |ui| {
                // Logo oficial + botão recolher lateral
                ui.horizontal(|ui| {
                    let h = 26.0;
                    let w = h * (780.0 / 165.0);
                    ui.add(egui::Image::new((self.wordmark.id(), Vec2::new(w, h))));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let close_btn = egui::Button::new(RichText::new("◀").size(12.0).color(t::MUTED))
                            .fill(t::SURFACE)
                            .stroke(Stroke::new(1.0_f32, t::BORDER))
                            .rounding(Rounding::same(6.0))
                            .min_size(Vec2::new(24.0, 24.0));
                        if ui
                            .add(close_btn)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Esconder painel lateral")
                            .clicked()
                        {
                            self.sidebar_open = false;
                        }
                    });
                });
                ui.add_space(3.0);
                ui.label(RichText::new("Agente autônomo nativo").size(11.5).color(t::MUTED));
                ui.add_space(14.0);

                let btn = egui::Button::new(
                    RichText::new("+  Nova conversa").color(Color32::WHITE).family(t::bold_family()),
                )
                .fill(t::ACCENT)
                .stroke(Stroke::NONE)
                .rounding(Rounding::same(10.0))
                .min_size(Vec2::new(ui.available_width(), 38.0));
                if ui.add_enabled(!self.is_thinking, btn).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.new_chat();
                }

                ui.add_space(14.0);

                // Abas de navegação estilo Antigravity
                ui.horizontal(|ui| {
                    let total_w = ui.available_width();
                    let tab_w = (total_w - 8.0) / 3.0;
                    for (tab, label) in [
                        (SidebarTab::Chats, "💬 Chats"),
                        (SidebarTab::Projects, "📁 Projetos"),
                        (SidebarTab::Settings, "⚙ Ajustes"),
                    ] {
                        let active = self.sidebar_tab == tab;
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(tab_w, 28.0), Sense::click());
                        let bg = if active {
                            t::ACCENT_SOFT
                        } else if resp.hovered() {
                            t::SURFACE_HI
                        } else {
                            t::SURFACE
                        };
                        let text_color = if active {
                            t::ACCENT_HI
                        } else if resp.hovered() {
                            Color32::WHITE
                        } else {
                            t::MUTED
                        };
                        ui.painter().rect_filled(rect, Rounding::same(8.0), bg);
                        if active {
                            ui.painter().rect_stroke(
                                rect,
                                Rounding::same(8.0),
                                Stroke::new(1.0_f32, t::ACCENT.linear_multiply(0.4)),
                            );
                        }
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            label,
                            FontId::new(11.5, if active { t::bold_family() } else { FontFamily::Proportional }),
                            text_color,
                        );
                        if resp.clicked() {
                            self.sidebar_tab = tab;
                        }
                    }
                });

                ui.add_space(14.0);

                // Corpo da barra lateral baseado na aba ativa
                match self.sidebar_tab {
                    SidebarTab::Chats => {
                        ui.horizontal(|ui| {
                            section_label(ui, "Histórico");
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.selectable_label(self.filter_by_current_project, "Deste projeto").clicked() {
                                    self.filter_by_current_project = !self.filter_by_current_project;
                                }
                            });
                        });
                        ui.add_space(4.0);

                        let current_dir = self.current_dir.clone();
                        let filter_active = self.filter_by_current_project;
                        let filtered: Vec<_> = self
                            .sessions_meta
                            .iter()
                            .filter(|s| !filter_active || s.project_path == current_dir)
                            .cloned()
                            .collect();

                        if filtered.is_empty() {
                            ui.vertical_centered(|ui| {
                                ui.add_space(30.0);
                                ui.label(RichText::new("Nenhuma conversa ainda").size(12.5).color(t::FAINT));
                                ui.add_space(4.0);
                                ui.label(RichText::new("Suas conversas serão salvas automaticamente").size(11.0).color(t::MUTED));
                            });
                        } else {
                            let mut session_to_load = None;
                            let mut session_to_delete = None;

                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .max_height(ui.available_height() - 76.0)
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 6.0;
                                    let full_w = ui.available_width();
                                    for meta in &filtered {
                                        ui.push_id(&meta.id, |ui| {
                                            let is_active = meta.id == self.current_session_id;
                                            ui.horizontal(|ui| {
                                                ui.spacing_mut().item_spacing.x = 4.0;
                                                let card_w = full_w - 28.0;
                                                let (rect, resp) = ui.allocate_exact_size(Vec2::new(card_w, 48.0), Sense::click());
                                                let bg = if is_active {
                                                    t::ACCENT_SOFT
                                                } else if resp.hovered() {
                                                    t::SURFACE_HI
                                                } else {
                                                    t::SURFACE
                                                };
                                                ui.painter().rect_filled(rect, Rounding::same(10.0), bg);
                                                let border_col = if is_active {
                                                    t::ACCENT.linear_multiply(0.6)
                                                } else {
                                                    t::BORDER
                                                };
                                                ui.painter().rect_stroke(rect, Rounding::same(10.0), Stroke::new(1.0_f32, border_col));

                                                let title_text = if meta.title.chars().count() > 22 {
                                                    let s: String = meta.title.chars().take(20).collect();
                                                    format!("{}…", s)
                                                } else {
                                                    meta.title.clone()
                                                };
                                                let title_color = if is_active { Color32::WHITE } else { t::TEXT };
                                                ui.painter().text(
                                                    egui::pos2(rect.min.x + 10.0, rect.min.y + 7.0),
                                                    Align2::LEFT_TOP,
                                                    title_text,
                                                    FontId::new(13.0, t::bold_family()),
                                                    title_color,
                                                );

                                                let rel_time = storage::format_relative_time(meta.updated_at);
                                                let folder_name = std::path::Path::new(&meta.project_path)
                                                    .file_name()
                                                    .map(|f| f.to_string_lossy().to_string())
                                                    .unwrap_or_else(|| meta.project_path.clone());
                                                let sub_text = format!("{} · {}", rel_time, folder_name);
                                                ui.painter().text(
                                                    egui::pos2(rect.min.x + 10.0, rect.min.y + 26.0),
                                                    Align2::LEFT_TOP,
                                                    sub_text,
                                                    FontId::new(11.0, FontFamily::Proportional),
                                                    t::MUTED,
                                                );

                                                let (del_rect, del_resp) = ui.allocate_exact_size(Vec2::new(22.0, 48.0), Sense::click());
                                                let del_color = if del_resp.hovered() { t::DANGER } else { t::FAINT };
                                                ui.painter().text(
                                                    del_rect.center(),
                                                    Align2::CENTER_CENTER,
                                                    "×",
                                                    FontId::new(16.0, t::bold_family()),
                                                    del_color,
                                                );

                                                if del_resp.clicked() {
                                                    session_to_delete = Some(meta.id.clone());
                                                } else if resp.clicked() {
                                                    session_to_load = Some(meta.id.clone());
                                                }
                                            });
                                        });
                                    }
                                });

                            if let Some(id) = session_to_delete {
                                self.delete_session(&id);
                            } else if let Some(id) = session_to_load {
                                self.switch_session(&id);
                            }
                        }
                    }
                    SidebarTab::Projects => {
                        section_label(ui, "Pasta de trabalho ativa");
                        ui.add_space(4.0);
                        Frame::none()
                            .fill(t::ACCENT_SOFT)
                            .stroke(Stroke::new(1.0_f32, t::ACCENT.linear_multiply(0.4)))
                            .rounding(Rounding::same(10.0))
                            .inner_margin(Margin::symmetric(12.0, 10.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("📁").size(15.0));
                                    let folder_name = std::path::Path::new(&self.current_dir)
                                        .file_name()
                                        .map(|f| f.to_string_lossy().to_string())
                                        .unwrap_or_else(|| self.current_dir.clone());
                                    ui.vertical(|ui| {
                                        ui.label(bold(folder_name).size(13.5).color(Color32::WHITE));
                                        let path_short = if self.current_dir.chars().count() > 30 {
                                            let tail: String = self.current_dir.chars().rev().take(28).collect::<Vec<_>>().into_iter().rev().collect();
                                            format!("…{}", tail)
                                        } else {
                                            self.current_dir.clone()
                                        };
                                        ui.label(RichText::new(path_short).monospace().size(10.5).color(t::MUTED));
                                    });
                                });
                            });

                        ui.add_space(10.0);
                        if !self.show_new_project_dialog {
                            let btn = egui::Button::new(
                                RichText::new("+ Abrir outra pasta...").color(t::ACCENT_HI).family(t::bold_family()),
                            )
                            .fill(t::SURFACE)
                            .stroke(Stroke::new(1.0_f32, t::BORDER))
                            .rounding(Rounding::same(10.0))
                            .min_size(Vec2::new(ui.available_width(), 34.0));
                            if ui.add(btn).clicked() {
                                self.show_new_project_dialog = true;
                                self.new_project_path = self.current_dir.clone();
                                self.project_path_error = None;
                            }
                        } else {
                            Frame::none()
                                .fill(t::SURFACE)
                                .stroke(Stroke::new(1.0_f32, t::ACCENT.linear_multiply(0.5)))
                                .rounding(Rounding::same(10.0))
                                .inner_margin(Margin::same(10.0))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(bold("Caminho da pasta:").size(12.0).color(Color32::WHITE));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.new_project_path)
                                            .desired_width(ui.available_width())
                                            .hint_text("/caminho/para/projeto"),
                                    );
                                    if let Some(err) = &self.project_path_error {
                                        ui.label(RichText::new(err).size(11.0).color(t::DANGER));
                                    }
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        if ui.button(RichText::new("Abrir").family(t::bold_family())).clicked() {
                                            let p = self.new_project_path.clone();
                                            self.switch_project(p);
                                        }
                                        if ui.button("Cancelar").clicked() {
                                            self.show_new_project_dialog = false;
                                            self.project_path_error = None;
                                        }
                                    });
                                });
                        }

                        ui.add_space(16.0);
                        section_label(ui, "Projetos recentes");
                        ui.add_space(4.0);

                        let mut project_to_switch = None;
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .max_height(ui.available_height() - 76.0)
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 6.0;
                                let full_w = ui.available_width();
                                for proj_path in &self.project_store.recent {
                                    ui.push_id(proj_path, |ui| {
                                        let is_current = proj_path == &self.current_dir;
                                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(full_w, 44.0), Sense::click());
                                        let bg = if is_current {
                                            t::SURFACE_HI
                                        } else if resp.hovered() {
                                            t::SURFACE_HI
                                        } else {
                                            t::SURFACE
                                        };
                                        ui.painter().rect_filled(rect, Rounding::same(10.0), bg);
                                        let border_col = if is_current { t::ACCENT.linear_multiply(0.4) } else { t::BORDER };
                                        ui.painter().rect_stroke(rect, Rounding::same(10.0), Stroke::new(1.0_f32, border_col));

                                        let folder_name = std::path::Path::new(proj_path)
                                            .file_name()
                                            .map(|f| f.to_string_lossy().to_string())
                                            .unwrap_or_else(|| proj_path.clone());

                                        let text_pos = egui::pos2(rect.min.x + 10.0, rect.min.y + 5.0);
                                        ui.painter().text(
                                            text_pos,
                                            Align2::LEFT_TOP,
                                            folder_name,
                                            FontId::new(12.5, t::bold_family()),
                                            if is_current { t::ACCENT_HI } else { Color32::WHITE },
                                        );

                                        let path_short = if proj_path.chars().count() > 30 {
                                            let tail: String = proj_path.chars().rev().take(28).collect::<Vec<_>>().into_iter().rev().collect();
                                            format!("…{}", tail)
                                        } else {
                                            proj_path.clone()
                                        };
                                        ui.painter().text(
                                            egui::pos2(text_pos.x, text_pos.y + 18.0),
                                            Align2::LEFT_TOP,
                                            path_short,
                                            FontId::new(10.0, FontFamily::Monospace),
                                            t::MUTED,
                                        );

                                        if is_current {
                                            let badge_pos = egui::pos2(rect.max.x - 14.0, rect.min.y + 14.0);
                                            ui.painter().circle_filled(badge_pos, 4.0, t::SUCCESS);
                                        }

                                        if resp.clicked() && !is_current {
                                            project_to_switch = Some(proj_path.clone());
                                        }
                                    });
                                }
                            });

                        if let Some(p) = project_to_switch {
                            self.switch_project(p);
                        }
                    }
                    SidebarTab::Settings => {
                        section_label(ui, "Modelo");
                        ui.add_space(2.0);
                        let w = ui.available_width();
                        for (id, title, sub) in [
                            ("deepseek-web", "DeepSeek Chat", "Respostas rápidas"),
                            ("deepseek-web-reasoner", "DeepSeek R1", "Raciocínio profundo"),
                        ] {
                            let sel = self.selected_model == id;
                            if choice_card(ui, w, 52.0, sel, title, sub).clicked() && !sel {
                                self.selected_model = id.to_string();
                                let _ = self.worker_tx.try_send(WorkerCommand::SetModel(id.to_string()));
                            }
                        }

                        ui.add_space(18.0);
                        section_label(ui, "Autonomia");
                        ui.add_space(2.0);
                        Frame::none()
                            .fill(t::SURFACE)
                            .stroke(Stroke::new(1.0_f32, t::BORDER))
                            .rounding(Rounding::same(12.0))
                            .inner_margin(Margin::symmetric(14.0, 10.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(bold("Auto-aprovar").size(13.5).color(Color32::WHITE));
                                        ui.label(RichText::new("Executa sem perguntar").size(11.0).color(t::MUTED));
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let mut on = self.auto_flag.load(Ordering::Relaxed);
                                        if toggle_switch(ui, &mut on).changed() {
                                            self.auto_flag.store(on, Ordering::Relaxed);
                                        }
                                    });
                                });
                            });

                        ui.add_space(18.0);
                        section_label(ui, "Autenticação DeepSeek");
                        ui.add_space(2.0);
                        Frame::none()
                            .fill(t::SURFACE)
                            .stroke(Stroke::new(1.0_f32, t::BORDER))
                            .rounding(Rounding::same(12.0))
                            .inner_margin(Margin::same(12.0))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(bold("Token da Conta Web").size(13.0).color(Color32::WHITE));
                                ui.add_space(2.0);
                                ui.label(
                                    RichText::new("No chat.deepseek.com: abra DevTools (F12) > Console e cole:\nJSON.parse(localStorage.getItem('userToken')).value")
                                        .size(10.5)
                                        .color(t::MUTED),
                                );
                                ui.add_space(6.0);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.token_input)
                                        .hint_text("Cole seu token aqui...")
                                        .desired_width(ui.available_width())
                                );
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    let btn = egui::Button::new(
                                        RichText::new("Salvar Token").family(t::bold_family()).size(12.0).color(Color32::WHITE),
                                    )
                                    .fill(t::ACCENT)
                                    .rounding(Rounding::same(6.0));
                                    if ui.add(btn).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                        let tok = self.token_input.trim().to_string();
                                        if !tok.is_empty() {
                                            let python_bin = "/home/olympio/deepseek-web-universal-api/.venv/bin/python";
                                            let py_code = format!(
                                                "import sys, json; sys.path.insert(0, '/home/olympio/deepseek-web-universal-api'); from credential_store import save_credentials; t = {:?}.strip(); t = json.loads(t).get('value', t) if (t.startswith('{{') and 'value' in t) else t; t = str(t).strip().strip('\"\\''); ok = save_credentials(t); sys.exit(0 if ok else 1)",
                                                tok
                                            );
                                            match std::process::Command::new(python_bin)
                                                .arg("-c")
                                                .arg(&py_code)
                                                .output()
                                            {
                                                Ok(o) if o.status.success() => {
                                                    self.token_status = Some(("✓ Token atualizado com sucesso!".to_string(), true));
                                                    self.token_input.clear();
                                                }
                                                Ok(o) => {
                                                    let err = String::from_utf8_lossy(&o.stderr);
                                                    self.token_status = Some((format!("Erro ao salvar: {}", err.trim()), false));
                                                }
                                                Err(e) => {
                                                    self.token_status = Some((format!("Erro ao executar: {}", e), false));
                                                }
                                            }
                                        }
                                    }
                                });
                                if let Some((msg, ok)) = &self.token_status {
                                    ui.add_space(4.0);
                                    ui.label(RichText::new(msg).size(11.0).color(if *ok { t::SUCCESS } else { t::DANGER }));
                                }
                            });

                        ui.add_space(18.0);
                        section_label(ui, "Armazenamento");
                        ui.add_space(2.0);
                        let data_dir = storage::get_data_dir().to_string_lossy().to_string();
                        ui.label(RichText::new(format!("{} conversas gravadas", self.sessions_meta.len())).size(12.0).color(t::TEXT));
                        ui.label(RichText::new(data_dir).monospace().size(10.5).color(t::MUTED));
                    }
                }

                // Rodapé sempre fixo no fundo da barra lateral
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.horizontal(|ui| {
                        pill(ui, t::SUCCESS, &format!("RAM {:.0} MB", self.ram_mb));
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let online = self.online.load(Ordering::Relaxed);
                        let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        ui.painter()
                            .circle_filled(dot.center(), 4.0, if online { t::SUCCESS } else { t::DANGER });
                        let host = self
                            .endpoint
                            .trim_start_matches("http://")
                            .trim_end_matches("/v1");
                        ui.label(
                            RichText::new(if online { format!("API conectada · {}", host) } else { format!("API offline · {}", host) })
                                .size(11.5)
                                .color(t::MUTED),
                        );
                    });
                    ui.add_space(6.0);
                    ui.separator();
                });
            });
    }

    fn input_panel(&mut self, ctx: &egui::Context, pad: f32) {
        egui::TopBottomPanel::bottom("input_panel")
            .show_separator_line(false)
            .frame(Frame::none().fill(t::BG).inner_margin(Margin {
                left: pad,
                right: pad,
                top: 8.0,
                bottom: 14.0,
            }))
            .show(ctx, |ui| {
                // Confirmação de comando
                let mut decision: Option<(bool, bool)> = None;
                if let Some(p) = &self.pending_confirm {
                    Frame::none()
                        .fill(Color32::from_rgb(34, 28, 14))
                        .stroke(Stroke::new(1.0_f32, t::WARN.linear_multiply(0.6)))
                        .rounding(Rounding::same(14.0))
                        .inner_margin(Margin::same(14.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(bold("O DeepGravity quer executar um comando").size(14.0).color(t::WARN));
                            ui.add_space(2.0);
                            Frame::none()
                                .fill(t::CODE_BG)
                                .rounding(Rounding::same(8.0))
                                .inner_margin(Margin::symmetric(10.0, 8.0))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        RichText::new(&p.command)
                                            .font(FontId::new(12.5, FontFamily::Monospace))
                                            .color(Color32::from_rgb(255, 230, 180)),
                                    );
                                });
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                let allow_btn = egui::Button::new(
                                    RichText::new("Permitir uma vez").family(t::bold_family()).color(Color32::WHITE),
                                )
                                .fill(t::ACCENT)
                                .rounding(Rounding::same(8.0));
                                if ui.add(allow_btn).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    decision = Some((true, false));
                                }

                                let always_btn = egui::Button::new(
                                    RichText::new("Sempre permitir").color(t::ACCENT_HI),
                                )
                                .fill(t::SURFACE_HI)
                                .rounding(Rounding::same(8.0));
                                if ui.add(always_btn).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    decision = Some((true, true));
                                }

                                let deny_btn = egui::Button::new(
                                    RichText::new("Negar").color(t::DANGER),
                                )
                                .fill(Color32::from_rgb(40, 20, 24))
                                .rounding(Rounding::same(8.0));
                                if ui.add(deny_btn).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    decision = Some((false, false));
                                }
                            });
                        });
                    ui.add_space(10.0);
                }

                if let Some((allow, always)) = decision {
                    if always {
                        self.auto_flag.store(true, Ordering::Relaxed);
                    }
                    if let Some(p) = self.pending_confirm.take() {
                        let _ = p.tx.send(allow);
                    }
                }

                // Chips de anexos / estado de gravação / avisos
                let mut remove_idx: Option<usize> = None;
                let recording = self.recorder.is_some();
                let has_note = self.status_note.is_some();
                if !self.attachments.is_empty() || recording || self.busy_stt > 0 || has_note {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                        for (i, p) in self.attachments.iter().enumerate() {
                            let is_img = client::is_image_path(p);
                            let tex = if is_img { load_thumb(ui.ctx(), &mut self.thumbs, p, 96) } else { None };
                            Frame::none()
                                .fill(t::SURFACE)
                                .stroke(Stroke::new(1.0_f32, t::BORDER))
                                .rounding(Rounding::same(10.0))
                                .inner_margin(Margin::same(5.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        if let Some(tex) = &tex {
                                            ui.add(
                                                egui::Image::new((tex.id(), Vec2::splat(38.0)))
                                                    .rounding(Rounding::same(6.0)),
                                            );
                                        } else {
                                            ui.label(RichText::new(if is_img { "🖼" } else { "📎" }).size(18.0));
                                        }
                                        let name = std::path::Path::new(p)
                                            .file_name()
                                            .map(|f| f.to_string_lossy().to_string())
                                            .unwrap_or_else(|| p.clone());
                                        let short: String = if name.chars().count() > 26 {
                                            format!("{}…", name.chars().take(25).collect::<String>())
                                        } else {
                                            name
                                        };
                                        ui.label(RichText::new(short).size(12.0).color(t::TEXT));
                                        if ui
                                            .add(egui::Button::new(RichText::new("✕").size(11.0).color(t::MUTED)).frame(false))
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .on_hover_text("Remover anexo")
                                            .clicked()
                                        {
                                            remove_idx = Some(i);
                                        }
                                    });
                                });
                        }
                        if recording {
                            let secs = self.rec_started.elapsed().as_secs();
                            let blink = ((ui.input(|i| i.time) * 3.0).sin() * 0.5 + 0.5) as f32;
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 4.5, t::DANGER.linear_multiply(0.4 + 0.6 * blink));
                            ui.label(
                                RichText::new(format!("Gravando {}:{:02} — clique no microfone para parar", secs / 60, secs % 60))
                                    .size(12.5)
                                    .color(t::DANGER),
                            );
                        }
                        if self.busy_stt > 0 {
                            ui.add(egui::Spinner::new().size(14.0).color(t::ACCENT_HI));
                            ui.label(RichText::new("Transcrevendo áudio…").size(12.5).color(t::ACCENT_HI));
                        }
                        if let Some((msg, _)) = &self.status_note {
                            ui.label(RichText::new(msg).size(12.5).color(t::WARN));
                        }
                    });
                    ui.add_space(6.0);
                }
                if let Some(i) = remove_idx {
                    self.attachments.remove(i);
                }

                // Pílula de digitação
                let mut do_send = false;
                let mut do_stop = false;
                let mut do_attach = false;
                let mut do_mic = false;
                let stroke_color = if self.is_thinking {
                    t::BORDER
                } else {
                    t::ACCENT.linear_multiply(0.4)
                };

                let input_id = egui::Id::new("deepgravity_input");
                // Enter envia; Shift+Enter quebra linha. Remove o Enter antes do TextEdit vê-lo.
                if ui.memory(|m| m.has_focus(input_id)) {
                    let plain_enter = ui.input(|i| {
                        i.key_pressed(Key::Enter) && !i.modifiers.shift && !i.modifiers.alt && !i.modifiers.command
                    });
                    if plain_enter {
                        ui.input_mut(|i| {
                            i.events.retain(|e| !matches!(e, egui::Event::Key { key: Key::Enter, .. }))
                        });
                        do_send = true;
                    }
                }

                Frame::none()
                    .fill(t::SURFACE)
                    .stroke(Stroke::new(1.0_f32, stroke_color))
                    .rounding(Rounding::same(24.0))
                    .inner_margin(Margin { left: 8.0, right: 8.0, top: 7.0, bottom: 7.0 })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;

                            // Anexar
                            let (rect, resp) = icon_button(ui, 34.0, t::SURFACE_HI);
                            let c = rect.center();
                            let st = Stroke::new(2.0_f32, t::MUTED);
                            ui.painter().line_segment([egui::pos2(c.x - 6.0, c.y), egui::pos2(c.x + 6.0, c.y)], st);
                            ui.painter().line_segment([egui::pos2(c.x, c.y - 6.0), egui::pos2(c.x, c.y + 6.0)], st);
                            if resp
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("Anexar imagem, documento ou áudio (também dá para arrastar arquivos)")
                                .clicked()
                            {
                                do_attach = true;
                            }

                            // Campo de texto (multilinha, cresce até ~6 linhas)
                            let reserved = 34.0 + 34.0 + 36.0 + 3.0 * 6.0 + 16.0;
                            let text_w = (ui.available_width() - reserved).max(80.0);
                            ui.allocate_ui_with_layout(
                                Vec2::new(text_w + 8.0, 150.0),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    egui::ScrollArea::vertical()
                                        .id_salt("input_scroll")
                                        .max_height(150.0)
                                        .auto_shrink([false, true])
                                        .show(ui, |ui| {
                                            let re = ui.add(
                                                egui::TextEdit::multiline(&mut self.input_text)
                                                    .id(input_id)
                                                    .frame(false)
                                                    .desired_rows(1)
                                                    .desired_width(text_w)
                                                    .lock_focus(true)
                                                    .hint_text(
                                                        RichText::new("Peça qualquer coisa ao DeepGravity...").color(t::FAINT),
                                                    )
                                                    .font(FontId::new(14.5, FontFamily::Proportional))
                                                    .text_color(Color32::WHITE)
                                                    .margin(Margin::symmetric(6.0, 7.0)),
                                            );
                                            if self.want_focus {
                                                re.request_focus();
                                                self.want_focus = false;
                                            }
                                        });
                                },
                            );

                            // Microfone
                            let rec_now = self.recorder.is_some();
                            let mic_fill = if rec_now { t::DANGER } else { t::SURFACE_HI };
                            let (rect, resp) = icon_button(ui, 34.0, mic_fill);
                            let c = rect.center();
                            let col = if rec_now { Color32::WHITE } else { t::MUTED };
                            let st = Stroke::new(1.8_f32, col);
                            ui.painter().rect_filled(
                                Rect::from_center_size(egui::pos2(c.x, c.y - 3.5), Vec2::new(7.0, 11.0)),
                                Rounding::same(3.5),
                                col,
                            );
                            let arc: Vec<egui::Pos2> = (0..=12)
                                .map(|k| {
                                    let a = std::f32::consts::PI * (k as f32 / 12.0);
                                    egui::pos2(c.x + 6.5 * a.cos(), c.y - 2.5 + 6.5 * a.sin())
                                })
                                .collect();
                            ui.painter().add(egui::epaint::PathShape::line(arc, st));
                            ui.painter().line_segment([egui::pos2(c.x, c.y + 4.0), egui::pos2(c.x, c.y + 8.0)], st);
                            ui.painter().line_segment([egui::pos2(c.x - 3.5, c.y + 8.0), egui::pos2(c.x + 3.5, c.y + 8.0)], st);
                            if resp
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(if rec_now { "Parar gravação e transcrever" } else { "Gravar áudio (vira texto)" })
                                .clicked()
                            {
                                do_mic = true;
                            }

                            // Enviar / PARAR
                            let can_send = (!self.input_text.trim().is_empty() || !self.attachments.is_empty())
                                && !self.is_thinking;
                            if self.is_thinking {
                                let (rect, resp) = icon_button(ui, 36.0, t::DANGER);
                                ui.painter().rect_filled(
                                    Rect::from_center_size(rect.center(), Vec2::splat(12.0)),
                                    Rounding::same(3.0),
                                    Color32::WHITE,
                                );
                                if resp
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .on_hover_text("PARAR agora (Esc)")
                                    .clicked()
                                {
                                    do_stop = true;
                                }
                            } else {
                                let fill = if can_send { t::ACCENT } else { t::SURFACE_HI };
                                let (rect, resp) = icon_button(ui, 36.0, fill);
                                let c = rect.center();
                                let col = if can_send { Color32::WHITE } else { t::FAINT };
                                let st = Stroke::new(2.2_f32, col);
                                ui.painter().line_segment([egui::pos2(c.x, c.y + 6.0), egui::pos2(c.x, c.y - 6.0)], st);
                                ui.painter().line_segment([egui::pos2(c.x - 5.5, c.y - 0.5), egui::pos2(c.x, c.y - 6.0)], st);
                                ui.painter().line_segment([egui::pos2(c.x + 5.5, c.y - 0.5), egui::pos2(c.x, c.y - 6.0)], st);
                                if can_send && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    do_send = true;
                                }
                            }
                        });
                    });
                if do_stop {
                    self.stop_agent();
                }
                if do_attach {
                    self.open_picker();
                }
                if do_mic {
                    self.toggle_recording();
                }
                if do_send {
                    let msg = self.input_text.clone();
                    self.send_text(msg);
                    self.want_focus = true;
                }
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new("Enter envia  ·  Shift+Enter nova linha  ·  Esc para parar  ·  Arraste arquivos para anexar")
                            .size(11.0)
                            .color(t::FAINT),
                    );
                });
            });
    }

    fn hero(&mut self, ui: &mut Ui) {
        let mut chosen: Option<String> = None;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.14).max(16.0));
            ui.add(egui::Image::new((self.whale.id(), Vec2::splat(76.0))));
            ui.add_space(14.0);
            ui.label(bold("Como posso ajudar hoje?").size(30.0).color(Color32::WHITE));
            ui.add_space(4.0);
            ui.label(
                RichText::new("Eu leio arquivos, escrevo código, rodo comandos e corrijo erros por você.")
                    .size(14.5)
                    .color(t::MUTED),
            );
            ui.add_space(28.0);

            let gap = 12.0;
            let cw = ((ui.available_width().min(680.0)) - gap) / 2.0;
            let suggestions = [
                ("Explorar este projeto", "Liste os arquivos da pasta atual e resuma a arquitetura"),
                ("Criar um script", "Crie um script Python que organize arquivos por extensão"),
                ("Diagnosticar o sistema", "Mostre uso de CPU, memória e disco deste computador"),
                ("Revisar código", "Procure possíveis bugs nos arquivos .rs desta pasta"),
            ];
            for row in suggestions.chunks(2) {
                ui.allocate_ui_with_layout(
                    Vec2::new(cw * 2.0 + gap, 66.0),
                    Layout::left_to_right(Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for (title, prompt) in row {
                            if choice_card(ui, cw, 66.0, false, title, prompt).clicked() {
                                chosen = Some(prompt.to_string());
                            }
                        }
                    },
                );
            }
        });
        if let Some(p) = chosen {
            self.send_text(p);
        }
    }

    fn chat(&mut self, ui: &mut Ui) {
        let time = ui.input(|i| i.time);
        let whale = self.whale.clone();
        let avail_w = ui.available_width();
        let mut open_settings = false;

        egui::ScrollArea::vertical()
            .id_salt("chat_scroll_area_v2")
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .hscroll(false)
            .horizontal_scroll_offset(0.0)
            .show(ui, |ui| {
                ui.set_width(avail_w);
                ui.set_max_width(avail_w);
                ui.add_space(14.0);
                ui.spacing_mut().item_spacing.y = 14.0;
                let chat_w = (avail_w - 6.0).max(200.0);
                for item in &mut self.items {
                    match item {
                        ChatItem::User(text) => {
                            let (clean, atts) = client::split_attachments(text);
                            let max_bubble_w = (chat_w * 0.72).min(600.0).max(140.0);
                            let max_inner = max_bubble_w - 32.0;
                            let job = inline_job(&clean, 15.0, Color32::WHITE, max_inner);
                            let text_w = if clean.is_empty() {
                                0.0
                            } else {
                                ui.fonts(|f| f.layout_job(job.clone())).size().x
                            };
                            let img_w = if atts.is_empty() { 0.0 } else { 200.0 };
                            let inner_w = (text_w.max(img_w).ceil() + 2.0).min(max_inner).max(30.0);
                            let bubble_total_w = inner_w + 32.0;
                            let space_left = (chat_w - bubble_total_w).max(0.0);

                            ui.horizontal(|ui| {
                                ui.add_space(space_left);
                                Frame::none()
                                    .fill(t::USER_BUBBLE)
                                    .stroke(Stroke::new(1.0_f32, t::ACCENT.linear_multiply(0.25)))
                                    .rounding(Rounding { nw: 18.0, ne: 18.0, sw: 18.0, se: 5.0 })
                                    .inner_margin(Margin::symmetric(16.0, 11.0))
                                    .show(ui, |ui| {
                                        ui.set_width(inner_w);
                                        ui.set_max_width(inner_w);
                                        ui.spacing_mut().item_spacing.y = 8.0;
                                        for p in &atts {
                                            let name = std::path::Path::new(p)
                                                .file_name()
                                                .map(|f| f.to_string_lossy().to_string())
                                                .unwrap_or_else(|| p.clone());
                                            let tex = if client::is_image_path(p) {
                                                load_thumb(ui.ctx(), &mut self.thumbs, p, 360)
                                            } else {
                                                None
                                            };
                                            if let Some(tex) = tex {
                                                let s = tex.size_vec2();
                                                let k = (200.0 / s.x).min(180.0 / s.y).min(1.0);
                                                ui.add(
                                                    egui::Image::new((tex.id(), s * k))
                                                        .rounding(Rounding::same(10.0)),
                                                );
                                            } else {
                                                ui.label(
                                                    RichText::new(format!("📎 {}", name))
                                                        .size(13.0)
                                                        .color(t::ACCENT_HI),
                                                );
                                            }
                                        }
                                        if !clean.is_empty() {
                                            ui.add(egui::Label::new(job).selectable(true));
                                        }
                                    });
                            });
                        }
                        ChatItem::Notice(msg) => {
                            ui.vertical_centered(|ui| {
                                Frame::none()
                                    .fill(t::SURFACE)
                                    .stroke(Stroke::new(1.0_f32, t::BORDER))
                                    .rounding(Rounding::same(12.0))
                                    .inner_margin(Margin::symmetric(12.0, 6.0))
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(format!("⏹  {}", msg)).size(12.5).color(t::MUTED));
                                    });
                            });
                        }
                        ChatItem::Assistant(text) => {
                            ui.horizontal_top(|ui| {
                                avatar(ui, &whale);
                                let content_w = (chat_w - 42.0).max(100.0);
                                ui.vertical(|ui| {
                                    ui.set_width(content_w);
                                    ui.set_max_width(content_w);
                                    ui.label(bold("DeepGravity").size(12.5).color(t::ACCENT_HI));
                                    ui.add_space(2.0);
                                    render_markdown(ui, text, content_w);
                                });
                            });
                        }
                        ChatItem::Tool { name, args, result, success, expanded } => {
                            ui.horizontal_top(|ui| {
                                ui.add_space(42.0);
                                let card_w = (chat_w - 42.0).max(100.0);
                                ui.vertical(|ui| {
                                    ui.set_width(card_w);
                                    ui.set_max_width(card_w);
                                    tool_card(ui, name, args, result, *success, expanded, time, card_w);
                                });
                            });
                        }
                        ChatItem::Error(err) => {
                            let err_str = err.as_str();
                            let is_invalid_token = err_str.contains("INVALID_TOKEN")
                                || err_str.contains("40003")
                                || err_str.contains("AuthenticationError");
                            ui.horizontal_top(|ui| {
                                ui.add_space(42.0);
                                let card_w = (chat_w - 42.0).max(100.0);
                                ui.vertical(|ui| {
                                    ui.set_width(card_w);
                                    ui.set_max_width(card_w);
                                    let inner_w = (card_w - 28.0).max(60.0);
                                    Frame::none()
                                        .fill(Color32::from_rgb(42, 17, 22))
                                        .stroke(Stroke::new(1.0_f32, t::DANGER.linear_multiply(0.55)))
                                        .rounding(Rounding::same(12.0))
                                        .inner_margin(Margin::symmetric(14.0, 10.0))
                                        .show(ui, |ui| {
                                            ui.set_width(inner_w);
                                            ui.set_max_width(inner_w);
                                            if is_invalid_token {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new("⚠️").size(15.0));
                                                    ui.label(bold("Sessão DeepSeek Expirada").size(13.5).color(Color32::from_rgb(255, 180, 180)));
                                                });
                                                ui.add_space(4.0);
                                                ui.add(egui::Label::new(
                                                    RichText::new("O token web do chat.deepseek.com expirou ou foi invalidado. Para voltar a usar, cole seu novo token em Ajustes ⚙.")
                                                        .size(12.5)
                                                        .color(Color32::from_rgb(240, 210, 210)),
                                                ).wrap());
                                                ui.add_space(8.0);
                                                ui.horizontal(|ui| {
                                                    let btn = egui::Button::new(
                                                        RichText::new("⚙ Atualizar Token").family(t::bold_family()).size(12.0).color(Color32::WHITE),
                                                    )
                                                    .fill(t::ACCENT)
                                                    .rounding(Rounding::same(6.0));
                                                    if ui.add(btn).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                                        open_settings = true;
                                                    }
                                                });
                                            } else {
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new("❌").size(14.0));
                                                    ui.label(bold("Algo deu errado").size(13.0).color(Color32::from_rgb(255, 180, 180)));
                                                });
                                                ui.add_space(3.0);
                                                ui.add(egui::Label::new(
                                                    RichText::new(format!("Algo deu errado: {}", err_str))
                                                        .size(12.5)
                                                        .color(Color32::from_rgb(254, 202, 202)),
                                                ).wrap());
                                            }
                                        });
                                });
                            });
                        }
                    }
                }

                if self.is_thinking {
                    ui.horizontal(|ui| {
                        avatar(ui, &whale);
                        for k in 0..3 {
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                            let a = ((time as f32) * 4.5 - k as f32 * 0.9).sin() * 0.5 + 0.5;
                            ui.painter().circle_filled(
                                egui::pos2(r.center().x, r.center().y - 3.0 * a),
                                3.5,
                                t::ACCENT_HI.linear_multiply(0.35 + 0.65 * a),
                            );
                        }
                    });
                }
                ui.add_space(12.0);
            });

        if open_settings {
            self.sidebar_open = true;
            self.sidebar_tab = SidebarTab::Settings;
        }
    }
}

impl eframe::App for DeepGravityApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.check_events();

        // Parada de emergência: Esc
        if self.is_thinking && ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.stop_agent();
        }

        // Arrastar e soltar arquivos (imagens, documentos, áudio)
        let dropped: Vec<String> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.as_ref().map(|p| p.to_string_lossy().to_string()))
                .collect()
        });
        for p in dropped {
            self.add_media(p);
        }

        if self.is_thinking || self.recorder.is_some() || self.busy_stt > 0 {
            ctx.request_repaint_after(Duration::from_millis(33));
        } else {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        if self.sidebar_open {
            self.sidebar(ctx);
        }

        let avail = ctx.available_rect().width();
        let max_col = COLUMN_WIDTH.min(avail - 32.0);
        let pad = ((avail - max_col) / 2.0).max(16.0);

        self.input_panel(ctx, pad);

        egui::CentralPanel::default()
            .frame(Frame::none().fill(t::BG).inner_margin(Margin {
                left: pad,
                right: pad,
                top: 10.0,
                bottom: 0.0,
            }))
            .show(ctx, |ui| {
                // Barra de cabeçalho estilo Antigravity (Toggle lateral + Pasta ativa + Sessão + Modelo)
                ui.horizontal(|ui| {
                    let toggle_icon = if self.sidebar_open { "◀" } else { "▶" };
                    let toggle_tip = if self.sidebar_open {
                        "Esconder painel lateral"
                    } else {
                        "Mostrar painel lateral"
                    };
                    let toggle_btn = egui::Button::new(
                        RichText::new(toggle_icon)
                            .size(12.0)
                            .color(if self.sidebar_open { t::MUTED } else { t::ACCENT_HI }),
                    )
                    .fill(if self.sidebar_open { t::SURFACE } else { t::ACCENT_SOFT })
                    .stroke(Stroke::new(
                        1.0_f32,
                        if self.sidebar_open { t::BORDER } else { t::ACCENT.linear_multiply(0.5) },
                    ))
                    .rounding(Rounding::same(6.0))
                    .min_size(Vec2::new(26.0, 24.0));
                    if ui
                        .add(toggle_btn)
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(toggle_tip)
                        .clicked()
                    {
                        self.sidebar_open = !self.sidebar_open;
                    }

                    ui.add_space(4.0);
                    ui.label(RichText::new("📁").size(13.0));
                    let folder_name = std::path::Path::new(&self.current_dir)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| self.current_dir.clone());
                    ui.label(RichText::new(folder_name).family(t::bold_family()).size(12.5).color(t::TEXT));
                    ui.label(RichText::new("·").size(12.5).color(t::FAINT));
                    let title_display = if self.current_session_title.chars().count() > 38 {
                        let s: String = self.current_session_title.chars().take(36).collect();
                        format!("{}…", s)
                    } else {
                        self.current_session_title.clone()
                    };
                    ui.label(RichText::new(title_display).size(12.5).color(t::MUTED));

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let model_label = if self.selected_model.contains("reasoner") {
                            "DeepSeek R1"
                        } else {
                            "DeepSeek Chat"
                        };
                        pill(ui, t::ACCENT, model_label);
                    });
                });
                ui.add_space(6.0);
                ui.separator();

                if self.items.is_empty() && !self.is_thinking {
                    self.hero(ui);
                } else {
                    self.chat(ui);
                }
            });
    }
}

pub fn run_gui(
    endpoint: String,
    model: String,
    current_dir: String,
    auto_approve: bool,
    initial_prompt: Option<String>,
) -> eframe::Result<()> {
    let tmp_ctx = egui::Context::default();
    let (_, _, icon) = load_textures(&tmp_ctx);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([760.0, 520.0])
            .with_title("DeepGravity")
            .with_app_id("deepgravity")
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "DeepGravity",
        native_options,
        Box::new(move |cc| {
            theme::install_fonts(&cc.egui_ctx);
            cc.egui_ctx.style_mut(|style| theme::apply_theme(style));
            let mut app = DeepGravityApp::new(&cc.egui_ctx, endpoint, model, current_dir, auto_approve);
            if let Some(p) = initial_prompt {
                app.send_text(p);
            }
            Ok(Box::new(app))
        }),
    )
}
