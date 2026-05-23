use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use androidconnect_protocol::{
    AUTH_CHALLENGE_BYTES, AuthMethod, AuthResponse, ClipboardSource, DndMode, Envelope,
    FeatureStatus, FileBrowseResponse, FileTransferChunk, FileTransferComplete, FileTransferStart,
    InputEvent, MAX_VIDEO_FRAME_BYTES, MediaControlAction, MediaPlaybackState, MediaStatus,
    MessageEvent, MessageSendResponse, MessageThreadDetail, MessageThreadList, NotificationPosted,
    NotificationRemoved, PAIRED_SECRET_BYTES, PROTOCOL_VERSION, Payload, TransferDirection,
    TransferStatus, WireError, bytes_to_hex, derive_session_key, paired_secret_from_pairing_code,
    pairing_auth_response, read_length_prefixed, session_key_fingerprint,
    trusted_session_auth_response, write_length_prefixed,
};
use anyhow::{Result, bail};
use log::{error, info, warn};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;

use crate::RgbaFrame;
use crate::trust::TrustStore;

const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkStatus {
    Listening {
        bind: String,
    },
    ClientConnected {
        peer: String,
    },
    DeviceHello {
        device_name: String,
    },
    PairingAuthenticated {
        trusted: bool,
        session_key_fingerprint: Option<String>,
    },
    PairingRejected {
        message: String,
    },
    VideoFormat {
        width: u32,
        height: u32,
        frame_rate: u32,
        rotation_degrees: u16,
    },
    HeartbeatPong {
        nonce: u64,
    },
    ClientDisconnected,
    ClientError {
        message: String,
    },
    DeviceStatus {
        battery_percent: Option<u8>,
        charging: Option<bool>,
        feature_summary: String,
        features: Vec<FeatureStatus>,
        wifi_summary: Option<String>,
        bluetooth_enabled: Option<bool>,
        dnd_mode: Option<DndMode>,
        volume_percent: Option<u8>,
    },
    MediaStatus {
        active: bool,
        summary: String,
        app_name: Option<String>,
        title: Option<String>,
        artist: Option<String>,
        playback_state: MediaPlaybackState,
        supported_actions: Vec<MediaControlAction>,
    },
    ClipboardText {
        source: ClipboardSource,
        characters: usize,
    },
    NotificationPosted {
        app_name: String,
        title: Option<String>,
        sensitive: bool,
    },
    NotificationRemoved {
        notification_id: String,
    },
    FileTransfer {
        transfer_id: String,
        file_name: String,
        bytes: u64,
        status: String,
    },
    FileBrowse {
        path: String,
        entries: usize,
        state: String,
    },
    PhotoAssets {
        count: usize,
        state: String,
    },
    MessageThreads {
        count: usize,
        state: String,
    },
    CallState {
        state: String,
        status: String,
    },
    RelayStatus {
        enabled: bool,
        connected: bool,
        state: String,
    },
    ClientList {
        clients: usize,
        input_owner: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopCommand {
    Input(InputEvent),
    Utility(Payload),
}

/// Structured copies of inbound protocol payloads that the UI panels consume.
/// `NetworkStatus` carries summary strings for the status bar; this enum carries the
/// data the panels actually render.
#[derive(Debug, Clone)]
pub enum DesktopEvent {
    NotificationPosted(NotificationPosted),
    NotificationRemoved(NotificationRemoved),
    FileBrowseResponse(FileBrowseResponse),
    MessageThreadList(MessageThreadList),
    MessageEvent(MessageEvent),
    MessageThreadDetail(MessageThreadDetail),
    MessageSendResponse(MessageSendResponse),
    SessionLost,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    bind: &str,
    frame_sender: SyncSender<RgbaFrame>,
    command_rx: Receiver<DesktopCommand>,
    pairing_code: String,
    trust_store_path: PathBuf,
    status_tx: Sender<NetworkStatus>,
    clipboard_apply_tx: Sender<String>,
    event_tx: Sender<DesktopEvent>,
    download_destinations: Arc<Mutex<HashMap<String, PathBuf>>>,
) -> Result<()> {
    let listener = TcpListener::bind(bind)?;
    info!("listening on {bind}");
    send_status(
        &status_tx,
        NetworkStatus::Listening {
            bind: bind.to_owned(),
        },
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let peer = stream
                    .peer_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|_| "unknown peer".to_owned());
                info!("client connected: {peer:?}");
                send_status(
                    &status_tx,
                    NetworkStatus::ClientConnected { peer: peer.clone() },
                );
                let trust_store = TrustStore::load_or_create_at(&trust_store_path)?;
                drain_stale_commands(&command_rx);
                let result = handle_client(
                    stream,
                    frame_sender.clone(),
                    &command_rx,
                    &pairing_code,
                    trust_store,
                    status_tx.clone(),
                    clipboard_apply_tx.clone(),
                    event_tx.clone(),
                    download_destinations.clone(),
                );
                let _ = event_tx.send(DesktopEvent::SessionLost);
                if let Err(e) = result {
                    if is_clean_disconnect(&e) {
                        info!("client disconnected");
                        send_status(&status_tx, NetworkStatus::ClientDisconnected);
                    } else {
                        error!("client error: {e:#}");
                        send_status(
                            &status_tx,
                            NetworkStatus::ClientError {
                                message: e.to_string(),
                            },
                        );
                    }
                    drain_stale_commands(&command_rx);
                }
            }
            Err(e) => {
                error!("accept: {e}");
                send_status(
                    &status_tx,
                    NetworkStatus::ClientError {
                        message: format!("accept failed: {e}"),
                    },
                );
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_client(
    stream: TcpStream,
    frame_sender: SyncSender<RgbaFrame>,
    command_rx: &Receiver<DesktopCommand>,
    pairing_code: &str,
    trust_store: TrustStore,
    status_tx: Sender<NetworkStatus>,
    clipboard_apply_tx: Sender<String>,
    event_tx: Sender<DesktopEvent>,
    download_destinations: Arc<Mutex<HashMap<String, PathBuf>>>,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let writer = stream.try_clone()?;
    let (reader_done_tx, reader_done_rx) = mpsc::sync_channel(1);
    let (writer_command_tx, writer_command_rx) = mpsc::channel();
    let pairing_code = pairing_code.to_owned();

    std::thread::spawn(move || {
        let result = read_client_loop(
            stream,
            frame_sender,
            pairing_code,
            trust_store,
            writer_command_tx,
            status_tx,
            clipboard_apply_tx,
            event_tx,
            download_destinations,
        );
        let _ = reader_done_tx.send(result);
    });

    write_command_loop(writer, command_rx, reader_done_rx, writer_command_rx)
}

fn write_command_loop(
    mut stream: TcpStream,
    command_rx: &Receiver<DesktopCommand>,
    reader_done_rx: Receiver<Result<()>>,
    writer_command_rx: Receiver<WriterCommand>,
) -> Result<()> {
    write_command_loop_with_heartbeat(
        &mut stream,
        command_rx,
        reader_done_rx,
        writer_command_rx,
        HEARTBEAT_INTERVAL,
    )
}

fn write_command_loop_with_heartbeat(
    stream: &mut TcpStream,
    command_rx: &Receiver<DesktopCommand>,
    reader_done_rx: Receiver<Result<()>>,
    writer_command_rx: Receiver<WriterCommand>,
    heartbeat_interval: Duration,
) -> Result<()> {
    let mut sequence = 1_u64;
    let mut input_authenticated = false;
    let mut logged_unauthenticated_input = false;
    let mut last_heartbeat = Instant::now();

    loop {
        if let Ok(result) = reader_done_rx.try_recv() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return result;
        }
        drain_writer_commands(
            stream,
            &mut sequence,
            &writer_command_rx,
            &mut input_authenticated,
        )?;
        if heartbeat_interval > Duration::ZERO && last_heartbeat.elapsed() >= heartbeat_interval {
            write_ping(stream, &mut sequence)?;
            last_heartbeat = Instant::now();
        }

        match command_rx.recv_timeout(INPUT_POLL_INTERVAL) {
            Ok(command) => {
                drain_writer_commands(
                    stream,
                    &mut sequence,
                    &writer_command_rx,
                    &mut input_authenticated,
                )?;
                if input_authenticated {
                    write_desktop_command(stream, &mut sequence, command)?;
                } else if !logged_unauthenticated_input {
                    warn!("dropping desktop commands until pairing succeeds");
                    logged_unauthenticated_input = true;
                }
                while let Ok(command) = command_rx.try_recv() {
                    if input_authenticated {
                        write_desktop_command(stream, &mut sequence, command)?;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("desktop input channel closed"),
        }
    }
}

fn write_desktop_command(
    stream: &mut TcpStream,
    sequence: &mut u64,
    command: DesktopCommand,
) -> Result<()> {
    match command {
        DesktopCommand::Input(event) => write_payload(stream, sequence, Payload::Input(event)),
        DesktopCommand::Utility(payload) => {
            if !is_desktop_utility_payload(&payload) {
                bail!("payload is not a desktop utility command");
            }
            write_payload(stream, sequence, payload)
        }
    }
}

fn write_ping(stream: &mut TcpStream, sequence: &mut u64) -> Result<()> {
    let nonce = *sequence;
    write_payload(stream, sequence, Payload::Ping { nonce })
}

fn write_payload(stream: &mut TcpStream, sequence: &mut u64, payload: Payload) -> Result<()> {
    let envelope = Envelope::new(*sequence, payload);
    *sequence += 1;
    write_length_prefixed(stream, &envelope)?;
    Ok(())
}

fn drain_writer_commands(
    stream: &mut TcpStream,
    sequence: &mut u64,
    writer_command_rx: &Receiver<WriterCommand>,
    input_authenticated: &mut bool,
) -> Result<()> {
    loop {
        match writer_command_rx.try_recv() {
            Ok(WriterCommand::SendPayload(payload)) => {
                write_payload(stream, sequence, *payload)?;
            }
            Ok(WriterCommand::SetInputAuthenticated(accepted)) => {
                *input_authenticated = accepted;
            }
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Disconnected) => return Ok(()),
        }
    }
}

fn drain_stale_commands(command_rx: &Receiver<DesktopCommand>) {
    while command_rx.try_recv().is_ok() {}
}

fn is_desktop_utility_payload(payload: &Payload) -> bool {
    matches!(
        payload,
        Payload::MediaControl(_)
            | Payload::ClipboardText(_)
            | Payload::ClipboardImage(_)
            | Payload::FileTransferStart(_)
            | Payload::FileTransferChunk(_)
            | Payload::FileTransferComplete(_)
            | Payload::FileBrowseRequest(_)
            | Payload::FileMutation(_)
            | Payload::FileTransferRequest(_)
            | Payload::NotificationAction(_)
            | Payload::NotificationFilterUpdate(_)
            | Payload::AudioControl(_)
            | Payload::AppWindowOpen(_)
            | Payload::AppWindowClose(_)
            | Payload::AppWindowInput(_)
            | Payload::MessageSendRequest(_)
            | Payload::MessageThreadOpen(_)
            | Payload::CallAction(_)
            | Payload::PhotoAssetTransfer(_)
            | Payload::RelayOffer(_)
            | Payload::ClientRoleUpdate(_)
    )
}

#[allow(clippy::too_many_arguments)]
fn read_client_loop(
    mut stream: TcpStream,
    sender: SyncSender<RgbaFrame>,
    pairing_code: String,
    mut trust_store: TrustStore,
    writer_command_tx: Sender<WriterCommand>,
    status_tx: Sender<NetworkStatus>,
    clipboard_apply_tx: Sender<String>,
    event_tx: Sender<DesktopEvent>,
    download_destinations: Arc<Mutex<HashMap<String, PathBuf>>>,
) -> Result<()> {
    let mut decoder = Decoder::new().map_err(|e| anyhow::anyhow!("decoder init failed: {e:?}"))?;
    let desktop_identity = trust_store.identity();
    let mut current_device: Option<RemoteDevice> = None;
    let mut pending_auth: Option<PendingAuth> = None;
    let mut incoming_transfers: HashMap<String, DesktopIncomingTransfer> = HashMap::new();

    loop {
        let envelope = read_length_prefixed(&mut stream, MAX_VIDEO_FRAME_BYTES)?;

        if envelope.version != PROTOCOL_VERSION {
            bail!(
                "unsupported protocol version {}, expected {PROTOCOL_VERSION}",
                envelope.version
            );
        }

        match envelope.payload {
            Payload::Hello(hello) => {
                info!(
                    "hello from {} ({}) protocol={}",
                    hello.device_name, hello.device_id, hello.protocol_version
                );
                current_device = Some(RemoteDevice {
                    device_id: hello.device_id.clone(),
                    device_name: hello.device_name.clone(),
                });
                send_status(
                    &status_tx,
                    NetworkStatus::DeviceHello {
                        device_name: hello.device_name,
                    },
                );
            }
            Payload::VideoFormat(fmt) => {
                info!(
                    "video: {}x{} {:?} {}fps rotation={}°",
                    fmt.width, fmt.height, fmt.codec, fmt.frame_rate, fmt.rotation_degrees
                );
                send_status(
                    &status_tx,
                    NetworkStatus::VideoFormat {
                        width: fmt.width,
                        height: fmt.height,
                        frame_rate: fmt.frame_rate,
                        rotation_degrees: fmt.rotation_degrees,
                    },
                );
            }
            Payload::VideoFrame(frame) => {
                match decoder.decode(&frame.data) {
                    Ok(Some(yuv)) => {
                        let (w, h) = yuv.dimensions();
                        let mut rgba = vec![0u8; w * h * 4];
                        yuv.write_rgba8(&mut rgba);
                        // try_send: drop the frame if the render loop can't keep up,
                        // rather than queuing unboundedly and building lag.
                        let _ = sender.try_send(RgbaFrame {
                            width: w as u32,
                            height: h as u32,
                            data: rgba,
                        });
                    }
                    Ok(None) => {} // codec config or decoder buffering
                    Err(e) => warn!("decode error: {e:?}"),
                }
            }
            Payload::Ping { nonce } => info!("ping {nonce}"),
            Payload::Pong { nonce } => {
                info!("pong {nonce}");
                send_status(&status_tx, NetworkStatus::HeartbeatPong { nonce });
            }
            Payload::Error { message } => error!("peer error: {message}"),
            Payload::AuthChallenge(challenge) => {
                let device = current_device
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("auth challenge received before hello"))?;
                let auth_challenge = challenge.challenge;
                let trusted_pairing = trust_store.paired_android(&device.device_id);
                let (method, response, paired_secret) = if let Some(pairing) = trusted_pairing {
                    info!(
                        "using stored trust for {} ({})",
                        pairing.device_name, pairing.device_id
                    );
                    (
                        AuthMethod::TrustedSession,
                        trusted_session_auth_response(
                            &pairing.paired_secret,
                            &device.device_id,
                            &desktop_identity.desktop_id,
                            &auth_challenge,
                        ),
                        pairing.paired_secret,
                    )
                } else {
                    let paired_secret = paired_secret_from_pairing_code(
                        &pairing_code,
                        &device.device_id,
                        &desktop_identity.desktop_id,
                    );
                    (
                        AuthMethod::PairingCode,
                        pairing_auth_response(
                            &pairing_code,
                            &device.device_id,
                            &desktop_identity.desktop_id,
                            &auth_challenge,
                        ),
                        paired_secret,
                    )
                };
                let response = AuthResponse {
                    desktop_id: desktop_identity.desktop_id.clone(),
                    desktop_name: desktop_identity.desktop_name.clone(),
                    method,
                    response,
                };
                pending_auth = Some(PendingAuth {
                    device,
                    method,
                    challenge: auth_challenge,
                    paired_secret,
                });
                let _ = writer_command_tx.send(WriterCommand::SendPayload(Box::new(
                    Payload::AuthResponse(response),
                )));
            }
            Payload::AuthResult(result) => {
                if result.accepted {
                    let pending = pending_auth
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("auth result received without challenge"))?;
                    let session_key = derive_session_key(
                        &pending.paired_secret,
                        &pending.device.device_id,
                        &desktop_identity.desktop_id,
                        &pending.challenge,
                    );
                    let fingerprint = session_key_fingerprint(&session_key);
                    if let Some(peer_fingerprint) = result.session_key_fingerprint
                        && peer_fingerprint != fingerprint
                    {
                        bail!("session key fingerprint mismatch");
                    }
                    match pending.method {
                        AuthMethod::PairingCode => {
                            trust_store.store_pairing(
                                &pending.device.device_id,
                                &pending.device.device_name,
                                pending.paired_secret,
                            )?;
                            info!(
                                "stored trusted Android device {} ({})",
                                pending.device.device_name, pending.device.device_id
                            );
                        }
                        AuthMethod::TrustedSession => {
                            trust_store.mark_authenticated(
                                &pending.device.device_id,
                                &pending.device.device_name,
                            )?;
                        }
                    }
                    let trusted = matches!(pending.method, AuthMethod::TrustedSession);
                    info!(
                        "pairing authenticated via {:?}; desktop input enabled",
                        pending.method
                    );
                    send_status(
                        &status_tx,
                        NetworkStatus::PairingAuthenticated {
                            trusted,
                            session_key_fingerprint: Some(bytes_to_hex(&fingerprint)),
                        },
                    );
                } else {
                    pending_auth = None;
                    warn!("pairing rejected by Android: {}", result.message);
                    send_status(
                        &status_tx,
                        NetworkStatus::PairingRejected {
                            message: result.message.clone(),
                        },
                    );
                }
                let _ =
                    writer_command_tx.send(WriterCommand::SetInputAuthenticated(result.accepted));
            }
            Payload::AuthResponse(_) => {}
            Payload::Input(_) => {}
            Payload::DeviceStatus(status) => {
                info!(
                    "device_status: battery={:?}% charging={:?} interactive={:?} wifi={:?} bt={:?} dnd={:?} vol={:?} features={}",
                    status.battery_percent,
                    status.charging,
                    status.interactive,
                    status.wifi_state,
                    status.bluetooth_state,
                    status.dnd_state,
                    status.volume,
                    feature_summary(&status.features)
                );
                let wifi_summary = status.wifi_state.as_ref().map(|w| {
                    if w.connected {
                        match (&w.ssid, w.signal_strength) {
                            (Some(ssid), Some(rssi)) => format!("{ssid} ({rssi} dBm)"),
                            (Some(ssid), None) => ssid.clone(),
                            (None, _) => "connected".to_owned(),
                        }
                    } else {
                        "off".to_owned()
                    }
                });
                let bt_enabled = status.bluetooth_state.as_ref().map(|b| b.enabled);
                let dnd_mode = status.dnd_state.as_ref().map(|d| d.mode);
                let volume_percent = status.volume.as_ref().map(|v| v.media_percent);
                let features = status.features.clone();
                send_status(
                    &status_tx,
                    NetworkStatus::DeviceStatus {
                        battery_percent: status.battery_percent,
                        charging: status.charging,
                        feature_summary: feature_summary(&status.features),
                        features,
                        wifi_summary,
                        bluetooth_enabled: bt_enabled,
                        dnd_mode,
                        volume_percent,
                    },
                );
            }
            Payload::MediaStatus(status) => {
                send_status(
                    &status_tx,
                    NetworkStatus::MediaStatus {
                        active: status.active,
                        summary: media_summary(&status),
                        app_name: status.app_name.clone(),
                        title: status.title.clone(),
                        artist: status.artist.clone(),
                        playback_state: status.playback_state,
                        supported_actions: status.supported_actions.clone(),
                    },
                );
            }
            Payload::MediaControl(_) | Payload::ClipboardImage(_) => {}
            Payload::ClipboardText(clipboard) => {
                info!(
                    "clipboard_text: source={:?} chars={} preview={:?}",
                    clipboard.source,
                    clipboard.text.chars().count(),
                    clipboard.text.chars().take(40).collect::<String>()
                );
                send_status(
                    &status_tx,
                    NetworkStatus::ClipboardText {
                        source: clipboard.source,
                        characters: clipboard.text.chars().count(),
                    },
                );
                if matches!(clipboard.source, ClipboardSource::Android)
                    && !clipboard.text.is_empty()
                {
                    let _ = clipboard_apply_tx.send(clipboard.text);
                }
            }
            Payload::FileTransferStart(start) => {
                handle_incoming_file_start(
                    &mut incoming_transfers,
                    &status_tx,
                    start,
                    &download_destinations,
                )?;
            }
            Payload::FileTransferChunk(chunk) => {
                handle_incoming_file_chunk(&mut incoming_transfers, &status_tx, chunk)?;
            }
            Payload::FileTransferComplete(complete) => {
                handle_incoming_file_complete(&mut incoming_transfers, &status_tx, complete);
            }
            Payload::FileBrowseRequest(_)
            | Payload::FileMutation(_)
            | Payload::FileTransferRequest(_)
            | Payload::NotificationAction(_)
            | Payload::NotificationFilterUpdate(_)
            | Payload::AudioFormat(_)
            | Payload::AudioFrame(_)
            | Payload::AudioControl(_)
            | Payload::AppWindowOpen(_)
            | Payload::AppWindowClose(_)
            | Payload::AppWindowInput(_)
            | Payload::MessageSendRequest(_)
            | Payload::MessageThreadOpen(_)
            | Payload::CallAction(_)
            | Payload::PhotoAssetTransfer(_)
            | Payload::RelayOffer(_)
            | Payload::ClientRoleUpdate(_) => {}
            Payload::MessageEvent(message) => {
                info!(
                    "message_event: thread={} sender={} chars={}",
                    message.thread_id,
                    message.sender,
                    message.body.chars().count()
                );
                let _ = event_tx.send(DesktopEvent::MessageEvent(message));
            }
            Payload::MessageThreadDetail(detail) => {
                let _ = event_tx.send(DesktopEvent::MessageThreadDetail(detail));
            }
            Payload::MessageSendResponse(resp) => {
                info!(
                    "message_send_response: thread={:?} result={:?}",
                    resp.thread_id, resp.result
                );
                let _ = event_tx.send(DesktopEvent::MessageSendResponse(resp));
            }
            Payload::FileBrowseResponse(response) => {
                send_status(
                    &status_tx,
                    NetworkStatus::FileBrowse {
                        path: response.path.clone(),
                        entries: response.entries.len(),
                        state: format!("{:?}", response.status.state),
                    },
                );
                let _ = event_tx.send(DesktopEvent::FileBrowseResponse(response));
            }
            Payload::NotificationPosted(notification) => {
                info!(
                    "notification_posted: app={:?} pkg={:?} title={:?} sensitive={} actions={}",
                    notification.app_name,
                    notification.app_package,
                    notification.title,
                    notification.sensitive,
                    notification.actions.len()
                );
                send_status(
                    &status_tx,
                    NetworkStatus::NotificationPosted {
                        app_name: notification.app_name.clone(),
                        title: notification.title.clone(),
                        sensitive: notification.sensitive,
                    },
                );
                let _ = event_tx.send(DesktopEvent::NotificationPosted(notification));
            }
            Payload::NotificationRemoved(removed) => {
                info!("notification_removed: id={:?}", removed.notification_id);
                send_status(
                    &status_tx,
                    NetworkStatus::NotificationRemoved {
                        notification_id: removed.notification_id.clone(),
                    },
                );
                let _ = event_tx.send(DesktopEvent::NotificationRemoved(removed));
            }
            Payload::MessageThreadList(list) => {
                send_status(
                    &status_tx,
                    NetworkStatus::MessageThreads {
                        count: list.threads.len(),
                        state: format!("{:?}", list.status.state),
                    },
                );
                let _ = event_tx.send(DesktopEvent::MessageThreadList(list));
            }
            Payload::CallState(call) => {
                send_status(
                    &status_tx,
                    NetworkStatus::CallState {
                        state: format!("{:?}", call.state),
                        status: format!("{:?}", call.status.state),
                    },
                );
            }
            Payload::PhotoAssetList(list) => {
                send_status(
                    &status_tx,
                    NetworkStatus::PhotoAssets {
                        count: list.assets.len(),
                        state: format!("{:?}", list.status.state),
                    },
                );
            }
            Payload::RelayStatus(relay) => {
                send_status(
                    &status_tx,
                    NetworkStatus::RelayStatus {
                        enabled: relay.enabled,
                        connected: relay.connected,
                        state: format!("{:?}", relay.status.state),
                    },
                );
            }
            Payload::ClientList(list) => {
                send_status(
                    &status_tx,
                    NetworkStatus::ClientList {
                        clients: list.clients.len(),
                        input_owner: list.input_owner_desktop_id,
                    },
                );
            }
        }
    }
}

struct DesktopIncomingTransfer {
    file_name: String,
    path: PathBuf,
    bytes: u64,
    /// If set, move the file to this path once the transfer completes.
    redirect_to: Option<PathBuf>,
}

enum WriterCommand {
    SendPayload(Box<Payload>),
    SetInputAuthenticated(bool),
}

#[derive(Debug, Clone)]
struct RemoteDevice {
    device_id: String,
    device_name: String,
}

#[derive(Debug, Clone)]
struct PendingAuth {
    device: RemoteDevice,
    method: AuthMethod,
    challenge: [u8; AUTH_CHALLENGE_BYTES],
    paired_secret: [u8; PAIRED_SECRET_BYTES],
}

fn send_status(status_tx: &Sender<NetworkStatus>, status: NetworkStatus) {
    let _ = status_tx.send(status);
}

fn feature_summary(features: &[FeatureStatus]) -> String {
    let available = features
        .iter()
        .filter(|feature| {
            matches!(
                feature.state,
                androidconnect_protocol::FeatureState::Available
            )
        })
        .count();
    format!("{available}/{} available", features.len())
}

fn media_summary(status: &MediaStatus) -> String {
    if !status.active {
        return status.status.message.clone();
    }
    let title = status.title.as_deref().unwrap_or("active media");
    let artist = status.artist.as_deref().unwrap_or("");
    if artist.is_empty() {
        format!("{:?}: {title}", status.playback_state)
    } else {
        format!("{:?}: {title} - {artist}", status.playback_state)
    }
}

fn handle_incoming_file_start(
    transfers: &mut HashMap<String, DesktopIncomingTransfer>,
    status_tx: &Sender<NetworkStatus>,
    start: FileTransferStart,
    download_destinations: &Arc<Mutex<HashMap<String, PathBuf>>>,
) -> Result<()> {
    if start.direction != TransferDirection::AndroidToDesktop {
        return Ok(());
    }
    let dir = desktop_download_dir()?;
    fs::create_dir_all(&dir)?;
    let path = unique_child_path(&dir, &sanitize_file_name(&start.file_name));
    File::create(&path)?;
    // If the user requested this download via `request_download`, the Android side stamps the
    // source path into `target_path`. Look up and pop the chosen destination.
    let redirect_to = start.target_path.as_ref().and_then(|src| {
        if let Ok(mut map) = download_destinations.lock() {
            map.remove(src)
        } else {
            None
        }
    });
    transfers.insert(
        start.transfer_id.clone(),
        DesktopIncomingTransfer {
            file_name: start.file_name.clone(),
            path,
            bytes: 0,
            redirect_to,
        },
    );
    send_status(
        status_tx,
        NetworkStatus::FileTransfer {
            transfer_id: start.transfer_id,
            file_name: start.file_name,
            bytes: 0,
            status: "started".to_owned(),
        },
    );
    Ok(())
}

fn handle_incoming_file_chunk(
    transfers: &mut HashMap<String, DesktopIncomingTransfer>,
    status_tx: &Sender<NetworkStatus>,
    chunk: FileTransferChunk,
) -> Result<()> {
    let Some(transfer) = transfers.get_mut(&chunk.transfer_id) else {
        return Ok(());
    };
    if transfer.bytes != chunk.offset {
        bail!(
            "transfer {} offset mismatch: got {}, expected {}",
            chunk.transfer_id,
            chunk.offset,
            transfer.bytes
        );
    }
    let mut file = OpenOptions::new().write(true).open(&transfer.path)?;
    file.seek(SeekFrom::Start(chunk.offset))?;
    file.write_all(&chunk.data)?;
    transfer.bytes += chunk.data.len() as u64;
    send_status(
        status_tx,
        NetworkStatus::FileTransfer {
            transfer_id: chunk.transfer_id,
            file_name: transfer.file_name.clone(),
            bytes: transfer.bytes,
            status: "receiving".to_owned(),
        },
    );
    Ok(())
}

fn handle_incoming_file_complete(
    transfers: &mut HashMap<String, DesktopIncomingTransfer>,
    status_tx: &Sender<NetworkStatus>,
    complete: FileTransferComplete,
) {
    let Some(transfer) = transfers.remove(&complete.transfer_id) else {
        return;
    };
    if complete.status != TransferStatus::Completed {
        let _ = fs::remove_file(&transfer.path);
    } else if let Some(dest) = transfer.redirect_to.as_ref() {
        // Move the staged file to the user's chosen destination. Fall back to copy+remove on
        // cross-filesystem rename failures.
        if let Err(e) = fs::rename(&transfer.path, dest)
            && let Err(e2) =
                fs::copy(&transfer.path, dest).and_then(|_| fs::remove_file(&transfer.path))
        {
            warn!(
                "could not move {} to {}: rename={e:?} copy={e2:?}",
                transfer.path.display(),
                dest.display()
            );
        }
    }
    send_status(
        status_tx,
        NetworkStatus::FileTransfer {
            transfer_id: complete.transfer_id,
            file_name: transfer.file_name,
            bytes: transfer.bytes,
            status: format!("{:?}", complete.status),
        },
    );
}

fn desktop_download_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ANDROIDCONNECT_DOWNLOAD_DIR") {
        return Ok(PathBuf::from(path));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join("Downloads").join("AndroidConnect"));
    }
    Ok(std::env::current_dir()?.join("androidconnect-downloads"))
}

