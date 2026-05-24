use std::time::{Duration, Instant};

use androidconnect_protocol::DndMode;

use crate::status::DesktopStatus;

const PENDING_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
pub struct PhoneState {
    pub pending_volume: Option<u8>,
    pub pending_volume_since: Option<Instant>,
    pub pending_dnd: Option<DndMode>,
    pub pending_dnd_since: Option<Instant>,
    pub pending_bluetooth: Option<bool>,
    pub pending_bluetooth_since: Option<Instant>,
    pub error: Option<String>,
}

impl PhoneState {
    pub fn reconcile(&mut self, status: &DesktopStatus) {
        if let Some(volume) = status.volume_percent
            && self.pending_volume == Some(volume)
        {
            self.pending_volume = None;
            self.pending_volume_since = None;
            self.error = None;
        }
        if let Some(mode) = status.dnd_mode
            && self.pending_dnd == Some(mode)
        {
            self.pending_dnd = None;
            self.pending_dnd_since = None;
            self.error = None;
        }
        if let Some(enabled) = status.bluetooth_enabled
            && self.pending_bluetooth == Some(enabled)
        {
            self.pending_bluetooth = None;
            self.pending_bluetooth_since = None;
            self.error = None;
        }
    }

    pub fn expire_pending(&mut self) {
        if pending_expired(self.pending_volume_since) {
            self.pending_volume = None;
            self.pending_volume_since = None;
            self.error = Some("Volume change was not confirmed by Android.".to_owned());
        }
        if pending_expired(self.pending_dnd_since) {
            self.pending_dnd = None;
            self.pending_dnd_since = None;
            self.error = Some(
                "Do Not Disturb change was not confirmed. Grant Do Not Disturb access and try again."
                    .to_owned(),
            );
        }
        if pending_expired(self.pending_bluetooth_since) {
            self.pending_bluetooth = None;
            self.pending_bluetooth_since = None;
            self.error = Some("Bluetooth change was not confirmed by Android.".to_owned());
        }
    }
}

fn pending_expired(started: Option<Instant>) -> bool {
    started.is_some_and(|s| s.elapsed() >= PENDING_TIMEOUT)
}
