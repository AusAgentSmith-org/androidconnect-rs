use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText, Slider};

use androidconnect_protocol::{AudioControl, AudioControlCommand, DndMode, Payload};

use crate::status::DesktopStatus;

const PENDING_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
pub struct PhoneState {
    /// Optimistic local value while we wait for DeviceStatus confirmation.
    pub pending_volume: Option<u8>,
    pub pending_volume_since: Option<Instant>,
    pub pending_dnd: Option<DndMode>,
    pub pending_dnd_since: Option<Instant>,
    pub pending_bluetooth: Option<bool>,
    pub pending_bluetooth_since: Option<Instant>,
    pub error: Option<String>,
}

pub struct PendingActions {
    pub payloads: Vec<Payload>,
}

pub fn draw(ui: &mut egui::Ui, status: &DesktopStatus, state: &mut PhoneState) -> PendingActions {
    let mut pending: Vec<Payload> = Vec::new();

    ui.label(RichText::new("Quick settings").heading());
    ui.separator();
    ui.add_space(10.0);

    // Reconcile pending values against the latest DeviceStatus.
    if let Some(volume) = status.volume_percent
        && state.pending_volume == Some(volume)
    {
        state.pending_volume = None;
        state.pending_volume_since = None;
        state.error = None;
    }
    if let Some(mode) = status.dnd_mode
        && state.pending_dnd == Some(mode)
    {
        state.pending_dnd = None;
        state.pending_dnd_since = None;
        state.error = None;
    }
    if let Some(enabled) = status.bluetooth_enabled
        && state.pending_bluetooth == Some(enabled)
    {
        state.pending_bluetooth = None;
        state.pending_bluetooth_since = None;
        state.error = None;
    }
    expire_pending(state);

    // Media volume slider.
    let mut volume = state.pending_volume.or(status.volume_percent).unwrap_or(0);
    let prev_volume = volume;
    ui.horizontal(|ui| {
        ui.label("Media volume");
        if state.pending_volume.is_some() {
            ui.add(egui::Spinner::new().size(14.0));
        }
    });
    if ui
        .add(Slider::new(&mut volume, 0..=100).suffix("%"))
        .changed()
        && volume != prev_volume
    {
        state.pending_volume = Some(volume);
        state.pending_volume_since = Some(Instant::now());
        state.error = None;
        pending.push(Payload::AudioControl(AudioControl {
            command: AudioControlCommand::SetVolume { percent: volume },
        }));
    }
    ui.add_space(12.0);

    // DND segmented control.
    let current_dnd = state.pending_dnd.or(status.dnd_mode);
    ui.horizontal(|ui| {
        ui.label("Do not disturb");
        if state.pending_dnd.is_some() {
            ui.add(egui::Spinner::new().size(14.0));
        }
    });
    ui.horizontal(|ui| {
        for mode in [
            DndMode::Off,
            DndMode::Priority,
            DndMode::Alarms,
            DndMode::TotalSilence,
        ] {
            let selected = current_dnd == Some(mode);
            if ui.selectable_label(selected, dnd_label(mode)).clicked() && !selected {
                state.pending_dnd = Some(mode);
                state.pending_dnd_since = Some(Instant::now());
                state.error = None;
                pending.push(Payload::AudioControl(AudioControl {
                    command: AudioControlCommand::SetDnd { mode },
                }));
            }
        }
    });
    ui.add_space(12.0);

    // Bluetooth toggle.
    let bt_enabled = state.pending_bluetooth.or(status.bluetooth_enabled);
    ui.horizontal(|ui| {
        ui.label("Bluetooth");
        if state.pending_bluetooth.is_some() {
            ui.add(egui::Spinner::new().size(14.0));
        }
    });
    let current_label = match bt_enabled {
        Some(true) => "On",
        Some(false) => "Off",
        None => "Unknown",
    };
    if ui
        .button(current_label)
        .on_hover_text(
            "On Android 13+ the OS sends you to the Bluetooth settings page instead of toggling \
             directly.",
        )
        .clicked()
    {
        let next = !bt_enabled.unwrap_or(false);
        state.pending_bluetooth = Some(next);
        state.pending_bluetooth_since = Some(Instant::now());
        state.error = None;
        pending.push(Payload::AudioControl(AudioControl {
            command: AudioControlCommand::SetBluetooth { enabled: next },
        }));
    }

    if let Some(error) = &state.error {
        ui.add_space(12.0);
        ui.colored_label(Color32::RED, error);
    }

    ui.add_space(20.0);
    ui.separator();
    ui.label(
        RichText::new(
            "Quick settings reflect the latest DeviceStatus from the phone. A spinner means a \
             change is pending Android-side confirmation.",
        )
        .small()
        .italics(),
    );

    PendingActions { payloads: pending }
}

fn expire_pending(state: &mut PhoneState) {
    if pending_expired(state.pending_volume_since) {
        state.pending_volume = None;
        state.pending_volume_since = None;
        state.error = Some("Volume change was not confirmed by Android.".to_owned());
    }
    if pending_expired(state.pending_dnd_since) {
        state.pending_dnd = None;
        state.pending_dnd_since = None;
        state.error = Some(
            "Do Not Disturb change was not confirmed. Grant Do Not Disturb access and try again."
                .to_owned(),
        );
    }
    if pending_expired(state.pending_bluetooth_since) {
        state.pending_bluetooth = None;
        state.pending_bluetooth_since = None;
        state.error = Some("Bluetooth change was not confirmed by Android.".to_owned());
    }
}

fn pending_expired(started: Option<Instant>) -> bool {
    started.is_some_and(|started| started.elapsed() >= PENDING_TIMEOUT)
}

fn dnd_label(mode: DndMode) -> &'static str {
    match mode {
        DndMode::Off => "Off",
        DndMode::Priority => "Priority",
        DndMode::Alarms => "Alarms",
        DndMode::TotalSilence => "Silence",
    }
}
