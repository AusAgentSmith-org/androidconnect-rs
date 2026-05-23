use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui;
use egui::{Color32, Frame, Margin, RichText, ScrollArea, Stroke};

use androidconnect_protocol::{
    FileBrowseRequest, FileBrowseResponse, FileEntry, FileEntryType, FileMutation,
    FileMutationKind, Payload,
};

#[derive(Debug, Default)]
pub struct FilesState {
    pub current_path: String,
    pub history: Vec<String>,
    pub responses: HashMap<String, FileBrowseResponse>,
    pub pending_request: Option<String>,
    #[allow(dead_code)]
    pub error: Option<String>,
    /// Local "needs refresh on next draw" flag (set when the panel becomes active
    /// without a current response).
    requested_once: bool,
    /// Save target chosen by the user for downloads; the actual file write happens in
    /// app.rs because arboard-style I/O lives there.
    pub pending_downloads: Vec<(String, PathBuf)>,
}

impl FilesState {
    pub fn record_response(&mut self, response: FileBrowseResponse) {
        if Some(&response.request_id) == self.pending_request.as_ref() {
            self.pending_request = None;
        }
        self.current_path = response.path.clone();
        self.responses.insert(response.path.clone(), response);
    }

    fn ensure_initial_browse(&mut self) -> Option<Payload> {
        if self.requested_once {
            return None;
        }
        if !self.responses.contains_key(&self.current_path) && self.pending_request.is_none() {
            self.requested_once = true;
            return Some(self.request_browse(self.current_path.clone()));
        }
        self.requested_once = true;
        None
    }

    fn request_browse(&mut self, path: String) -> Payload {
        let request_id = format!("browse-{}", new_request_token());
        self.pending_request = Some(request_id.clone());
        Payload::FileBrowseRequest(FileBrowseRequest {
            request_id,
            path,
            include_thumbnails: false,
        })
    }

    fn navigate_to(&mut self, path: String) -> Payload {
        self.history.push(self.current_path.clone());
        self.current_path = path.clone();
        self.request_browse(path)
    }

    fn navigate_back(&mut self) -> Option<Payload> {
        let prev = self.history.pop()?;
        self.current_path = prev.clone();
        Some(self.request_browse(prev))
    }
}

pub struct PendingActions {
    pub payloads: Vec<Payload>,
}

pub fn draw(ui: &mut egui::Ui, state: &mut FilesState) -> PendingActions {
    let mut pending: Vec<Payload> = Vec::new();

    if let Some(p) = state.ensure_initial_browse() {
        pending.push(p);
    }

    ui.horizontal(|ui| {
        ui.label(RichText::new("Files").heading());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("⟳ Refresh").clicked() {
                let path = state.current_path.clone();
                pending.push(state.request_browse(path));
            }
            if ui.button("⬅ Back").clicked()
                && let Some(p) = state.navigate_back()
            {
                pending.push(p);
            }
        });
    });
    ui.separator();
    ui.add_space(6.0);

    ui.horizontal_wrapped(|ui| {
        ui.label("Path:");
        ui.label(RichText::new(&state.current_path).monospace().italics());
    });
    ui.add_space(6.0);

    if state.pending_request.is_some() {
        ui.label(RichText::new("Loading…").italics());
        return PendingActions { payloads: pending };
    }

    let Some(response) = state.responses.get(&state.current_path).cloned() else {
        ui.label(RichText::new("No data yet for this path.").italics());
        return PendingActions { payloads: pending };
    };

    if !matches!(
        response.status.state,
        androidconnect_protocol::FeatureState::Available
    ) {
        Frame::new()
            .stroke(Stroke::new(1.0, Color32::RED))
            .corner_radius(4)
            .inner_margin(Margin::same(6))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!(
                        "{:?}: {}",
                        response.status.state, response.status.message
                    ))
                    .strong(),
                );
            });
        return PendingActions { payloads: pending };
    }

    let entries = response.entries.clone();
    ScrollArea::vertical().show(ui, |ui| {
        for entry in entries {
            let entry_clone = entry.clone();
            let label = entry_row(&entry);
            let selectable = ui.selectable_label(false, label);
            if selectable.clicked() {
                match entry_clone.entry_type {
                    FileEntryType::Directory => {
                        pending.push(state.navigate_to(entry_clone.path.clone()));
                    }
                    _ => { /* selection-only; preview/download via context menu */ }
                }
            }
            selectable.context_menu(|ui| {
                if matches!(
                    entry_clone.entry_type,
                    FileEntryType::File | FileEntryType::Media
                ) && ui.button("Download").clicked()
                {
                    ui.close();
                    if let Some(dest) = rfd::FileDialog::new()
                        .set_file_name(&entry_clone.name)
                        .save_file()
                    {
                        state
                            .pending_downloads
                            .push((entry_clone.path.clone(), dest));
                    }
                }
                if ui.button("Delete").clicked() {
                    ui.close();
                    pending.push(Payload::FileMutation(FileMutation {
                        request_id: format!("mut-{}", new_request_token()),
                        mutation: FileMutationKind::Delete,
                        path: entry_clone.path.clone(),
                        new_path: None,
                    }));
                }
                if ui.button("New folder here").clicked() {
                    ui.close();
                    let new_name = format!("New folder {}", short_token());
                    let joined = join_path(&state.current_path, &new_name);
                    pending.push(Payload::FileMutation(FileMutation {
                        request_id: format!("mut-{}", new_request_token()),
                        mutation: FileMutationKind::CreateFolder,
                        path: joined,
                        new_path: None,
                    }));
                }
            });
        }
    });

    PendingActions { payloads: pending }
}

fn entry_row(entry: &FileEntry) -> String {
    let icon = match entry.entry_type {
        FileEntryType::Directory => "📁",
        FileEntryType::File => "📄",
        FileEntryType::Media => "🖼",
    };
    let size = entry
        .size_bytes
        .map(crate::status::format_bytes_short)
        .unwrap_or_else(|| "—".to_owned());
    format!("{icon}  {:<48} {}", truncate(&entry.name, 46), size)
}

fn truncate(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let kept: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{kept}…")
    } else {
        kept
    }
}

fn join_path(base: &str, name: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

fn new_request_token() -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or_default();
    format!("{micros}-{}", std::process::id())
}

fn short_token() -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or_default();
    format!("{:x}", micros & 0xffff)
}
