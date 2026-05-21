use std::io::{self, Read};
use std::net::{Shutdown, TcpStream};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use androidconnect_protocol::{
    AUTH_CHALLENGE_BYTES, AuthChallenge, AuthResult, DeviceHello, Envelope, InputEvent,
    MAX_CONTROL_FRAME_BYTES, PROTOCOL_VERSION, Payload, PointerButton, PointerEvent, PointerPhase,
    SystemAction, VideoCodec, VideoFormat, VideoFrame, auth_responses_equal,
    normalize_pairing_code, pairing_auth_response, read_length_prefixed,
    video_flags_from_android_media_codec, write_length_prefixed,
};
use jni::objects::{GlobalRef, JByteArray, JClass, JString, JValue};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};
use jni::{JNIEnv, JavaVM};

static STATE: OnceLock<Mutex<AppState>> = OnceLock::new();

#[derive(Debug)]
struct AppState {
    sequence: u64,
    capture: Option<CaptureState>,
    stream: Option<TcpStream>,
    connected_to: Option<String>,
    connection_generation: u64,
    last_error: Option<String>,
    last_video_format: Option<VideoFormat>,
    pairing_code_set: bool,
    input_authenticated: bool,
    sent_envelopes: u64,
    sent_bytes: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            sequence: 1,
            capture: None,
            stream: None,
            connected_to: None,
            connection_generation: 0,
            last_error: None,
            last_video_format: None,
            pairing_code_set: false,
            input_authenticated: false,
            sent_envelopes: 0,
            sent_bytes: 0,
        }
    }
}

impl AppState {
    fn next_sequence(&mut self) -> u64 {
        let sequence = self.sequence;
        self.sequence += 1;
        sequence
    }

    fn send_payload(&mut self, payload: Payload) -> Result<(), String> {
        let envelope = Envelope::new(self.next_sequence(), payload);
        let video_bytes = match &envelope.payload {
            Payload::VideoFrame(frame) => frame.data.len() as u64,
            _ => 0,
        };
        let stream = self.stream.as_mut().ok_or("not connected")?;
        write_length_prefixed(stream, &envelope).map_err(|error| error.to_string())?;
        self.sent_envelopes += 1;
        self.sent_bytes += video_bytes;
        Ok(())
    }

    fn clear_connection(&mut self, error: impl Into<String>) {
        self.connection_generation += 1;
        close_stream(self.stream.take());
        self.connected_to = None;
        self.input_authenticated = false;
        self.last_error = Some(error.into());
    }

    fn is_connected(&self) -> bool {
        self.stream.is_some()
    }
}

#[derive(Debug)]
struct CaptureState {
    started_at: Instant,
    encoded_frames: u64,
    encoded_bytes: u64,
}

