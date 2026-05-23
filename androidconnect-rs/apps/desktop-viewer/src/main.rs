mod app;
mod clipboard;
mod config;
mod network;
mod panels;
mod status;
mod streaming;
mod trust;

use std::sync::mpsc;
use std::thread;

use anyhow::Result;
use fluent_app::FluentApp;
use gpui::AppContext as _;
use log::error;

use crate::app::AppModel;
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
    let (event_tx, event_rx) = mpsc::channel::<network::DesktopEvent>();

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
            event_tx,
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

    let bind = config.bind.clone();
    let pairing_code = config.pairing_code.clone();
    let desktop_id = desktop_identity.desktop_id.clone();
    let desktop_name = desktop_identity.desktop_name.clone();

    FluentApp::new("AndroidConnect")
        .window_size(960.0, 720.0)
        .run(move |cx| {
            let model = cx.new(|_| {
                AppModel::new(
                    command_tx.clone(),
                    bind,
                    pairing_code,
                    desktop_id,
                    desktop_name,
                )
            });

            let model_weak = model.downgrade();
            cx.spawn({
                let model_weak = model_weak.clone();
                async move |cx| loop {
                    while let Ok(frame) = frame_rx.try_recv() {
                        model_weak
                            .update(&mut cx.clone(), |m, cx| m.push_frame(frame, cx))
                            .ok();
                    }
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(5))
                        .await;
                }
            })
            .detach();

            cx.spawn({
                let model_weak = model_weak.clone();
                async move |cx| loop {
                    while let Ok(s) = status_rx.try_recv() {
                        model_weak
                            .update(&mut cx.clone(), |m, cx| m.push_status(s, cx))
                            .ok();
                    }
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                }
            })
            .detach();

            cx.spawn({
                let model_weak = model_weak.clone();
                async move |cx| loop {
                    while let Ok(ev) = event_rx.try_recv() {
                        model_weak
                            .update(&mut cx.clone(), |m, cx| m.push_event(ev, cx))
                            .ok();
                    }
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                }
            })
            .detach();

            model
        });

    Ok(())
}
