use std::io;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use androidconnect_protocol::{
    AUTH_CHALLENGE_BYTES, AuthMethod, AuthResponse, Envelope, InputEvent, MAX_VIDEO_FRAME_BYTES,
    PAIRED_SECRET_BYTES, PROTOCOL_VERSION, Payload, WireError, bytes_to_hex, derive_session_key,
    paired_secret_from_pairing_code, pairing_auth_response, read_length_prefixed,
    session_key_fingerprint, trusted_session_auth_response, write_length_prefixed,
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
}

pub fn run(
    bind: &str,
    frame_sender: Sender<RgbaFrame>,
    input_rx: Receiver<InputEvent>,
    pairing_code: String,
    trust_store_path: PathBuf,
    status_tx: Sender<NetworkStatus>,
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
                drain_stale_input(&input_rx);
                if let Err(e) = handle_client(
                    stream,
                    frame_sender.clone(),
                    &input_rx,
                    &pairing_code,
                    trust_store,
                    status_tx.clone(),
                ) {
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
                    drain_stale_input(&input_rx);
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

fn handle_client(
    stream: TcpStream,
    frame_sender: Sender<RgbaFrame>,
    input_rx: &Receiver<InputEvent>,
    pairing_code: &str,
    trust_store: TrustStore,
    status_tx: Sender<NetworkStatus>,
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
        );
        let _ = reader_done_tx.send(result);
    });

    write_input_loop(writer, input_rx, reader_done_rx, writer_command_rx)
}

fn write_input_loop(
    mut stream: TcpStream,
    input_rx: &Receiver<InputEvent>,
    reader_done_rx: Receiver<Result<()>>,
    writer_command_rx: Receiver<WriterCommand>,
) -> Result<()> {
    write_input_loop_with_heartbeat(
        &mut stream,
        input_rx,
        reader_done_rx,
        writer_command_rx,
        HEARTBEAT_INTERVAL,
    )
}

fn write_input_loop_with_heartbeat(
    stream: &mut TcpStream,
    input_rx: &Receiver<InputEvent>,
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

        match input_rx.recv_timeout(INPUT_POLL_INTERVAL) {
            Ok(event) => {
                drain_writer_commands(
                    stream,
                    &mut sequence,
                    &writer_command_rx,
                    &mut input_authenticated,
                )?;
                if input_authenticated {
                    write_input(stream, &mut sequence, event)?;
                } else if !logged_unauthenticated_input {
                    warn!("dropping desktop input until pairing succeeds");
                    logged_unauthenticated_input = true;
                }
                while let Ok(event) = input_rx.try_recv() {
                    if input_authenticated {
                        write_input(stream, &mut sequence, event)?;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => bail!("desktop input channel closed"),
        }
    }
}

fn write_input(stream: &mut TcpStream, sequence: &mut u64, event: InputEvent) -> Result<()> {
    write_payload(stream, sequence, Payload::Input(event))
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
                write_payload(stream, sequence, payload)?;
            }
            Ok(WriterCommand::SetInputAuthenticated(accepted)) => {
                *input_authenticated = accepted;
            }
            Err(TryRecvError::Empty) => return Ok(()),
            Err(TryRecvError::Disconnected) => return Ok(()),
        }
    }
}

fn drain_stale_input(input_rx: &Receiver<InputEvent>) {
    while input_rx.try_recv().is_ok() {}
}

fn read_client_loop(
    mut stream: TcpStream,
    sender: Sender<RgbaFrame>,
    pairing_code: String,
    mut trust_store: TrustStore,
    writer_command_tx: Sender<WriterCommand>,
    status_tx: Sender<NetworkStatus>,
) -> Result<()> {
    let mut decoder = Decoder::new().map_err(|e| anyhow::anyhow!("decoder init failed: {e:?}"))?;
    let desktop_identity = trust_store.identity();
    let mut current_device: Option<RemoteDevice> = None;
    let mut pending_auth: Option<PendingAuth> = None;

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
                        let _ = sender.send(RgbaFrame {
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
                let _ = writer_command_tx
                    .send(WriterCommand::SendPayload(Payload::AuthResponse(response)));
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
        }
    }
}

enum WriterCommand {
    SendPayload(Payload),
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
        MAX_CONTROL_FRAME_BYTES, PointerButton, PointerEvent, PointerPhase,
    };

    #[test]
    fn writer_drops_input_until_pairing_is_authenticated() -> Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let client = TcpStream::connect(address)?;
        let (mut server, _) = listener.accept()?;
        server.set_read_timeout(Some(Duration::from_millis(100)))?;

        let (input_tx, input_rx) = mpsc::sync_channel(4);
        let (reader_done_tx, reader_done_rx) = mpsc::sync_channel(1);
        let (writer_command_tx, writer_command_rx) = mpsc::channel();

        let writer = std::thread::spawn(move || {
            write_input_loop(client, &input_rx, reader_done_rx, writer_command_rx)
        });

        input_tx.send(InputEvent::Pointer(test_tap(10, 20)))?;
        let error = read_length_prefixed(&mut server, MAX_CONTROL_FRAME_BYTES)
            .expect_err("unauthenticated input must not be written");
        assert!(
            is_timeout(&error),
            "expected socket read timeout, got {error}"
        );

        writer_command_tx.send(WriterCommand::SetInputAuthenticated(true))?;
        input_tx.send(InputEvent::Pointer(test_tap(30, 40)))?;
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

        let (input_tx, input_rx) = mpsc::sync_channel(4);
        let (reader_done_tx, reader_done_rx) = mpsc::sync_channel(1);
        let (_writer_command_tx, writer_command_rx) = mpsc::channel();

        let writer = std::thread::spawn(move || {
            let mut client = client;
            write_input_loop_with_heartbeat(
                &mut client,
                &input_rx,
                reader_done_rx,
                writer_command_rx,
                Duration::from_millis(25),
            )
        });

        let _keep_input_channel_open = input_tx;
        let envelope = read_length_prefixed(&mut server, MAX_CONTROL_FRAME_BYTES)?;
        assert!(matches!(envelope.payload, Payload::Ping { nonce: 1 }));

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

        let (frame_tx, _frame_rx) = mpsc::channel();
        let (writer_command_tx, _writer_command_rx) = mpsc::channel();
        let (status_tx, status_rx) = mpsc::channel();

        let reader = std::thread::spawn(move || {
            read_client_loop(
                server,
                frame_tx,
                "123456".to_owned(),
                test_trust_store("reader-reports-pong")?,
                writer_command_tx,
                status_tx,
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
