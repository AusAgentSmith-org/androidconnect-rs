use androidconnect_protocol::{DndMode, normalize_pairing_code};

use crate::network;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Listening,
    Connected,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopStatus {
    pub bind: String,
    pub pairing_code: String,
    pub connection: ConnectionState,
    pub peer: Option<String>,
    pub device_name: Option<String>,
    pub input_authenticated: bool,
    pub video_format: Option<String>,
    pub last_pong_nonce: Option<u64>,
    pub last_error: Option<String>,
    pub battery_status: Option<String>,
    pub feature_summary: Option<String>,
    pub media_summary: Option<String>,
    pub notification_summary: Option<String>,
    pub transfer_summary: Option<String>,
    pub file_browser_summary: Option<String>,
    pub photo_summary: Option<String>,
    pub message_summary: Option<String>,
    pub call_summary: Option<String>,
    pub relay_summary: Option<String>,
    pub client_summary: Option<String>,
    pub clipboard_summary: Option<String>,
    pub wifi_summary: Option<String>,
    pub bluetooth_enabled: Option<bool>,
    pub dnd_mode: Option<DndMode>,
    pub volume_percent: Option<u8>,
}

impl DesktopStatus {
    pub fn new(bind: String, pairing_code: String) -> Self {
        Self {
            bind,
            pairing_code,
            connection: ConnectionState::Listening,
            peer: None,
            device_name: None,
            input_authenticated: false,
            video_format: None,
            last_pong_nonce: None,
            last_error: None,
            battery_status: None,
            feature_summary: None,
            media_summary: None,
            notification_summary: None,
            transfer_summary: None,
            file_browser_summary: None,
            photo_summary: None,
            message_summary: None,
            call_summary: None,
            relay_summary: None,
            client_summary: None,
            clipboard_summary: None,
            wifi_summary: None,
            bluetooth_enabled: None,
            dnd_mode: None,
            volume_percent: None,
        }
    }

