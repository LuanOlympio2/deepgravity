use eframe::egui::{
    self, epaint::Shadow, style::ScrollStyle, Color32, FontData, FontDefinitions, FontFamily, FontId,
    Rounding, Stroke, Style, TextStyle,
};

// Paleta "premium dark" inspirada no DeepSeek.
pub const BG: Color32 = Color32::from_rgb(11, 13, 18);
pub const SIDEBAR: Color32 = Color32::from_rgb(14, 17, 23);
pub const SURFACE: Color32 = Color32::from_rgb(20, 24, 32);
pub const SURFACE_HI: Color32 = Color32::from_rgb(27, 32, 43);
pub const CODE_BG: Color32 = Color32::from_rgb(9, 11, 15);
pub const BORDER: Color32 = Color32::from_rgb(33, 39, 51);
pub const ACCENT: Color32 = Color32::from_rgb(77, 107, 254); // azul oficial DeepSeek
pub const ACCENT_HI: Color32 = Color32::from_rgb(107, 132, 255);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(25, 33, 74);
pub const USER_BUBBLE: Color32 = Color32::from_rgb(30, 38, 66);
pub const TEXT: Color32 = Color32::from_rgb(230, 234, 243);
pub const MUTED: Color32 = Color32::from_rgb(133, 142, 163);
pub const FAINT: Color32 = Color32::from_rgb(82, 90, 110);
pub const SUCCESS: Color32 = Color32::from_rgb(52, 211, 153);
pub const WARN: Color32 = Color32::from_rgb(251, 191, 36);
pub const DANGER: Color32 = Color32::from_rgb(248, 113, 113);

pub fn bold_family() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "noto".into(),
        FontData::from_static(include_bytes!("../../assets/NotoSans-Regular.ttf")),
    );
    fonts.font_data.insert(
        "noto-bold".into(),
        FontData::from_static(include_bytes!("../../assets/NotoSans-Bold.ttf")),
    );
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "noto".into());
    // Família "bold": Noto Bold primeiro, com fallback para as fontes padrão (símbolos/emoji).
    let mut bold = vec!["noto-bold".to_string()];
    bold.extend(
        fonts
            .families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts.families.insert(bold_family(), bold);
    ctx.set_fonts(fonts);
}

pub fn apply_theme(style: &mut Style) {
    use TextStyle::*;
    style.text_styles = [
        (Heading, FontId::new(24.0, bold_family())),
        (Body, FontId::new(15.0, FontFamily::Proportional)),
        (Button, FontId::new(14.0, FontFamily::Proportional)),
        (Small, FontId::new(12.0, FontFamily::Proportional)),
        (Monospace, FontId::new(13.5, FontFamily::Monospace)),
    ]
    .into();

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_rounding = Rounding::same(14.0);
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.extreme_bg_color = CODE_BG;
    v.faint_bg_color = SURFACE;
    v.selection.bg_fill = ACCENT.linear_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0_f32, ACCENT_HI);
    v.hyperlink_color = ACCENT_HI;

    let r = Rounding::same(10.0);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = r;
    }
    v.widgets.noninteractive.bg_fill = SURFACE;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.inactive.bg_fill = SURFACE;
    v.widgets.inactive.weak_bg_fill = SURFACE;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.hovered.bg_fill = SURFACE_HI;
    v.widgets.hovered.weak_bg_fill = SURFACE_HI;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, ACCENT.linear_multiply(0.6));
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.widgets.active.bg_fill = ACCENT;
    v.widgets.active.weak_bg_fill = ACCENT;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT_HI);

    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style.spacing.scroll = ScrollStyle::floating();
}
