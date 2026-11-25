use egui::{pos2, vec2, Color32, Rect, Response, Sense, Ui};

pub fn vertical(ui: &mut Ui, value: &mut u8, color: Color32) -> Response {
    let height = 130.0;
    let width = 36.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_rgb(24, 24, 28));
    let inner = rect.shrink2(vec2(4.0, 4.0));
    let fill_h = inner.height() * (*value as f32 / 255.0);
    let fill = Rect::from_min_max(
        pos2(inner.left(), inner.bottom() - fill_h),
        inner.right_bottom(),
    );
    painter.rect_filled(fill, 2.0, color);
    let cap_y = inner.bottom() - fill_h;
    painter.rect_filled(
        Rect::from_center_size(
            pos2(inner.center().x, cap_y),
            vec2(inner.width(), 6.0),
        ),
        2.0,
        Color32::from_gray(235),
    );
    painter.text(
        pos2(rect.center().x, rect.bottom() - 2.0),
        egui::Align2::CENTER_BOTTOM,
        format!("{value}"),
        egui::FontId::monospace(11.0),
        Color32::from_gray(180),
    );
    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let u = ((inner.bottom() - pos.y) / inner.height()).clamp(0.0, 1.0);
            *value = (u * 255.0).round() as u8;
        }
    }
    response
}