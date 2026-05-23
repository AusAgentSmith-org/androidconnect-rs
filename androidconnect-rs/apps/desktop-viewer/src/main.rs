mod app;
mod clipboard;
mod config;
mod network;
mod status;
mod streaming;
mod trust;

use std::sync::mpsc;
use std::thread;

use anyhow::Result;
use eframe::NativeOptions;
use log::error;

use crate::app::{App, shell_window_title};
use crate::config::parse_config;
use crate::streaming::RgbaFrame;

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config = parse_config(std::env::args().skip(1))?;
    let trust_store = trust::TrustStore::load_or_create()?;
    let trust_store_path = trust_store.path().to_path_buf();
    let desktop_identity = trust_store.identity();

    let (frame_tx, frame_rx) = mpsc::sync_channel::<RgbaFrame>(2);
    let (command_tx, command_rx) = mpsc::sync_channel::<network::DesktopCommand>(1024);
    let (status_tx, status_rx) = mpsc::channel::<network::NetworkStatus>();

    let clipboard_apply_tx = clipboard::spawn(command_tx.clone());

    let bind_for_thread = config.bind.clone();
    let pairing_code_for_thread = config.pairing_code.clone();
    let trust_store_path_for_thread = trust_store_path.clone();
    let status_tx_for_error = status_tx.clone();
    thread::spawn(move || {
        if let Err(e) = network::run(
            &bind_for_thread,
            frame_tx,
            command_rx,
            pairing_code_for_thread,
            trust_store_path_for_thread,
            status_tx,
            clipboard_apply_tx,
        ) {
            error!("network thread: {e:#}");
            let _ = status_tx_for_error.send(network::NetworkStatus::ClientError {
                message: e.to_string(),
            });
        }
    });

    log::info!("androidconnect desktop viewer — binding {}", config.bind);
    log::info!("pairing code: {}", config.pairing_code);
    log::info!(
        "desktop identity: {} ({})",
        desktop_identity.desktop_name,
        desktop_identity.desktop_id
    );
    log::info!("desktop trust store: {}", trust_store_path.display());

    let bind = config.bind.clone();
    let pairing_code = config.pairing_code.clone();

    let options = NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(shell_window_title())
            .with_inner_size([960.0, 720.0])
            .with_min_inner_size([640.0, 480.0]),
        ..NativeOptions::default()
    };

    eframe::run_native(
        shell_window_title(),
        options,
        Box::new(move |_cc| {
            Ok(Box::new(App::new(
                frame_rx,
                command_tx,
                status_rx,
                bind,
                pairing_code,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))?;

    Ok(())
}