    pub fn apply(&mut self, status: network::NetworkStatus) {
        match status {
            network::NetworkStatus::Listening { bind } => {
                self.bind = bind;
                self.connection = ConnectionState::Listening;
                self.peer = None;
                self.device_name = None;
                self.input_authenticated = false;
                self.video_format = None;
                self.last_pong_nonce = None;
                self.last_error = None;
                self.clear_utility_status();
            }
            network::NetworkStatus::ClientConnected { peer } => {
                self.connection = ConnectionState::Connected;
                self.peer = Some(peer);
                self.device_name = None;
                self.input_authenticated = false;
                self.video_format = None;
                self.last_pong_nonce = None;
                self.last_error = None;
                self.clear_utility_status();
            }
            network::NetworkStatus::DeviceHello { device_name } => {
                self.device_name = Some(device_name);
                self.connection = ConnectionState::Connected;
                self.last_error = None;
            }
            network::NetworkStatus::PairingAuthenticated { .. } => {
                self.input_authenticated = true;
                self.connection = ConnectionState::Connected;
                self.last_error = None;
            }
            network::NetworkStatus::PairingRejected { message } => {
                self.input_authenticated = false;
                self.connection = ConnectionState::Connected;
                self.last_error = Some(format!("pairing rejected: {message}"));
            }
            network::NetworkStatus::VideoFormat {
                width,
                height,
                frame_rate,
                rotation_degrees,
            } => {
                self.video_format = Some(format!(
                    "{width}x{height}@{frame_rate}fps rot {rotation_degrees}"
                ));
                self.connection = ConnectionState::Connected;
            }
            network::NetworkStatus::HeartbeatPong { nonce } => {
                self.last_pong_nonce = Some(nonce);
                self.connection = ConnectionState::Connected;
                self.last_error = None;
            }
            network::NetworkStatus::ClientDisconnected => {
                self.connection = ConnectionState::Listening;
                self.peer = None;
                self.device_name = None;
                self.input_authenticated = false;
                self.video_format = None;
                self.last_pong_nonce = None;
                self.last_error = None;
                self.clear_utility_status();
            }
            network::NetworkStatus::ClientError { message } => {
                self.connection = ConnectionState::Error;
                self.input_authenticated = false;
                self.last_pong_nonce = None;
                self.last_error = Some(message);
            }
            network::NetworkStatus::DeviceStatus {
                battery_percent,
                charging,
                feature_summary,
                wifi_summary,
                bluetooth_enabled,
                dnd_mode,
                volume_percent,
            } => {
                self.battery_status = battery_percent.map(|percent| {
                    if charging.unwrap_or(false) {
                        format!("{percent}% charging")
                    } else {
                        format!("{percent}%")
                    }
                });
                self.feature_summary = Some(feature_summary);
                self.wifi_summary = wifi_summary;
                self.bluetooth_enabled = bluetooth_enabled;
                self.dnd_mode = dnd_mode;
                self.volume_percent = volume_percent;
                self.connection = ConnectionState::Connected;
            }
            network::NetworkStatus::MediaStatus { active, summary } => {
                self.media_summary = Some(if active {
                    summary
                } else if summary.is_empty() {
                    "no media".to_owned()
                } else {
                    summary
                });
            }
            network::NetworkStatus::ClipboardText { source, characters } => {
                self.clipboard_summary = Some(format!("{source:?} clipboard {characters} chars"));
            }
            network::NetworkStatus::NotificationPosted {
                app_name,
                title,
                sensitive,
            } => {
                self.notification_summary = Some(if sensitive {
                    format!("{app_name}: hidden")
                } else {
                    format!(
                        "{app_name}: {}",
                        title.unwrap_or_else(|| "notification".to_owned())
                    )
                });
            }
            network::NetworkStatus::NotificationRemoved { notification_id } => {
                self.notification_summary = Some(format!(
                    "notification removed {}",
                    truncate_title(&notification_id, 24)
                ));
            }
            network::NetworkStatus::FileTransfer {
                transfer_id: _,
                file_name,
                bytes,
                status,
            } => {
                self.transfer_summary = Some(format!(
                    "{} {} {}",
                    truncate_title(&file_name, 32),
                    format_bytes_short(bytes),
                    status
                ));
            }
            network::NetworkStatus::FileBrowse {
                path,
                entries,
                state,
            } => {
                self.file_browser_summary = Some(format!("{path}: {entries} entries {state}"));
            }
            network::NetworkStatus::PhotoAssets { count, state } => {
                self.photo_summary = Some(format!("{count} media assets {state}"));
            }
            network::NetworkStatus::MessageThreads { count, state } => {
                self.message_summary = Some(format!("{count} message threads {state}"));
            }
            network::NetworkStatus::CallState { state, status } => {
                self.call_summary = Some(format!("call {state} {status}"));
            }
            network::NetworkStatus::RelayStatus {
                enabled,
                connected,
                state,
            } => {
                self.relay_summary = Some(format!(
                    "relay {} {} {state}",
                    if enabled { "on" } else { "off" },
                    if connected { "connected" } else { "idle" }
                ));
            }
            network::NetworkStatus::ClientList {
                clients,
                input_owner,
            } => {
                self.client_summary = Some(format!(
                    "{clients} client{}{}",
                    if clients == 1 { "" } else { "s" },
                    input_owner
                        .map(|owner| format!(" owner {}", truncate_title(&owner, 16)))
                        .unwrap_or_default()
                ));
            }
        }
    }