fn unique_child_path(parent: &std::path::Path, file_name: &str) -> PathBuf {
    let mut candidate = parent.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = std::path::Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..10_000 {
        let name = if let Some(extension) = extension {
            format!("{stem}-{index}.{extension}")
        } else {
            format!("{stem}-{index}")
        };
        candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!(
        "{}-{}",
        Instant::now().elapsed().as_nanos(),
        file_name
    ))
}

fn sanitize_file_name(value: &str) -> String {
    let clean: String = value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '\0' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect();
    let clean = clean.trim_matches('.').trim();
    if clean.is_empty() {
        "file".to_owned()
    } else {
        clean.to_owned()
    }
}

fn is_clean_disconnect(e: &anyhow::Error) -> bool {
    let kind = if let Some(WireError::Io(io)) = e.downcast_ref::<WireError>() {
        Some(io.kind())
    } else {
        e.downcast_ref::<io::Error>().map(|io| io.kind())
    };
    matches!(
        kind,
        Some(
            io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::UnexpectedEof
                | io::ErrorKind::BrokenPipe
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Shutdown;

    use androidconnect_protocol::{
        MAX_CONTROL_FRAME_BYTES, MediaControl, MediaControlAction, PointerButton, PointerEvent,
        PointerPhase,
    };

    #[test]
    fn writer_drops_commands_until_pairing_is_authenticated() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let client = TcpStream::connect(address)?;
        let (mut server, _) = listener.accept()?;
        server.set_read_timeout(Some(Duration::from_millis(100)))?;

        let (command_tx, command_rx) = mpsc::sync_channel(4);
        let (reader_done_tx, reader_done_rx) = mpsc::sync_channel(1);
        let (writer_command_tx, writer_command_rx) = mpsc::channel();

        let writer = std::thread::spawn(move || {
            write_command_loop(client, &command_rx, reader_done_rx, writer_command_rx)
        });

        command_tx.send(DesktopCommand::Input(InputEvent::Pointer(test_tap(10, 20))))?;
        let error = read_length_prefixed(&mut server, MAX_CONTROL_FRAME_BYTES)
            .expect_err("unauthenticated input must not be written");
        assert!(
            is_timeout(&error),
            "expected socket read timeout, got {error}"
        );

        writer_command_tx.send(WriterCommand::SetInputAuthenticated(true))?;
        command_tx.send(DesktopCommand::Input(InputEvent::Pointer(test_tap(30, 40))))?;
        server.set_read_timeout(Some(Duration::from_secs(1)))?;

        let envelope = read_length_prefixed(&mut server, MAX_CONTROL_FRAME_BYTES)?;
        assert!(matches!(
            envelope.payload,
            Payload::Input(InputEvent::Pointer(PointerEvent { x: 30, y: 40, .. }))
        ));

        reader_done_tx.send(Ok(()))?;
        let _ = server.shutdown(Shutdown::Both);
        writer.join().expect("writer thread join")?;
        Ok(())
    }

    #[test]
    fn writer_sends_periodic_ping() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let client = TcpStream::connect(address)?;
        let (mut server, _) = listener.accept()?;
        server.set_read_timeout(Some(Duration::from_secs(1)))?;

        let (command_tx, command_rx) = mpsc::sync_channel(4);
        let (reader_done_tx, reader_done_rx) = mpsc::sync_channel(1);
        let (_writer_command_tx, writer_command_rx) = mpsc::channel();

        let writer = std::thread::spawn(move || {
            let mut client = client;
            write_command_loop_with_heartbeat(
                &mut client,
                &command_rx,
                reader_done_rx,
                writer_command_rx,
                Duration::from_millis(25),
            )
        });

        let _keep_input_channel_open = command_tx;
        let envelope = read_length_prefixed(&mut server, MAX_CONTROL_FRAME_BYTES)?;
        assert!(matches!(envelope.payload, Payload::Ping { nonce: 1 }));

        reader_done_tx.send(Ok(()))?;
        let _ = server.shutdown(Shutdown::Both);
        writer.join().expect("writer thread join")?;
        Ok(())
    }

    #[test]
    fn writer_sends_authenticated_utility_commands() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let client = TcpStream::connect(address)?;
        let (mut server, _) = listener.accept()?;
        server.set_read_timeout(Some(Duration::from_secs(1)))?;

        let (command_tx, command_rx) = mpsc::sync_channel(4);
        let (reader_done_tx, reader_done_rx) = mpsc::sync_channel(1);
        let (writer_command_tx, writer_command_rx) = mpsc::channel();

        let writer = std::thread::spawn(move || {
            write_command_loop(client, &command_rx, reader_done_rx, writer_command_rx)
        });

        writer_command_tx.send(WriterCommand::SetInputAuthenticated(true))?;
        command_tx.send(DesktopCommand::Utility(Payload::MediaControl(
            MediaControl {
                action: MediaControlAction::PlayPause,
            },
        )))?;

        let envelope = read_length_prefixed(&mut server, MAX_CONTROL_FRAME_BYTES)?;
        assert!(matches!(
            envelope.payload,
            Payload::MediaControl(MediaControl {
                action: MediaControlAction::PlayPause
            })
        ));

        reader_done_tx.send(Ok(()))?;
        let _ = server.shutdown(Shutdown::Both);
        writer.join().expect("writer thread join")?;
        Ok(())
    }

    #[test]
    fn reader_reports_pong_status() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let mut client = TcpStream::connect(address)?;
        let (server, _) = listener.accept()?;

        let (frame_tx, _frame_rx) = mpsc::sync_channel(2);
        let (writer_command_tx, _writer_command_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();
        let (clipboard_apply_tx, _clipboard_apply_rx) = mpsc::channel();
        let (event_tx, _event_rx) = mpsc::channel();

        let downloads = Arc::new(Mutex::new(HashMap::new()));
        let reader = std::thread::spawn(move || {
            read_client_loop(
                server,
                frame_tx,
                "123456".to_owned(),
                test_trust_store("reader-reports-pong")?,
                writer_command_tx,
                status_tx,
                clipboard_apply_tx,
                event_tx,
                downloads,
            )
        });

        write_length_prefixed(&mut client, &Envelope::new(1, Payload::Pong { nonce: 42 }))?;
        let status = status_rx.recv_timeout(Duration::from_secs(1))?;
        assert_eq!(status, NetworkStatus::HeartbeatPong { nonce: 42 });

        let _ = client.shutdown(Shutdown::Both);
        let _ = reader.join().expect("reader thread join");
        Ok(())
    }

    fn test_tap(x: i32, y: i32) -> PointerEvent {
        PointerEvent {
            pointer_id: 0,
            x,
            y,
            phase: PointerPhase::Up,
            button: PointerButton::Left,
            delta_x: 0,
            delta_y: 0,
        }
    }

    fn is_timeout(error: &WireError) -> bool {
        matches!(
            error,
            WireError::Io(io)
                if matches!(io.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
        )
    }

    fn test_trust_store(name: &str) -> Result<TrustStore> {
        let path = std::env::temp_dir().join(format!(
            "androidconnect-desktop-network-{name}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        TrustStore::load_or_create_at(path)
    }
}
