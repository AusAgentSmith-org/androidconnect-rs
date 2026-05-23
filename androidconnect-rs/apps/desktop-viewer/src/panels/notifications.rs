use std::collections::{HashMap, HashSet};

use eframe::egui;
use egui::{Color32, Frame, Margin, RichText, ScrollArea, Stroke};

use androidconnect_protocol::{
    NotificationAction, NotificationFilterUpdate, NotificationPosted, NotificationRemoved, Payload,
};

#[derive(Debug, Default)]
pub struct NotificationsState {
    /// Active notifications keyed by `notification_id`, preserving insertion order via `order`.
    items: HashMap<String, NotificationPosted>,
    order: Vec<String>,
    /// Per-notification quick-reply draft buffer keyed on (notification_id, action_id).
    reply_drafts: HashMap<(String, String), String>,
    /// Local-only suppression list — set from the per-app kebab menu.
    suppressed_packages: HashSet<String>,
    pub hide_sensitive: bool,
}

impl NotificationsState {
    pub fn posted(&mut self, n: NotificationPosted) {
        if let Some(existing) = self.items.get_mut(&n.notification_id) {
            *existing = n;
        } else {
            self.order.push(n.notification_id.clone());
            self.items.insert(n.notification_id.clone(), n);
        }
    }

    pub fn removed(&mut self, r: NotificationRemoved) {
        self.items.remove(&r.notification_id);
        self.order.retain(|id| id != &r.notification_id);
        self.reply_drafts
            .retain(|(id, _), _| id != &r.notification_id);
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.order.clear();
        self.reply_drafts.clear();
    }

    pub fn toggle_suppress(&mut self, package: &str) {
        if !self.suppressed_packages.remove(package) {
            self.suppressed_packages.insert(package.to_owned());
        }
    }
}

/// Outbound payloads the panel wants the network thread to send.
pub struct PendingActions {
    pub payloads: Vec<Payload>,
}

pub fn draw(ui: &mut egui::Ui, state: &mut NotificationsState) -> PendingActions {
    let mut pending: Vec<Payload> = Vec::new();

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} notifications", state.items.len())).heading());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.checkbox(&mut state.hide_sensitive, "Hide sensitive content");
        });
    });
    ui.separator();
    ui.add_space(4.0);

    if state.items.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(RichText::new("No active notifications").italics());
        });
        return PendingActions { payloads: pending };
    }

    ScrollArea::vertical().show(ui, |ui| {
        let ids = state.order.clone();
        for id in ids {
            let Some(n) = state.items.get(&id).cloned() else {
                continue;
            };
            let suppressed = state.suppressed_packages.contains(&n.app_package);
            Frame::new()
                .stroke(Stroke::new(1.0, Color32::from_gray(80)))
                .corner_radius(4)
                .inner_margin(Margin::same(8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&n.app_name).strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("⋮").on_hover_text("Per-app actions").clicked() {
                                let enabled_after_click = suppressed;
                                state.toggle_suppress(&n.app_package);
                                pending.push(Payload::NotificationFilterUpdate(
                                    NotificationFilterUpdate {
                                        package_name: n.app_package.clone(),
                                        enabled: enabled_after_click,
                                    },
                                ));
                            }
                            if suppressed {
                                ui.label(RichText::new("suppressed").italics().small());
                            }
                        });
                    });
                    if let Some(title) = &n.title {
                        let display = if state.hide_sensitive && n.sensitive {
                            "[sensitive content hidden]".to_owned()
                        } else {
                            title.clone()
                        };
                        ui.label(RichText::new(display).strong());
                    }
                    if let Some(text) = &n.text {
                        let display = if state.hide_sensitive && n.sensitive {
                            String::new()
                        } else {
                            text.clone()
                        };
                        if !display.is_empty() {
                            ui.label(display);
                        }
                    }
                    if !n.actions.is_empty() {
                        ui.add_space(4.0);
                        for action in &n.actions {
                            if action.allows_reply {
                                let key = (n.notification_id.clone(), action.action_id.clone());
                                let draft = state.reply_drafts.entry(key.clone()).or_default();
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(draft)
                                            .desired_width(220.0)
                                            .hint_text("Reply…"),
                                    );
                                    if ui.button(&action.title).clicked() && !draft.is_empty() {
                                        pending.push(Payload::NotificationAction(
                                            NotificationAction {
                                                notification_id: n.notification_id.clone(),
                                                action_id: action.action_id.clone(),
                                                reply_text: Some(draft.clone()),
                                            },
                                        ));
                                        draft.clear();
                                    }
                                });
                            } else if ui.button(&action.title).clicked() {
                                pending.push(Payload::NotificationAction(NotificationAction {
                                    notification_id: n.notification_id.clone(),
                                    action_id: action.action_id.clone(),
                                    reply_text: None,
                                }));
                            }
                        }
                    }
                });
            ui.add_space(6.0);
        }
    });

    PendingActions { payloads: pending }
}