    pub fn window_title(&self) -> String {
        let subject = self
            .device_name
            .as_deref()
            .or(self.peer.as_deref())
            .unwrap_or(self.bind.as_str());
        let auth = if self.input_authenticated {
            "input paired"
        } else {
            "pairing required"
        };

        let state = match self.connection {
            ConnectionState::Listening => format!("listening on {}", self.bind),
            ConnectionState::Connected => {
                let mut state = format!("connected to {subject} - {auth}");
                if let Some(video) = &self.video_format {
                    state.push_str(" - ");
                    state.push_str(video);
                }
                if let Some(nonce) = self.last_pong_nonce {
                    state.push_str(&format!(" - heartbeat ok #{nonce}"));
                }
                if let Some(utilities) = self.utility_summary() {
                    state.push_str(" - ");
                    state.push_str(&truncate_title(&utilities, 120));
                }
                state
            }
            ConnectionState::Error => {
                let message = self.last_error.as_deref().unwrap_or("connection error");
                format!("error: {}", truncate_title(message, 72))
            }
        };

        format!(
            "AndroidConnect - code {} - {state}",
            format_pairing_code(&self.pairing_code)
        )
    }

    fn clear_utility_status(&mut self) {
        self.battery_status = None;
        self.feature_summary = None;
        self.media_summary = None;
        self.notification_summary = None;
        self.transfer_summary = None;
        self.file_browser_summary = None;
        self.photo_summary = None;
        self.message_summary = None;
        self.call_summary = None;
        self.relay_summary = None;
        self.client_summary = None;
        self.clipboard_summary = None;
        self.wifi_summary = None;
        self.bluetooth_enabled = None;
        self.dnd_mode = None;
        self.volume_percent = None;
    }

    fn utility_summary(&self) -> Option<String> {
        let parts = [
            self.battery_status.as_deref(),
            self.feature_summary.as_deref(),
            self.media_summary.as_deref(),
            self.notification_summary.as_deref(),
            self.transfer_summary.as_deref(),
            self.file_browser_summary.as_deref(),
            self.photo_summary.as_deref(),
            self.message_summary.as_deref(),
            self.call_summary.as_deref(),
            self.relay_summary.as_deref(),
            self.client_summary.as_deref(),
            self.clipboard_summary.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .take(5)
        .collect::<Vec<_>>();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" - "))
        }
    }
}

pub fn format_pairing_code(code: &str) -> String {
    let clean = normalize_pairing_code(code);
    if clean.len() == 6 && clean.chars().all(|ch| ch.is_ascii_digit()) {
        format!("{} {}", &clean[..3], &clean[3..])
    } else {
        clean
    }
}

pub fn format_bytes_short(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{}KiB", bytes / 1024)
    } else {
        format!("{}MiB", bytes / 1024 / 1024)
    }
}

pub fn truncate_title(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_six_digit_pairing_code_for_title() {
        assert_eq!(format_pairing_code("123456"), "123 456");
        assert_eq!(format_pairing_code(" ab cd "), "ABCD");
    }

    #[test]
    fn status_title_tracks_pairing_and_video() {
        let mut status = DesktopStatus::new("0.0.0.0:48172".to_owned(), "123456".to_owned());
        assert_eq!(
            status.window_title(),
            "AndroidConnect - code 123 456 - listening on 0.0.0.0:48172"
        );

        status.apply(network::NetworkStatus::DeviceHello {
            device_name: "Pixel".to_owned(),
        });
        assert_eq!(
            status.window_title(),
            "AndroidConnect - code 123 456 - connected to Pixel - pairing required"
        );

        status.apply(network::NetworkStatus::PairingAuthenticated {
            trusted: false,
            session_key_fingerprint: None,
        });
        status.apply(network::NetworkStatus::VideoFormat {
            width: 1080,
            height: 2340,
            frame_rate: 30,
            rotation_degrees: 0,
        });
        assert_eq!(
            status.window_title(),
            "AndroidConnect - code 123 456 - connected to Pixel - input paired - 1080x2340@30fps rot 0"
        );

        status.apply(network::NetworkStatus::HeartbeatPong { nonce: 7 });
        assert_eq!(
            status.window_title(),
            "AndroidConnect - code 123 456 - connected to Pixel - input paired - 1080x2340@30fps rot 0 - heartbeat ok #7"
        );
    }
}
