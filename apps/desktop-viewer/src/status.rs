use androidconnect_protocol::{
    DndMode, FeatureStatus, MediaControlAction, MediaPlaybackState, StorageBreakdown,
    UtilityFeature, normalize_pairing_code,
};

use crate::network;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaInfo {
    pub app_name: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub playback_state: MediaPlaybackState,
    pub supported_actions: Vec<MediaControlAction>,
}

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
    pub charging: Option<bool>,
    pub feature_summary: Option<String>,
    pub features: Vec<FeatureStatus>,
    pub storage: Option<StorageBreakdown>,
    pub storage_status: Option<FeatureStatus>,
    pub media_summary: Option<String>,
    pub media_info: Option<MediaInfo>,
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
            charging: None,
            feature_summary: None,
            features: Vec::new(),
            storage: None,
            storage_status: None,
            media_summary: None,
            media_info: None,
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
                self.charging = None;
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
                self.charging = None;
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
                self.charging = None;
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
                features,
                wifi_summary,
                bluetooth_enabled,
                dnd_mode,
                volume_percent,
            } => {
                self.charging = charging;
                self.battery_status = battery_percent.map(|percent| {
                    if charging.unwrap_or(false) {
                        format!("{percent}% charging")
                    } else {
                        format!("{percent}%")
                    }
                });
                self.feature_summary = Some(feature_summary);
                self.features = features;
                self.wifi_summary = wifi_summary;
                self.bluetooth_enabled = bluetooth_enabled;
                self.dnd_mode = dnd_mode;
                self.volume_percent = volume_percent;
                self.connection = ConnectionState::Connected;
            }
            network::NetworkStatus::MediaStatus {
                active,
                summary,
                app_name,
                title,
                artist,
                playback_state,
                supported_actions,
            } => {
                self.media_summary = Some(if active {
                    summary
                } else if summary.is_empty() {
                    "no media".to_owned()
                } else {
                    summary
                });
                self.media_info = if active {
                    Some(MediaInfo {
                        app_name,
                        title,
                        artist,
                        playback_state,
                        supported_actions,
                    })
                } else {
                    None
                };
            }
            network::NetworkStatus::Storage { primary, status } => {
                self.storage = Some(primary);
                self.storage_status = Some(status);
                self.connection = ConnectionState::Connected;
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

    pub fn feature(&self, kind: UtilityFeature) -> Option<&FeatureStatus> {
        self.features.iter().find(|f| f.feature == kind)
    }

    fn clear_utility_status(&mut self) {
        self.battery_status = None;
        self.feature_summary = None;
        self.features.clear();
        self.storage = None;
        self.storage_status = None;
        self.media_summary = None;
        self.media_info = None;
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
}
