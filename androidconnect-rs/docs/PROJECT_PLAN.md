# AndroidConnect RS Project Plan

## Product Goal

Build an open, local-first, Rust-based Android screen mirroring and desktop control stack for non-rooted Android devices.

The product should let a user launch the Android app, approve screen capture and input permissions, connect from a desktop app on the same LAN, view the Android display in real time, and control the Android device with desktop mouse and keyboard input.

This is not a Phone Link clone and not a KDE Connect port. The target is broad Android compatibility, a permissive MIT/Apache-2.0 licence, and no dependency on Windows, Microsoft accounts, or GPL boundaries. See `docs/FEATURE_POSITIONING.md` for the full product thesis and feature-slice roadmap.

## MVP 1 Scope

MVP 1 is LAN-only Android mirroring with desktop input control.

Included:

- Android app starts a screen capture session through `MediaProjection`.
- Android app encodes full-device video using `MediaCodec` H.264.
- Android app sends video stream metadata and encoded frames to the desktop.
- Desktop app discovers or manually connects to the Android device on the same network.
- Desktop app decodes and renders the live Android screen.
- Desktop mouse input maps to Android tap, drag, and scroll actions.
- Desktop keyboard input can send text and common navigation commands.
- Back, home, recents, and lock actions work through Android accessibility globals.
- Session stops cleanly when the user revokes screen capture, disables accessibility, closes the desktop app, or disconnects the network.

Excluded from MVP 1:

- Internet relay or NAT traversal.
- Audio forwarding.
- File transfer, notifications, SMS, clipboard, media controls, or KDE Connect feature parity.
- Multiple simultaneous desktop clients.
- Root-only input injection or privileged Android APIs.
- Production-grade device account system.
- App store release readiness.

## Current Repository State

Implemented:

- Rust workspace at `androidconnect-rs`.
- Shared protocol crate at `crates/protocol`.
- Android JNI bridge crate at `crates/android-native`.
- Desktop receiver at `apps/desktop-viewer` — decodes H.264 with OpenH264, renders live in a native window (`winit` + `pixels`), letterbox-scales to window size.
- Android shell at `apps/android`.
- Android `MediaProjection` foreground service and H.264 encoder (working, validated).
- Android to desktop TCP transport — `DeviceHello`, `VideoFormat`, `VideoFrame` streaming end-to-end.
- Desktop to Android input transport over the existing TCP connection for MVP validation.
- Ephemeral pairing-code challenge/response using HMAC-SHA256. Android ignores desktop input until
  pairing succeeds.
- Android accessibility service with tap, drag, scroll, basic text edit, and global action helpers.
- Desktop coordinate mapping from letterboxed window pixels to Android frame pixels.
- Desktop window-title status for listening, connected, paired, video format, heartbeat,
  disconnected, and error states.
- Android status text for capture, desktop connection, pairing, input service, counters, and last
  error. It refreshes while the activity is visible and includes heartbeat counters.
- Desktop heartbeat pings and Android pong replies over the existing TCP connection, with visible
  heartbeat health on both sides.
- Gradle wrapper added to `apps/android` (copied from kdeconnect-android reference tree).

Not implemented yet:

- End-to-end input validation on a physical Android device (emulator pass does not substitute).
- Persistent paired desktop identity, durable trust state, and encrypted/authenticated session
  transport. The current pairing-code gate is not the final security model.
- Reconnect/session recovery and native connection status callbacks beyond the current polling UI.
- Rotation/resolution renegotiation beyond initial format metadata.

## Architecture

```text
Android UI
  MainActivity
    requests MediaProjection consent
    opens accessibility settings

Android foreground service
  MirrorService
    owns MediaProjection lifetime
    starts ScreenEncoder

Android video pipeline
  VirtualDisplay -> MediaCodec H.264 encoder -> JNI -> Rust protocol sender

Android input pipeline
  Rust inbound input receiver -> Java dispatch bridge -> AccessibilityService

Shared Rust protocol
  DeviceHello
  AuthChallenge / AuthResponse / AuthResult
  VideoFormat
  VideoFrame
  InputEvent
  Ping/Pong/Error

Desktop app
  connection manager
  H.264 decoder
  render window
  mouse/keyboard capture
  input event sender
```

