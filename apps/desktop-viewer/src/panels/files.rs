use std::collections::HashMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use androidconnect_protocol::{FileBrowseRequest, FileBrowseResponse, Payload};

#[derive(Debug, Default)]
pub struct FilesState {
    pub current_path: String,
    pub history: Vec<String>,
    pub responses: HashMap<String, FileBrowseResponse>,
    pub pending_request: Option<String>,
    pub view_mode: FilesViewMode,
    pub selected_path: Option<String>,
    pub last_click_path: Option<String>,
    pub last_click_time: Option<Instant>,
    requested_once: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum FilesViewMode {
    #[default]
    List,
    Grid,
}

impl FilesState {
    pub fn record_response(&mut self, response: FileBrowseResponse) {
        if Some(&response.request_id) == self.pending_request.as_ref() {
            self.pending_request = None;
        }
        self.current_path = response.path.clone();
        self.selected_path = None;
        self.responses.insert(response.path.clone(), response);
    }

    pub fn ensure_initial_browse(&mut self) -> Option<Payload> {
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

    pub fn request_browse(&mut self, path: String) -> Payload {
        let request_id = format!("browse-{}", new_request_token());
        self.pending_request = Some(request_id.clone());
        Payload::FileBrowseRequest(FileBrowseRequest {
            request_id,
            path,
            include_thumbnails: false,
        })
    }

    pub fn navigate_to(&mut self, path: String) -> Payload {
        self.history.push(self.current_path.clone());
        self.current_path = path.clone();
        self.selected_path = None;
        self.request_browse(path)
    }

    pub fn navigate_back(&mut self) -> Option<Payload> {
        let prev = self.history.pop()?;
        self.current_path = prev.clone();
        self.selected_path = None;
        Some(self.request_browse(prev))
    }
}

fn new_request_token() -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or_default();
    format!("{micros}-{}", std::process::id())
}