fn state() -> &'static Mutex<AppState> {
    STATE.get_or_init(|| Mutex::new(AppState::default()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeStartSession(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    config_json: JString<'_>,
) -> jlong {
    let _config = env
        .get_string(&config_json)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return 0,
    };

    guard.capture = Some(CaptureState {
        started_at: Instant::now(),
        encoded_frames: 0,
        encoded_bytes: 0,
    });
    guard.last_error = None;

    1
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeStopSession(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    if let Ok(mut guard) = state().lock() {
        guard.capture = None;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeConnect(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    host: JString<'_>,
    port: jint,
    device_name: JString<'_>,
    pairing_code: JString<'_>,
) -> jboolean {
    let host = match env.get_string(&host) {
        Ok(value) => value.to_string_lossy().into_owned(),
        Err(error) => {
            set_last_error(format!("invalid host string: {error}"));
            return JNI_FALSE;
        }
    };
    let device_name = env
        .get_string(&device_name)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "Android device".to_owned());
    let pairing_code = match env.get_string(&pairing_code) {
        Ok(value) => normalize_pairing_code(&value.to_string_lossy()),
        Err(error) => {
            set_last_error(format!("invalid pairing code string: {error}"));
            return JNI_FALSE;
        }
    };

    if host.trim().is_empty() || port <= 0 || port > u16::MAX as jint {
        set_last_error("invalid host or port");
        return JNI_FALSE;
    }
    if pairing_code.is_empty() {
        set_last_error("pairing code required");
        return JNI_FALSE;
    }

    let address = format!("{}:{}", host.trim(), port);
    let stream = match TcpStream::connect(&address) {
        Ok(stream) => stream,
        Err(error) => {
            set_last_error(format!("connect to {address} failed: {error}"));
            return JNI_FALSE;
        }
    };
    if let Err(error) = configure_stream(&stream) {
        set_last_error(format!("configure {address} failed: {error}"));
        return JNI_FALSE;
    }
    let input_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(error) => {
            set_last_error(format!("clone {address} stream failed: {error}"));
            return JNI_FALSE;
        }
    };
    let java_vm = match env.get_java_vm() {
        Ok(vm) => vm,
        Err(error) => {
            set_last_error(format!("JavaVM unavailable: {error}"));
            return JNI_FALSE;
        }
    };
    let input_class = match remote_control_class(&mut env) {
        Ok(class) => class,
        Err(error) => {
            set_last_error(error);
            return JNI_FALSE;
        }
    };

    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return JNI_FALSE,
    };
    guard.connection_generation += 1;
    guard.stream = Some(stream);
    guard.connected_to = Some(address.clone());
    guard.pairing_code_set = true;
    guard.input_authenticated = false;
    guard.last_error = None;
    let generation = guard.connection_generation;
    let auth_challenge = make_auth_challenge();
    let expected_auth_response = pairing_auth_response(&pairing_code, &auth_challenge.challenge);

    let hello = Payload::Hello(DeviceHello::android("android-local", device_name));
    let send_result = guard
        .send_payload(hello)
        .and_then(|()| guard.send_payload(Payload::AuthChallenge(auth_challenge)));
    match send_result {
        Ok(()) => {
            if let Some(format) = guard.last_video_format.clone()
                && let Err(error) = guard.send_payload(Payload::VideoFormat(format))
            {
                guard.clear_connection(format!("format send to {address} failed: {error}"));
                return JNI_FALSE;
            }
            drop(guard);
            spawn_input_reader(
                java_vm,
                input_class,
                input_stream,
                generation,
                address,
                expected_auth_response,
            );
            JNI_TRUE
        }
        Err(error) => {
            guard.clear_connection(format!("hello send to {address} failed: {error}"));
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeDisconnect(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    if let Ok(mut guard) = state().lock() {
        guard.connection_generation += 1;
        close_stream(guard.stream.take());
        guard.connected_to = None;
        guard.pairing_code_set = false;
        guard.input_authenticated = false;
        guard.last_error = None;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushVideoFormat(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    width: jint,
    height: jint,
    dpi: jint,
    frame_rate: jint,
    rotation_degrees: jint,
) -> jboolean {
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return JNI_FALSE,
    };
    if guard.capture.is_none() {
        return JNI_FALSE;
    }

    let format = VideoFormat {
        stream_id: 0,
        codec: VideoCodec::H264,
        width: width.max(0) as u32,
        height: height.max(0) as u32,
        rotation_degrees: rotation_degrees.max(0) as u16,
        dpi: dpi.max(0) as u32,
        frame_rate: frame_rate.max(0) as u32,
    };
    guard.last_video_format = Some(format.clone());

    if !guard.is_connected() {
        return JNI_FALSE;
    }

    match guard.send_payload(Payload::VideoFormat(format)) {
        Ok(()) => JNI_TRUE,
        Err(error) => {
            guard.clear_connection(error);
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushVideoFrame(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    frame: JByteArray<'_>,
    presentation_time_us: jlong,
    media_codec_flags: jint,
) -> jboolean {
    let frame = match env.convert_byte_array(frame) {
        Ok(frame) => frame,
        Err(_) => return JNI_FALSE,
    };

    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return JNI_FALSE,
    };
    let Some(capture) = guard.capture.as_mut() else {
        return JNI_FALSE;
    };

    capture.encoded_frames += 1;
    capture.encoded_bytes += frame.len() as u64;

    if !guard.is_connected() {
        return JNI_FALSE;
    }

    let payload = Payload::VideoFrame(VideoFrame {
        stream_id: 0,
        presentation_time_us,
        flags: video_flags_from_android_media_codec(media_codec_flags),
        data: frame,
    });

    match guard.send_payload(payload) {
        Ok(()) => JNI_TRUE,
        Err(error) => {
            guard.clear_connection(error);
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeStatsJson(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let stats = current_stats_json();
    match env.new_string(stats) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

pub fn current_stats_json() -> String {
    let Ok(guard) = state().lock() else {
        return r#"{"running":false,"error":"lock_poisoned"}"#.to_owned();
    };
    let capture = guard.capture.as_ref();

    serde_json::json!({
        "running": capture.is_some(),
        "connected": guard.stream.is_some(),
        "connected_to": guard.connected_to,
        "encoded_frames": capture.map(|capture| capture.encoded_frames).unwrap_or_default(),
        "encoded_bytes": capture.map(|capture| capture.encoded_bytes).unwrap_or_default(),
        "sent_envelopes": guard.sent_envelopes,
        "sent_bytes": guard.sent_bytes,
        "pairing_code_set": guard.pairing_code_set,
        "input_authenticated": guard.input_authenticated,
        "uptime_ms": capture.map(|capture| capture.started_at.elapsed().as_millis()).unwrap_or_default(),
        "last_error": guard.last_error,
    })
    .to_string()
}

fn spawn_input_reader(
    java_vm: JavaVM,
    input_class: GlobalRef,
    stream: TcpStream,
    generation: u64,
    address: String,
    expected_auth_response: [u8; androidconnect_protocol::AUTH_RESPONSE_BYTES],
) {
    let thread_address = address.clone();
    let spawn_result = thread::Builder::new()
        .name("AndroidConnectInputReader".to_owned())
        .spawn(move || {
            if let Err(error) = input_reader_loop(
                java_vm,
                input_class,
                stream,
                generation,
                expected_auth_response,
            ) {
                clear_connection_if_generation(
                    generation,
                    format!("input reader from {thread_address} stopped: {error}"),
                );
            }
        });

    if let Err(error) = spawn_result {
        clear_connection_if_generation(
            generation,
            format!("input reader for {address} failed to start: {error}"),
        );
    }
}

fn input_reader_loop(
    java_vm: JavaVM,
    input_class: GlobalRef,
    mut stream: TcpStream,
    generation: u64,
    expected_auth_response: [u8; androidconnect_protocol::AUTH_RESPONSE_BYTES],
) -> Result<(), String> {
    let mut env = java_vm
        .attach_current_thread()
        .map_err(|error| format!("attach JNI input thread failed: {error}"))?;
    let mut dispatcher = InputDispatcher::new(input_class);
    let mut desktop_authenticated = false;

    loop {
        let envelope = read_length_prefixed(&mut stream, MAX_CONTROL_FRAME_BYTES)
            .map_err(|error| error.to_string())?;

        if envelope.version != PROTOCOL_VERSION {
            return Err(format!(
                "unsupported protocol version {}, expected {PROTOCOL_VERSION}",
                envelope.version
            ));
        }

        match envelope.payload {
            Payload::Input(event) => {
                if desktop_authenticated {
                    dispatcher.dispatch(&mut env, event);
                } else {
                    set_last_error("input ignored before pairing authentication");
                }
            }
            Payload::AuthChallenge(_) => {}
            Payload::AuthResponse(response) => {
                let accepted = auth_responses_equal(&expected_auth_response, &response.response);
                let message = if accepted {
                    "paired".to_owned()
                } else {
                    "pairing authentication failed".to_owned()
                };
                set_input_authenticated_if_generation(generation, accepted);
                send_payload_if_generation(
                    generation,
                    Payload::AuthResult(AuthResult {
                        accepted,
                        message: message.clone(),
                    }),
                )?;
                if !accepted {
                    return Err(message);
                }
                desktop_authenticated = true;
            }
            Payload::AuthResult(result) => {
                if !result.accepted {
                    return Err(format!("desktop rejected pairing: {}", result.message));
                }
            }
            Payload::Ping { .. }
            | Payload::Pong { .. }
            | Payload::Error { .. }
            | Payload::Hello(_)
            | Payload::VideoFormat(_)
            | Payload::VideoFrame(_) => {}
        }
    }
}

struct InputDispatcher {
    input_class: GlobalRef,
    active_pointer: Option<ActivePointer>,
}

impl InputDispatcher {
    fn new(input_class: GlobalRef) -> Self {
        Self {
            input_class,
            active_pointer: None,
        }
    }

    fn dispatch(&mut self, env: &mut JNIEnv<'_>, event: InputEvent) -> bool {
        match event {
            InputEvent::Pointer(pointer) => self.dispatch_pointer(env, pointer),
            InputEvent::Text(text) => self.dispatch_text(env, text.text),
            InputEvent::System(action) => self.dispatch_system_action(env, action),
            InputEvent::Key(key) => self.dispatch_key(env, key),
        }
    }

    fn dispatch_pointer(&mut self, env: &mut JNIEnv<'_>, event: PointerEvent) -> bool {
        match event.phase {
            PointerPhase::Down => {
                if event.button != PointerButton::Left {
                    return false;
                }
                self.active_pointer = Some(ActivePointer {
                    pointer_id: event.pointer_id,
                    start_x: event.x,
                    start_y: event.y,
                    last_x: event.x,
                    last_y: event.y,
                    started_at: Instant::now(),
                });
                true
            }
            PointerPhase::Move => {
                if event.button != PointerButton::Left {
                    return false;
                }
                if let Some(pointer) = self.active_pointer.as_mut()
                    && pointer.pointer_id == event.pointer_id
                {
                    pointer.last_x = event.x;
                    pointer.last_y = event.y;
                }
                true
            }
            PointerPhase::Up => {
                if event.button != PointerButton::Left {
                    return false;
                }
                let Some(pointer) = self.active_pointer.take() else {
                    return self.tap(env, event.x, event.y);
                };
                if pointer.pointer_id != event.pointer_id {
                    return false;
                }

                let distance_x = event.x - pointer.start_x;
                let distance_y = event.y - pointer.start_y;
                let duration_ms = pointer.started_at.elapsed().as_millis().max(1) as jlong;
                if distance_x.abs() <= 8 && distance_y.abs() <= 8 {
                    self.tap(env, event.x, event.y)
                } else {
                    self.drag(
                        env,
                        pointer.start_x,
                        pointer.start_y,
                        event.x,
                        event.y,
                        duration_ms,
                    )
                }
            }
            PointerPhase::Wheel => {
                let delta = if event.delta_y != 0 {
                    event.delta_y
                } else {
                    event.delta_x
                };
                if delta == 0 {
                    true
                } else {
                    self.scroll(env, delta)
                }
            }
        }
    }

    fn dispatch_text(&self, env: &mut JNIEnv<'_>, text: String) -> bool {
        if text.is_empty() {
            return true;
        }

        let jtext = match env.new_string(text) {
            Ok(value) => value,
            Err(error) => {
                set_last_error(format!("input text JNI string failed: {error}"));
                clear_pending_exception(env);
                return false;
            }
        };
        call_static_bool(
            env,
            &self.input_class,
            "inputText",
            "(Ljava/lang/String;)Z",
            &[JValue::from(&jtext)],
        )
    }

    fn dispatch_system_action(&self, env: &mut JNIEnv<'_>, action: SystemAction) -> bool {
        let method = match action {
            SystemAction::Back => "systemBack",
            SystemAction::Home => "systemHome",
            SystemAction::Recents => "systemRecents",
            SystemAction::LockScreen => "lockScreen",
        };
        call_static_bool(env, &self.input_class, method, "()Z", &[])
    }

    fn dispatch_key(&self, env: &mut JNIEnv<'_>, key: androidconnect_protocol::KeyEvent) -> bool {
        if !key.pressed {
            return true;
        }

        match key.key_code {
            3 => self.dispatch_system_action(env, SystemAction::Home),
            4 => self.dispatch_system_action(env, SystemAction::Back),
            26 => self.dispatch_system_action(env, SystemAction::LockScreen),
            187 => self.dispatch_system_action(env, SystemAction::Recents),
            _ => false,
        }
    }

    fn tap(&self, env: &mut JNIEnv<'_>, x: i32, y: i32) -> bool {
        call_static_bool(
            env,
            &self.input_class,
            "tap",
            "(II)Z",
            &[JValue::Int(x), JValue::Int(y)],
        )
    }

    fn drag(
        &self,
        env: &mut JNIEnv<'_>,
        start_x: i32,
        start_y: i32,
        end_x: i32,
        end_y: i32,
        duration_ms: jlong,
    ) -> bool {
        call_static_bool(
            env,
            &self.input_class,
            "drag",
            "(IIIIJ)Z",
            &[
                JValue::Int(start_x),
                JValue::Int(start_y),
                JValue::Int(end_x),
                JValue::Int(end_y),
                JValue::Long(duration_ms),
            ],
        )
    }

    fn scroll(&self, env: &mut JNIEnv<'_>, delta_y: i32) -> bool {
        call_static_bool(
            env,
            &self.input_class,
            "scroll",
            "(I)Z",
            &[JValue::Int(delta_y)],
        )
    }
}

#[derive(Debug)]
struct ActivePointer {
    pointer_id: u32,
    start_x: i32,
    start_y: i32,
    last_x: i32,
    last_y: i32,
    started_at: Instant,
}

fn call_static_bool(
    env: &mut JNIEnv<'_>,
    class: &GlobalRef,
    name: &str,
    sig: &str,
    args: &[JValue<'_, '_>],
) -> bool {
    match env
        .call_static_method(class, name, sig, args)
        .and_then(|value| value.z())
    {
        Ok(result) => result,
        Err(error) => {
            set_last_error(format!("input dispatch {name} failed: {error}"));
            clear_pending_exception(env);
            false
        }
    }
}

fn clear_pending_exception(env: &mut JNIEnv<'_>) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

fn remote_control_class(env: &mut JNIEnv<'_>) -> Result<GlobalRef, String> {
    let class = env
        .find_class("dev/androidconnect/RemoteControlAccessibilityService")
        .map_err(|error| format!("RemoteControlAccessibilityService class unavailable: {error}"))?;
    env.new_global_ref(class)
        .map_err(|error| format!("RemoteControlAccessibilityService global ref failed: {error}"))
}

fn clear_connection_if_generation(generation: u64, error: impl Into<String>) {
    if let Ok(mut guard) = state().lock()
        && guard.connection_generation == generation
    {
        guard.clear_connection(error);
    }
}

fn set_input_authenticated_if_generation(generation: u64, authenticated: bool) {
    if let Ok(mut guard) = state().lock()
        && guard.connection_generation == generation
    {
        guard.input_authenticated = authenticated;
    }
}

fn send_payload_if_generation(generation: u64, payload: Payload) -> Result<(), String> {
    let mut guard = state()
        .lock()
        .map_err(|_| "state lock poisoned while sending auth payload".to_owned())?;
    if guard.connection_generation != generation {
        return Err("stale connection generation".to_owned());
    }
    guard.send_payload(payload)
}

fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)
}

fn close_stream(stream: Option<TcpStream>) {
    if let Some(stream) = stream {
        let _ = stream.shutdown(Shutdown::Both);
    }
}

fn set_last_error(error: impl Into<String>) {
    if let Ok(mut guard) = state().lock() {
        guard.last_error = Some(error.into());
    }
}

fn make_auth_challenge() -> AuthChallenge {
    let mut challenge = [0_u8; AUTH_CHALLENGE_BYTES];
    if fill_random(&mut challenge).is_err() {
        fill_fallback_challenge(&mut challenge);
    }
    AuthChallenge { challenge }
}

fn fill_random(output: &mut [u8]) -> io::Result<()> {
    let mut file = std::fs::File::open("/dev/urandom")?;
    file.read_exact(output)
}

fn fill_fallback_challenge(output: &mut [u8]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut seed = now ^ ((std::process::id() as u128) << 64);
    for chunk in output.chunks_mut(8) {
        seed ^= seed << 7;
        seed ^= seed >> 9;
        seed = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let bytes = seed.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}