Ownership boundaries:

- Kotlin/Java owns Android system APIs that have no stable Rust-native equivalent.
- Rust owns protocol encoding, session state, transport, pairing, and desktop logic.
- The JNI boundary should stay thin. It should pass buffers and events, not hide business logic in Java.

## Technical Direction

Video:

- Use Android `MediaCodec` with `COLOR_FormatSurface` for efficient capture.
- Start with H.264 because hardware encode/decode support is broad.
- Send codec config frames and key frames explicitly through protocol flags.
- Desktop renderer should tolerate format changes by recreating the decoder.

Transport:

- First implementation can use TCP for fast local validation.
- QUIC should replace TCP before broader testing because separate streams are cleaner for video, control, input, and later audio.
- Control messages must be reliable.
- Video should prefer low latency over perfect delivery once QUIC datagrams are introduced.

Input:

- Accessibility is the MVP input mechanism because it works without root.
- Pointer down/up/move is accumulated into tap or drag gestures on Android.
- Scroll maps to accessibility scroll actions against the active window.
- Text input currently uses accessibility `ACTION_SET_TEXT` and should move to an Android IME service
  before MVP polish for reliable cursor, selection, and composition behavior.
- The detailed input reference and QA checklist live in `docs/INPUT_CONTROL.md`.

Pairing and security:

- MVP starts with a manual pairing code printed by the desktop and entered on Android.
- The current implementation uses an HMAC-SHA256 challenge/response and ignores input until Android
  verifies that proof.
- A persistent paired session should still store trusted desktop identity and derive per-session keys.
- Unauthenticated input packets must continue to be ignored.
- LAN discovery must not imply trust.

## Milestones

### M0: Scaffold

Status: mostly complete.

Deliverables:

- Workspace layout.
- Shared protocol types.
- Android capture shell.
- Native bridge shell.
- Desktop receiver shell.
- Build notes and project plan.

Exit criteria:

- `cargo test --workspace --offline` passes.
- Android source tree exists and describes required permissions.
- Desktop receiver can compile and wait for protocol envelopes.

### M1: Android To Desktop Transport

Goal: get encoded Android frames leaving the device and reaching the desktop process.

Status: **complete**. Validated on emulator (Pixel 7 Pro AVD, API 36). Live H.264 frames streaming end-to-end at 1080p/30fps.

Tasks:

- Add a Rust transport module to `crates/android-native`. Done.
- Expose `nativeConnect(host, port, pairingCode)` and `nativeDisconnect()` through JNI. Done.
- Send `DeviceHello`, `VideoFormat`, and `VideoFrame` envelopes over TCP. Done.
- Add Android UI fields for desktop host and port. Done.
- Add connection status and error reporting to Android UI. Partial.
- Add desktop receiver validation for protocol version and capabilities. Partial.
- Validate the path on a physical Android device. Emulator only — physical device validation pending.

Known fixes applied during validation:

- `ScreenEncoder.displayMetrics()` must use `DisplayManager` (not `WindowManager.getDefaultDisplay()`) when called from a Service context on API 30+.
- Resolution must be scaled down before passing to `MediaCodec.configure()` — the emulator H.264 encoder rejects resolutions above ~1080p short edge. `ScreenEncoder` now caps to 1080px on the short edge.
- Emulator performance: use a standard `google_apis_playstore` system image, not `ps16k`. The 16KB page-size image causes periodic stalls in the emulator.

Exit criteria:

- Starting capture on Android produces live frame logs on desktop. ✓
- Disconnecting either side releases the capture session without process crashes. ✓
- Restarting capture creates a fresh `MediaProjection` session. ✓

