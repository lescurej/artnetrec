use crate::devices::tr512::cof::{Config, DmxPort, TriggerMode, TriggerStatus};
use egui::{RichText, Ui};

pub struct CofViewState {
    pub selected_trigger: usize,
}

impl Default for CofViewState {
    fn default() -> Self {
        Self {
            selected_trigger: 0,
        }
    }
}

pub fn ui(ui: &mut Ui, config: &mut Config, state: &mut CofViewState) {
    ui.heading("Device");
    ui.horizontal(|ui| {
        ui.label("Label");
        let mut label = config.device_label.clone();
        if ui.text_edit_singleline(&mut label).changed() {
            config.device_label = truncate(&label, 32);
        }
    });
    ui.horizontal(|ui| {
        ui.label("Firmware");
        ui.monospace(
            String::from_utf8_lossy(&config.firmware_block)
                .trim_matches('\0')
                .to_string(),
        );
    });

    ui.add_space(8.0);
    ui.heading("Display");
    ui.horizontal(|ui| {
        ui.label("0x3B");
        byte_field(ui, &mut config.display_a);
        ui.label("Timer (5s steps)");
        byte_field(ui, &mut config.display_timer_steps);
        ui.label("0x3D");
        byte_field(ui, &mut config.display_b);
    });
    ui.horizontal(|ui| {
        ui.label("0x3A");
        byte_field(ui, &mut config.byte_3a);
        ui.label("0x3E");
        byte_field(ui, &mut config.byte_3e);
        ui.label("0x3F");
        byte_field(ui, &mut config.byte_3f);
    });

    ui.add_space(8.0);
    ui.heading("DMX");
    ui.horizontal(|ui| {
        ui.label("Port");
        egui::ComboBox::from_id_salt("dmx_port")
            .selected_text(port_label(config.dmx_port))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut config.dmx_port, DmxPort::Single, "SINGLE");
                ui.selectable_value(&mut config.dmx_port, DmxPort::Htp, "HTP");
            });
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut config.dmx_trigger_enabled, "DMX trigger enabled");
        ui.label("Address");
        let mut address = config.dmx_trigger_address;
        if ui
            .add(egui::DragValue::new(&mut address).range(1..=512))
            .changed()
        {
            config.dmx_trigger_address = address;
        }
    });

    ui.add_space(8.0);
    ui.heading("Triggers");
    ui.horizontal(|ui| {
        for index in 0..6 {
            let selected = state.selected_trigger == index;
            if ui
                .selectable_label(selected, format!("{}", index + 1))
                .clicked()
            {
                state.selected_trigger = index;
            }
        }
    });
    let trigger = &mut config.triggers[state.selected_trigger];
    ui.group(|ui| {
        ui.label(RichText::new(format!("Trigger {}", state.selected_trigger + 1)).strong());
        ui.horizontal(|ui| {
            ui.label("Name");
            let mut name = trigger.name.clone();
            if ui.text_edit_singleline(&mut name).changed() {
                trigger.name = truncate(&name, 22);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Mode");
            egui::ComboBox::from_id_salt("trigger_mode")
                .selected_text(mode_label(trigger.mode))
                .show_ui(ui, |ui| {
                    for mode in TriggerMode::ALL {
                        ui.selectable_value(&mut trigger.mode, mode, mode_label(mode));
                    }
                });
            ui.label("Delay (s)");
            ui.add(egui::DragValue::new(&mut trigger.delay_secs).range(0..=120));
        });
        ui.horizontal(|ui| {
            ui.label("Status");
            let mut enabled = trigger.status == TriggerStatus::Enable;
            if ui.checkbox(&mut enabled, "Enabled").changed() {
                trigger.status = if enabled {
                    TriggerStatus::Enable
                } else {
                    TriggerStatus::Disable
                };
            }
            ui.label("Index");
            ui.add(egui::DragValue::new(&mut trigger.index).range(0..=5));
        });
        ui.horizontal(|ui| {
            ui.label("Show file");
            let mut file = trigger.show_file.clone();
            if ui.text_edit_singleline(&mut file).changed() {
                trigger.show_file = truncate(&file, 22);
            }
        });
    });

    ui.add_space(8.0);
    ui.heading("Idle");
    ui.horizontal(|ui| {
        ui.label("File");
        let mut file = config.idle_file.clone();
        if ui.text_edit_singleline(&mut file).changed() {
            config.idle_file = truncate(&file, 22);
        }
    });
    ui.horizontal(|ui| {
        ui.label("Mode");
        ui.add(egui::DragValue::new(&mut config.idle_mode).range(0..=2));
        ui.label("(0=factory, 1/2=Show/Passthru unconfirmed)");
    });
}

fn byte_field(ui: &mut Ui, value: &mut u8) {
    let mut wide = u32::from(*value);
    if ui
        .add(egui::DragValue::new(&mut wide).range(0..=255))
        .changed()
    {
        *value = wide as u8;
    }
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn port_label(port: DmxPort) -> &'static str {
    match port {
        DmxPort::Single => "SINGLE",
        DmxPort::Htp => "HTP",
    }
}

fn mode_label(mode: TriggerMode) -> &'static str {
    match mode {
        TriggerMode::NoTr => "NO_TR",
        TriggerMode::NcTr => "NC_TR",
        TriggerMode::ToggleTr => "TOGGLE_TR",
        TriggerMode::AutoTr => "AUTO_TR",
    }
}
