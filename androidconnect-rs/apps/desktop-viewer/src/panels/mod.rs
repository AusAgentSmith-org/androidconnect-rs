pub mod files;
pub mod messages;
pub mod mirror;
pub mod notifications;
pub mod pair;
pub mod phone;

use eframe::egui;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Panel {
    Mirror,
    Notifications,
    Messages,
    Files,
    Phone,
}

impl Panel {
    pub fn label(self) -> &'static str {
        match self {
            Panel::Mirror => "Mirror",
            Panel::Notifications => "Notifications",
            Panel::Messages => "Messages",
            Panel::Files => "Files",
            Panel::Phone => "Phone",
        }
    }

    pub const ALL: [Panel; 5] = [
        Panel::Mirror,
        Panel::Notifications,
        Panel::Messages,
        Panel::Files,
        Panel::Phone,
    ];
}

pub fn placeholder(ui: &mut egui::Ui, title: &str, sub: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(60.0);
        ui.label(egui::RichText::new(title).heading());
        ui.add_space(8.0);
        ui.label(sub);
    });
}
