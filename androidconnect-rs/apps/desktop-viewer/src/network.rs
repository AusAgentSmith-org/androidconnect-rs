use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Sender;

use androidconnect_protocol::{
    MAX_VIDEO_FRAME_BYTES, PROTOCOL_VERSION, Payload, WireError, read_length_prefixed,
};
use anyhow::{Result, bail};
use log::{error, info, warn};
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;

use crate::RgbaFrame;

pub fn run(bind: &str, sender: Sender<RgbaFrame>) -> Result<()> {
    let listener = TcpListener::bind(bind)?;
    info!("listening on {bind}");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let peer = stream.peer_addr().ok();
                info!("client connected: {peer:?}");
                let tx = sender.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle_client(stream, tx) {
                        if is_clean_disconnect(&e) {
                            info!("client disconnected");
                        } else {
                            error!("client error: {e:#}");
                        }
                    }
                });
            }
            Err(e) => error!("accept: {e}"),
        }
    }

    Ok(())
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

fn handle_client(mut stream: TcpStream, sender: Sender<RgbaFrame>) -> Result<()> {
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
            }
            Payload::VideoFormat(fmt) => {
                info!(
                    "video: {}x{} {:?} {}fps rotation={}°",
                    fmt.width, fmt.height, fmt.codec, fmt.frame_rate, fmt.rotation_degrees
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
            Payload::Input(_) => {}
        }
    }
}
