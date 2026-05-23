use eframe::egui;
use egui::{
    Color32, FontId, Frame, Image, Margin, RichText, ScrollArea, Stroke, TextureHandle, Vec2,
};

use crate::status::{ConnectionState, DesktopStatus, format_pairing_code};

pub fn draw(ui: &mut egui::Ui, status: &DesktopStatus, qr_texture: Option<&TextureHandle>) {
    ScrollArea::vertical().show(ui, |ui| {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Pair a device").font(FontId::proportional(28.0)));
            ui.add_space(12.0);
            ui.label(
                "Open AndroidConnect on your phone and scan the QR, or enter the code manually.",
            );
            ui.add_space(28.0);

            Frame::new()
                .stroke(Stroke::new(1.0, Color32::from_gray(120)))
                .corner_radius(4)
                .inner_margin(Margin::same(12))
                .fill(Color32::WHITE)
                .show(ui, |ui| match qr_texture {
                    Some(handle) => {
                        let size = Vec2::splat(256.0);
                        ui.add(Image::from_texture((handle.id(), size)).fit_to_exact_size(size));
                    }
                    None => {
                        ui.set_width(256.0);
                        ui.set_height(256.0);
                        ui.vertical_centered(|ui| {
                            ui.add_space(110.0);
                            ui.label(
                                RichText::new("QR unavailable")
                                    .color(Color32::from_gray(40))
                                    .italics(),
                            );
                        });
                    }
                });
            ui.add_space(20.0);

            ui.label(RichText::new("Manual pairing").heading());
            ui.add_space(6.0);
            ui.label(format!("Address: {}", status.bind));
            ui.label(format!(
                "Pairing code: {}",
                format_pairing_code(&status.pairing_code)
            ));
            ui.add_space(16.0);
            if !matches!(status.connection, ConnectionState::Listening) {
                ui.label(RichText::new("Waiting for handshake…").italics());
                if let Some(err) = &status.last_error {
                    ui.colored_label(Color32::RED, err);
                }
            }
        });
    });
}