### M2: Desktop Video Decode And Render

Goal: display the Android screen in a native desktop window.

Status: **complete**. OpenH264 decoder, `winit` + `pixels` render window, CPU-side letterbox scaling. Validated on emulator at 1080×2340/30fps.

Tasks:

- Choose the first decoder path: FFmpeg, GStreamer, or platform decoder. Done — OpenH264 (`openh264` crate, compiles from source, no system deps).
- Add a render window using `winit` plus `pixels`. Done.
- Feed H.264 frames from transport into decoder. Done.
- Preserve aspect ratio — letterbox scaling to window size. Done.
- Add frame timing and dropped-frame counters. Deferred to M5 polish.
- Add manual reconnect and clear error states. Deferred to M4.

Exit criteria:

- Desktop shows live Android screen at interactive latency on LAN. ✓
- Rotation or format changes do not require restarting the desktop app. Partial — format changes tolerated; explicit rotation renegotiation deferred.
- Renderer does not block the network receive loop. ✓ (channel + redraw loop decoupled)

### M3: Desktop Input Control

Goal: control Android from the desktop window.

Status: **implemented, pending device validation**. Desktop captures pointer, wheel, text, and
navigation shortcuts; Android receives `InputEvent` messages over the current TCP stream after
pairing authentication and dispatches them through `RemoteControlAccessibilityService`.

Tasks:

- Capture mouse position, button, drag, wheel, and keyboard events in desktop app. Done.
- Convert desktop coordinates to Android display coordinates. Done.
- Send `InputEvent` messages over reliable control transport. Done over the existing TCP connection.
- Add Rust inbound input receiver on Android. Done.
- Add Java dispatch bridge from native input events to `RemoteControlAccessibilityService`. Done.
- Add basic keyboard text input. Partial — accessibility `ACTION_SET_TEXT`, IME deferred.
- Add back/home/recents shortcuts in desktop app. Done.
- Validate on emulator and physical Android device. Pending.

Exit criteria:

- Desktop click taps Android at the correct location. Pending validation.
- Desktop drag performs Android drag/swipe. Pending validation.
- Desktop wheel scrolls the active Android content where accessibility permits it. Pending validation.
- Desktop keyboard can enter text into a focused Android field. Pending validation.
- Back/home/recents work from desktop controls. Pending validation.

### M4: Pairing, Trust, And Session Recovery

Goal: make the MVP safe enough for repeated local use.

Status: **partial**. Ephemeral pairing-code authentication gates input for each connection. Durable
paired identity storage, per-session keys, encrypted transport, and reconnect recovery are not
implemented yet.

Tasks:

- Add manual pairing code flow. Partial — desktop prints a code; Android enters it.
- Store paired desktop identity on Android.
- Authenticate each session before accepting video or input traffic. Partial — input is gated;
  video remains available during pairing.
- Add heartbeat and reconnect behavior. Partial — heartbeat is implemented and visible; reconnect is
  pending.
- Stop input processing immediately when a session is unauthenticated or disconnected. Done for the
  current connection.
- Add visible Android status for connected desktop identity. Partial — current status shows address
  and pairing state; persisted desktop identity is pending.
- Add desktop status line for connected/disconnected/authenticated state. Done in the window title.

Exit criteria:

- Unknown clients cannot send input. Partial — true for the current pairing-code gate; persistent
  trust is pending.
- Previously paired desktop can reconnect without repeating full setup.
- Revoking permissions or dropping the network produces clear user-visible state.

### M5: MVP Polish And Test Pass

Goal: make MVP 1 coherent enough to demo and iterate on.

Tasks:

- Add Android permission preflight checklist.
- Add desktop connection settings screen.
- Add basic logs and troubleshooting output.
- Add focused unit tests for protocol, coordinate mapping, and session state.
- Add manual QA script for real-device testing.
- Document known limitations.

