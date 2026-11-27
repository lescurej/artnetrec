use egui::{self, RichText};

pub fn window(ctx: &egui::Context, open: &mut bool, port: u16, server_on: bool) {
    let mut stay_open = *open;
    egui::Window::new("Connect MCP (Cursor)")
        .open(&mut stay_open)
        .default_width(540.0)
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(RichText::new("This editor exposes tools while it is running.").strong());
            ui.add_space(6.0);
            if server_on {
                ui.horizontal(|ui| {
                    ui.label("Server:");
                    ui.monospace(format!("http://127.0.0.1:{port}/mcp"));
                    if ui.button("Copy URL").clicked() {
                        ui.ctx().copy_text(format!("http://127.0.0.1:{port}/mcp"));
                    }
                });
            } else {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 120, 80),
                    "MCP server is off (port may be in use). Set ARTNETREC_MCP_PORT and restart.",
                );
            }
            ui.add_space(8.0);
            ui.label("1. Keep this app open.");
            ui.label("2. Open a .jsonl show (tools need an open show).");
            ui.label("3. In Cursor: Settings → MCP → Add server.");
            ui.label("4. Paste the URL above (Streamable HTTP / URL type).");
            ui.label("5. Refresh MCP servers or restart Cursor.");
            ui.label("6. In Chat, use Agent mode with this server enabled.");
            ui.label("An agent edits by overwriting the open .jsonl. This window reloads it.");
            ui.add_space(8.0);
            ui.label(RichText::new("Cursor config snippet").strong());
            let json = cursor_config(port);
            egui::ScrollArea::vertical()
                .max_height(140.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut json.as_str())
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace),
                    );
                });
            if ui.button("Copy snippet").clicked() {
                ui.ctx().copy_text(cursor_config(port));
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new("If tools are missing: reopen this window after rebuilding, then refresh MCP in Cursor.")
                    .color(egui::Color32::GRAY),
            );
        });
    *open = stay_open;
}

fn cursor_config(port: u16) -> String {
    format!(
        r#"{{
  "mcpServers": {{
    "dmx-recorder": {{
      "url": "http://127.0.0.1:{port}/mcp"
    }}
  }}
}}"#
    )
}
