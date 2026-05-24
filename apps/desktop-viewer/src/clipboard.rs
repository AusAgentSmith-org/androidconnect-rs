use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

use androidconnect_protocol::{ClipboardSource, ClipboardText, Payload};
use arboard::Clipboard;
use log::{debug, info, warn};

use crate::network::DesktopCommand;

const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Spawn a thread that owns the system clipboard handle.
///
/// The thread polls the local clipboard for changes and emits
/// `Payload::ClipboardText { source: Desktop, … }` through `command_tx` whenever the
/// content shifts. It also accepts incoming text from `apply_rx` (typically forwarded
/// from `Payload::ClipboardText { source: Android, … }` arrivals) and writes that
/// text into the local clipboard.
///
/// Feedback is suppressed by tracking the last text we wrote or observed: when the
/// next poll reads back identical content, we skip emitting it.
///
/// `arboard::Clipboard` is `!Send`, so the handle never leaves this thread.
pub fn spawn(command_tx: SyncSender<DesktopCommand>) -> Sender<String> {
    let (apply_tx, apply_rx) = mpsc::channel::<String>();
    thread::spawn(move || run(command_tx, apply_rx));
    apply_tx
}

fn run(command_tx: SyncSender<DesktopCommand>, apply_rx: Receiver<String>) {
    let mut clipboard = match Clipboard::new() {
        Ok(cb) => cb,
        Err(err) => {
            warn!("clipboard sync disabled: failed to open system clipboard: {err}");
            // Drain apply messages so senders don't block.
            while apply_rx.recv().is_ok() {}
            return;
        }
    };

    info!("clipboard sync enabled (poll interval {POLL_INTERVAL:?})");

    let mut sequence: u64 = 0;
    let mut last_text: Option<String> = clipboard.get_text().ok();

    loop {
        // Apply any pending remote text before polling so the poll sees our own write.
        while let Ok(remote) = apply_rx.try_recv() {
            if last_text.as_deref() == Some(remote.as_str()) {
                continue;
            }
            if let Err(err) = clipboard.set_text(remote.clone()) {
                warn!("clipboard set_text failed: {err}");
            } else {
                debug!(
                    "clipboard updated from desktop receive ({} chars)",
                    remote.chars().count()
                );
                last_text = Some(remote);
            }
        }

        match clipboard.get_text() {
            Ok(text) => {
                if text.is_empty() {
                    // ignore empty clipboards
                } else if last_text.as_deref() != Some(text.as_str()) {
                    sequence = sequence.wrapping_add(1);
                    let payload = Payload::ClipboardText(ClipboardText {
                        sequence,
                        text: text.clone(),
                        source: ClipboardSource::Desktop,
                    });
                    match command_tx.try_send(DesktopCommand::Utility(payload)) {
                        Ok(()) => debug!(
                            "clipboard outbound: seq={} chars={}",
                            sequence,
                            text.chars().count()
                        ),
                        Err(TrySendError::Full(_)) => {
                            warn!("clipboard outbound: command channel full, dropping update")
                        }
                        Err(TrySendError::Disconnected(_)) => {
                            warn!("clipboard outbound: command channel closed, exiting");
                            return;
                        }
                    }
                    last_text = Some(text);
                }
            }
            Err(arboard::Error::ContentNotAvailable) => {
                // No clipboard yet, or content is non-text — fine.
            }
            Err(err) => {
                debug!("clipboard get_text: {err}");
            }
        }

        thread::sleep(POLL_INTERVAL);
    }
}
