# MVP 1 Tactical Plan

Goal: Android screen mirrored to a desktop window with desktop mouse and keyboard control.

The full structured plan lives in `docs/PROJECT_PLAN.md`. This file is the short tactical view for the current MVP slice.

## Current Slice — Complete

- Android obtains user consent for screen capture. ✓
- A foreground service owns the `MediaProjection` session. ✓
- `MediaCodec` encodes the full display to H.264 from a `VirtualDisplay` surface. ✓
- Encoded frames are passed through JNI into Rust-owned protocol types. ✓
- Android opens a TCP connection to the desktop receiver and sends `DeviceHello`, `VideoFormat`, and `VideoFrame` envelopes. ✓
- Android exposes an accessibility service with tap, drag, and global navigation helpers. ✓
- Desktop has a Rust receiver that decodes H.264 and renders frames live in a native window. ✓
- Desktop rejects unsupported protocol versions. ✓
- Desktop letterbox-scales incoming frames to fit the window at any size. ✓

Validated on emulator: Pixel 7 Pro AVD, API 36 (`google_apis_playstore`), streaming H.264 at 1080×2340/30fps, rendered live on desktop.

## Next Slice — M3: Desktop Input Control

Capture desktop pointer and keyboard events and deliver them to the Android accessibility service:

- Capture mouse click/drag/wheel events in the `winit` window event loop.
- Map desktop window coordinates to Android display coordinates accounting for letterbox offset and scale.
- Encode `InputEvent` messages and send them over the existing TCP connection.
- Add Rust inbound input receiver on Android side (`crates/android-native`).
- Bridge native input events to `RemoteControlAccessibilityService` (tap, drag, scroll, global actions).
- Back/home/recents shortcuts from desktop keyboard.

TCP is acceptable until M3 is stable. QUIC should replace it before public testing to give separate streams for video and input.

## Input Path

Desktop should send `InputEvent` messages:

- pointer down/up maps to `RemoteControlAccessibilityService.tap` or drag strokes
- wheel maps to accessibility scroll actions
- back/home/recents map to global actions
- text input should use an IME service in a later slice for reliable composition

## Known Android Constraints

- `MediaProjection` consent is per session; do not reuse old result data.
- Apps using `FLAG_SECURE` may render black frames.
- The capture service must run in the foreground with media projection service type.
- Accessibility must be explicitly enabled by the user before remote input can work.

## Immediate Backlog

1. Capture `winit` mouse/keyboard events and send `InputEvent` over TCP.
2. Add Android inbound input receiver and accessibility dispatch bridge.
3. Add connection status callbacks instead of polling `statsJson()`.
4. Add desktop frame-rate and byte-rate counters.
5. Validate on a physical Android device.

## Current Manual Run Flow

1. Start `androidconnect-desktop-viewer` on the desktop and bind `0.0.0.0:48172`.
2. Build and install the Android app with `libandroidconnect_android_native.so` packaged.
3. Enter the desktop LAN IP in AndroidConnect.
4. Tap `Connect desktop`.
5. Tap `Start mirroring` and approve screen capture.
6. Confirm the desktop logs `hello`, `video`, and `frame` messages.
