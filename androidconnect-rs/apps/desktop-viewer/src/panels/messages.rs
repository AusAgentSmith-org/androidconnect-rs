use eframe::egui;
use egui::RichText;

use crate::panels;

pub fn draw(ui: &mut egui::Ui) {
    // SMS panel still depends on Android-side MessageThreadList delivery, which is
    // outside the protocol-prework scope. Surface a clean placeholder until 3B-SMS
    // gets its Android publishers wired.
    panels::placeholder(
        ui,
        "Messages",
        "SMS/MMS panel scaffolded — needs Android-side thread publisher.",
    );
    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(
                "(MessageThreadList + MessageEvent payloads already exist on the wire; \
                          Android does not yet emit them.)",
            )
            .small()
            .italics(),
        );
    });
}
