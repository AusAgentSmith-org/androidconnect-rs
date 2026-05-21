use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use androidconnect_protocol::{
    AuthResponse, Envelope, InputEvent, MAX_VIDEO_FRAME_BYTES, PROTOCOL_VERSION, Payload,
    WireError, pairing_auth_response, read_length_prefixed, write_length_prefixed,
};
use anyhow::{Result, bail};
use log::{error, info, warn};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;

use crate::RgbaFrame;

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
    PairingAuthenticated,
    PairingRejected {
        message: String,
    },
    VideoFormat {
        width: u32,
        height: u32,
        frame_rate: u32,
        rotation_degrees: u16,
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
                drain_stale_input(&input_rx);
                if let Err(e) = handle_client(
                    stream,
                    frame_sender.clone(),
                    &input_rx,
                    &pairing_code,
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
    mut stream: &mut TcpStream,
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
            &mut stream,
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
                    &mut stream,
                    &mut sequence,
                    &writer_command_rx,
                    &mut input_authenticated,
                )?;
                if input_authenticated {
                    write_input(&mut stream, &mut sequence, event)?;
                } else if !logged_unauthenticated_input {
                    warn!("dropping desktop input until pairing succeeds");
                    logged_unauthenticated_input = true;
                }
                while let Ok(event) = input_rx.try_recv() {
                    if input_authenticated {
                        write_input(&mut stream, &mut sequence, event)?;
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
    writer_command_tx: Sender<WriterCommand>,
    status_tx: Sender<NetworkStatus>,
) -> Result<()> {
    let mut decoder = Decoder::new().map_err(|e| anyhow::anyhow!("decoder init failed: {e:?}"))?;

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
            Payload::Pong { nonce } => info!("pong {nonce}"),
            Payload::Error { message } => error!("peer error: {message}"),
            Payload::AuthChallenge(challenge) => {
                let response = AuthResponse {
                    response: pairing_auth_response(&pairing_code, &challenge.challenge),
                };
                let _ = writer_command_tx
                    .send(WriterCommand::SendPayload(Payload::AuthResponse(response)));
            }
            Payload::AuthResult(result) => {
                if result.accepted {
                    info!("pairing authenticated; desktop input enabled");
                    send_status(&status_tx, NetworkStatus::PairingAuthenticated);
                } else {
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
}
