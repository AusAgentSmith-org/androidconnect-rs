# AndroidConnect RS Project Plan

## Product Goal

Build a Rust-first alternative for Android to desktop mirroring with input control.

The product should let a user launch the Android app, approve screen capture and input permissions, connect from a desktop app on the same LAN, view the Android display in real time, and control the Android device with desktop mouse and keyboard input.

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
- Android accessibility service with tap, drag, and global action helpers.
- Gradle wrapper added to `apps/android` (copied from kdeconnect-android reference tree).

Not implemented yet:

- Desktop input capture and return transport.
- Android inbound input dispatch from Rust/network to Java accessibility APIs.
- Pairing and session authentication.
- Rotation/resolution renegotiation beyond initial format metadata.
- Physical device validation (emulator only so far).

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
  Rust inbound input receiver -> Java dispatch bridge -> AccessibilityService / IME

Shared Rust protocol
  DeviceHello
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
- Pointer down/up/move should be accumulated into tap or drag gestures on Android.
- Scroll should map to accessibility scroll actions against the active window.
- Text input should use an Android IME service once basic pointer control is proven.

Pairing and security:

- MVP can start with a manual pairing code displayed by Android and entered on desktop.
- A paired session should derive a shared session key before accepting input.
- Unauthenticated input packets must be ignored.
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
- Validate the path on a physical Android device. Done (emulator).

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

Tasks:

- Capture mouse position, button, drag, wheel, and keyboard events in desktop app.
- Convert desktop coordinates to Android display coordinates.
- Send `InputEvent` messages over reliable control transport.
- Add Rust inbound input receiver on Android.
- Add Java dispatch bridge from native input events to `RemoteControlAccessibilityService`.
- Add basic keyboard text input.
- Add back/home/recents shortcuts in desktop app.

Exit criteria:

- Desktop click taps Android at the correct location.
- Desktop drag performs Android drag/swipe.
- Desktop wheel scrolls the active Android content where accessibility permits it.
- Desktop keyboard can enter text into a focused Android field.
- Back/home/recents work from desktop controls.

### M4: Pairing, Trust, And Session Recovery

Goal: make the MVP safe enough for repeated local use.

Tasks:

- Add manual pairing code flow.
- Store paired desktop identity on Android.
- Authenticate each session before accepting video or input traffic.
- Add heartbeat and reconnect behavior.
- Stop input processing immediately when a session is unauthenticated or disconnected.
- Add visible Android status for connected desktop identity.

Exit criteria:

- Unknown clients cannot send input.
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

1. Wire Android native sender to desktop receiver over TCP.
2. Prove real-device frame delivery with logs.
3. Add desktop decode/render for H.264.
4. Add desktop-to-Android input transport.
5. Add Android input dispatch bridge.
6. Add pairing/authentication.
7. Replace or extend TCP transport with QUIC.
8. Add QA scripts and polish.

This order keeps the highest-risk pipeline visible early: capture, network delivery, decode, render, and input.

## Target Manual QA Script

1. Install Android debug APK on a physical Android device.
2. Start desktop app on the same LAN.
3. Start Android app and connect to desktop host.
4. Grant screen capture.
5. Enable accessibility service.
6. Confirm desktop receives `DeviceHello`, `VideoFormat`, and frames.
7. Confirm desktop renders live screen.
8. Rotate Android device and confirm desktop updates.
9. Click an Android button from desktop.
10. Drag across a scrollable list.
11. Type into a text field.
12. Trigger back, home, and recents.
13. Stop mirroring from Android notification.
14. Restart mirroring and reconnect.

## Open Questions

- Initial desktop target: Linux only first, or Windows/macOS from day one?
- Decoder choice: FFmpeg, GStreamer, or platform-specific decoder first?
- Transport target before public testing: QUIC immediately, or TCP until video and input are stable?
- Keyboard input priority: text-only first, or full keycode/modifier mapping?
- Should the Android app initiate the connection, or should desktop discover and connect to Android?