Exit criteria:

- A new developer can build and run both sides from docs.
- A real Android device can be mirrored and controlled from desktop on LAN.
- Known limitations are documented rather than surprising.

## Definition Of Done For MVP 1

MVP 1 is done when all of the following are true:

- Android app can be installed on a non-rooted device.
- User can grant capture and accessibility permissions.
- Desktop app can connect to the Android app on the same LAN.
- Desktop renders the Android screen continuously.
- Desktop input can tap, drag, scroll, type text, and trigger Android navigation actions.
- Session can stop and restart without requiring app reinstall or device reboot.
- The system rejects unauthenticated input.
- Build and run instructions work from a clean checkout with required toolchains installed.

## Risks And Mitigations

Android capture lifecycle:

- Risk: `MediaProjection` grants are one-session resources and can be revoked.
- Mitigation: keep capture ownership in `MirrorService`, treat every start as a fresh grant, and make stop/restart paths explicit.

Input reliability:

- Risk: accessibility gestures are not equivalent to kernel-level input and can fail in some contexts.
- Mitigation: keep rootless accessibility as MVP behavior, document limits, and add optional advanced input backends later.

Latency:

- Risk: H.264 encode, transport buffering, decode, and render can add visible delay.
- Mitigation: measure timestamps in protocol, avoid blocking receive loops, and prefer low-latency encoder settings.

Desktop decode complexity:

- Risk: cross-platform hardware decode can consume time.
- Mitigation: start with one decoder backend and one target desktop OS, then abstract after the path is proven.

Security:

- Risk: remote input over LAN is sensitive.
- Mitigation: do not accept input before pairing/authentication; keep discovery separate from trust.

Android device variance:

- Risk: encoder behavior differs across devices.
- Mitigation: log selected media format, support format changes, and keep fallback settings simple.

## Suggested Implementation Order

1. Wire Android native sender to desktop receiver over TCP. ✓
2. Prove real-device frame delivery with logs. ✓ (emulator)
3. Add desktop decode/render for H.264. ✓
4. Add desktop-to-Android input transport. ✓
5. Add Android input dispatch bridge. ✓
6. Add ephemeral pairing-code input authentication. ✓
7. Validate input end-to-end on a physical Android device.
8. Persist paired desktop identity and derive per-session keys.
9. Add reconnect/session recovery.
10. Add rotation/resolution renegotiation.
11. Replace accessibility text with IME-backed input path.
12. Replace or extend TCP transport with QUIC.
13. Add QA scripts and polish.

Input authentication comes before QUIC because unauthenticated input is the higher-risk exposure on
LAN. QUIC and persistent session keys are still needed before public testing.

## Target Manual QA Script

1. Install Android debug APK on a physical Android device.
2. Start desktop app on the same LAN.
3. Start Android app, enter the desktop host and pairing code, then connect to desktop.
4. Confirm desktop logs pairing success.
5. Grant screen capture.
6. Enable accessibility service.
7. Confirm desktop receives `DeviceHello`, `VideoFormat`, and frames.
8. Confirm desktop renders live screen.
9. Click an Android button from desktop and confirm the tap coordinate.
10. Drag across a scrollable list.
11. Use wheel or touchpad scroll over scrollable content.
12. Type into a focused text field, including Backspace, Delete, Enter, and Tab.
13. Trigger Back, Home, Recents, and Lock from desktop controls.
14. Rotate Android device and confirm the desktop video path remains usable.
15. Stop mirroring from Android notification.
16. Restart mirroring and reconnect.

## Open Questions

- Initial desktop target: Linux only first, or Windows/macOS from day one?
- Decoder choice: FFmpeg, GStreamer, or platform-specific decoder first?
- Transport target before public testing: QUIC immediately, or TCP until video and input are stable?
- Keyboard input priority: text-only first, or full keycode/modifier mapping?
- Should the Android app initiate the connection, or should desktop discover and connect to Android?
