use egui::{Color32, Context, FontFamily, FontId, RichText, TextStyle, Visuals};

pub const BG_DEEP: Color32 = Color32::from_rgb(22, 22, 24);
pub const BG_PANEL: Color32 = Color32::from_rgb(32, 32, 36);
pub const BG_LANE: Color32 = Color32::from_rgb(28, 28, 31);
pub const BG_LANE_ALT: Color32 = Color32::from_rgb(31, 31, 35);
pub const RULER_BG: Color32 = Color32::from_rgb(40, 40, 44);
pub const GRID_MAJOR: Color32 = Color32::from_rgb(58, 58, 64);
pub const GRID_MINOR: Color32 = Color32::from_rgb(44, 44, 48);
pub const PLAYHEAD: Color32 = Color32::from_rgb(255, 90, 70);
pub const ACCENT: Color32 = Color32::from_rgb(100, 160, 220);
pub const TRANSPORT_PLAY: Color32 = Color32::from_rgb(72, 180, 110);

pub fn track_color(index: usize) -> Color32 {
    const PALETTE: [Color32; 8] = [
        Color32::from_rgb(100, 180, 255),
        Color32::from_rgb(255, 140, 90),
        Color32::from_rgb(140, 220, 140),
        Color32::from_rgb(220, 140, 255),
        Color32::from_rgb(255, 210, 90),
        Color32::from_rgb(90, 210, 210),
        Color32::from_rgb(255, 120, 160),
        Color32::from_rgb(180, 180, 180),
    ];
    PALETTE[index % PALETTE.len()]
}

pub fn apply(ctx: &Context) {
    let mut visuals = Visuals::dark();
    visuals.panel_fill = BG_PANEL;
    visuals.window_fill = Color32::from_rgb(36, 36, 40);
    visuals.extreme_bg_color = BG_DEEP;
    visuals.faint_bg_color = BG_LANE;
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(42, 42, 46);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(50, 50, 55);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(58, 58, 64);
    visuals.widgets.active.bg_fill = Color32::from_rgb(68, 68, 76);
    visuals.selection.bg_fill = Color32::from_rgb(55, 95, 135);
    visuals.selection.stroke.color = ACCENT;
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace));
    ctx.set_style(style);
}

pub fn label_muted(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).color(Color32::from_gray(150))
}

pub fn label_timecode(text: impl Into<String>) -> RichText {
    RichText::new(text.into())
        .monospace()
        .size(20.0)
        .color(Color32::from_rgb(230, 230, 235))
}