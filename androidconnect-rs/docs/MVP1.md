# MVP 1 Tactical Plan

Goal: Android screen mirrored to a desktop window with desktop mouse and keyboard control.

The full structured plan lives in `docs/PROJECT_PLAN.md`. This file is the short tactical view for the current MVP slice.
The detailed desktop input-control reference lives in `docs/INPUT_CONTROL.md`.

## Current Slice — Complete

- Android obtains user consent for screen capture. ✓
- A foreground service owns the `MediaProjection` session. ✓
- `MediaCodec` encodes the full display to H.264 from a `VirtualDisplay` surface. ✓
- Encoded frames are passed through JNI into Rust-owned protocol types. ✓
- Android opens a TCP connection to the desktop receiver and sends `DeviceHello`, `VideoFormat`, and `VideoFrame` envelopes. ✓
- Android and desktop perform an HMAC-SHA256 pairing-code challenge/response before input is accepted. ✓
- Android exposes an accessibility service with tap, drag, and global navigation helpers. ✓
- Desktop has a Rust receiver that decodes H.264 and renders frames live in a native window. ✓
- Desktop rejects unsupported protocol versions. ✓
- Desktop letterbox-scales incoming frames to fit the window at any size. ✓
- Desktop window title reports listening, connected, paired, video format, and error states. ✓
- Android status text summarizes capture, desktop connection, pairing, input service, counters, and
  last error. ✓

Validated on emulator: Pixel 7 Pro AVD, API 36 (`google_apis_playstore`), streaming H.264 at 1080×2340/30fps, rendered live on desktop. Physical-device validation is still pending.

## Current Slice — M3: Desktop Input Control

Capture desktop pointer and keyboard events and deliver them to the Android accessibility service:

- Capture mouse click/drag/wheel events in the `winit` window event loop. ✓
- Map desktop window coordinates to Android display coordinates accounting for letterbox offset and scale. ✓
- Encode `InputEvent` messages and send them over the existing TCP connection. ✓
- Add Rust inbound input receiver on Android side (`crates/android-native`). ✓
- Bridge native input events to `RemoteControlAccessibilityService` (tap, drag, scroll, global actions). ✓
- Back/home/recents shortcuts from desktop keyboard. ✓
- Validate input end-to-end on emulator/physical device.

TCP is acceptable for local validation. The current pairing-code input gate is enough to stop
accidental unauthenticated input, but persistent trust, encrypted transport, and session-key
derivation are still required before public testing. QUIC should replace TCP after that hardening
to give separate reliable/unreliable streams for control, input, and video.

The current pairing gate is ephemeral: the desktop prints a pairing code, Android sends a challenge,
the desktop proves knowledge of the code, and Android ignores input until the proof succeeds. It
does not persist trusted desktop identities and does not encrypt the TCP stream.

## Input Path

Desktop sends `InputEvent` messages over the same TCP connection currently used for video:

- left-button pointer down/up maps to `RemoteControlAccessibilityService.tap` or `drag`
- wheel maps to accessibility scroll actions against the active window
- typed text maps to accessibility `ACTION_SET_TEXT` on the focused editable node
- back/home/recents/lock map to Android accessibility global actions
- right/middle/back/forward mouse buttons are serialized but ignored by Android dispatch for now

Known shortcuts:

- `Esc`, Back key, or Browser Back key -> Android Back
- Home key -> Android Home
- App Switch key -> Android Recents
- Power / Standby key -> Android Lock Screen
- `Ctrl+Alt+B/H/R/L` -> Back/Home/Recents/Lock fallback shortcuts

## Known Android Constraints

- `MediaProjection` consent is per session; do not reuse old result data.
- Apps using `FLAG_SECURE` may render black frames.
- The capture service must run in the foreground with media projection service type.
- Accessibility must be explicitly enabled by the user before remote input can work.
- Accessibility text input does not preserve cursor position, selection, or IME composition.
- Scroll and gesture dispatch can fail where Android accessibility cannot act on the active UI.
- Input is gated by the pairing-code challenge/response, but the TCP stream is not encrypted or
  hardened against active man-in-the-middle attacks. Do not expose it on an untrusted network.

## Immediate Backlog

1. Validate desktop click/drag/wheel/text/global actions on a physical Android device.
2. Persist paired desktop identity and derive per-session keys beyond the current ephemeral input gate.
3. Add reconnect/session recovery and connection status callbacks (replace polling `statsJson()`).
4. Add rotation/resolution renegotiation.
5. Harden text input with an IME service instead of accessibility `ACTION_SET_TEXT`.
6. Add desktop frame-rate and byte-rate counters.
7. Replace single TCP stream with authenticated QUIC/control streams.

## Current Manual Run Flow

1. Start `androidconnect-desktop-viewer` on the desktop and bind `0.0.0.0:48172`.
2. Build and install the Android app with `libandroidconnect_android_native.so` packaged.
3. Enter the desktop LAN IP and the pairing code printed by the desktop viewer in AndroidConnect.
4. Tap `Connect desktop`.
5. Confirm the desktop logs `pairing authenticated; desktop input enabled` and the window title
   switches to `input paired`.
6. Tap `Start mirroring` and approve screen capture.
7. Confirm the desktop logs `hello`, `video`, and `frame` messages.
8. Enable the Android accessibility service.
9. Validate left-click tap, left-button drag, wheel scroll, text input, Back, Home, Recents, and Lock.
