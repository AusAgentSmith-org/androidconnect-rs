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
- Android and desktop perform an identity-bound HMAC-SHA256 pairing-code challenge/response before
  input is accepted. ✓
- Android persists trusted desktop identity, desktop persists paired Android trust, and both sides
  derive a per-session key fingerprint for authenticated sessions. ✓
- Android exposes an accessibility service with tap, drag, and global navigation helpers. ✓
- Desktop has a Rust receiver that decodes H.264 and renders frames live in a native window. ✓
- Desktop rejects unsupported protocol versions. ✓
- Desktop letterbox-scales incoming frames to fit the window at any size. ✓
- Desktop window title reports listening, connected, paired, video format, heartbeat, and error states. ✓
- Android status text summarizes capture, desktop connection, pairing, input service, counters, and
  last error, and refreshes while the activity is visible. ✓
- Desktop sends periodic TCP heartbeat pings, Android replies with pongs, and both surfaces report
  heartbeat health. ✓
- Android native connection/session status changes notify the visible activity immediately, with
  polling kept as a fallback. ✓

Validated on emulator: Pixel 7 Pro AVD, API 36 (`google_apis_playstore`), streaming H.264 at 1080×2340/30fps, rendered live on desktop. Physical-device validation is deferred for the current development pass.

## Current Slice — M3: Desktop Input Control

Capture desktop pointer and keyboard events and deliver them to the Android accessibility service:

- Capture mouse click/drag/wheel events in the `winit` window event loop. ✓
- Map desktop window coordinates to Android display coordinates accounting for letterbox offset and scale. ✓
- Encode `InputEvent` messages and send them over the existing TCP connection. ✓
- Add Rust inbound input receiver on Android side (`crates/android-native`). ✓
- Bridge native input events to `RemoteControlAccessibilityService` (tap, drag, scroll, global actions). ✓
- Back/home/recents shortcuts from desktop keyboard. ✓
- Validate input end-to-end on emulator/physical device.

TCP is acceptable for local validation. The current trusted-session input gate stops accidental
unauthenticated input, but the stream is still not encrypted or hardened against active
man-in-the-middle attacks. QUIC should replace TCP after reconnect/session recovery to give separate
reliable/unreliable streams for control, input, and video.

The current trust model: the desktop has a persistent local identity, Android has a persistent
app-install device id, first pairing derives and stores a shared secret on both sides, and later
connections can authenticate with that stored trust without re-entering the code. It still does not
encrypt the TCP stream.

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
- Input is gated by pairing/trusted-session authentication, but the TCP stream is not encrypted or
  hardened against active man-in-the-middle attacks. Do not expose it on an untrusted network.

## Immediate Backlog

1. Add reconnect/session recovery beyond the current manual connect/disconnect flow.
2. Add rotation/resolution renegotiation.
3. Harden text input with an IME service instead of accessibility `ACTION_SET_TEXT`.
4. Add desktop frame-rate and byte-rate counters.
5. Replace single TCP stream with encrypted/authenticated QUIC/control streams.
6. Run the deferred physical-device input QA pass.

## Current Manual Run Flow

1. Start `androidconnect-desktop-viewer` on the desktop and bind `0.0.0.0:48172`.
2. Build and install the Android app with `libandroidconnect_android_native.so` packaged.
3. Enter the desktop LAN IP and the pairing code printed by the desktop viewer in AndroidConnect.
   On later reconnects to an already trusted desktop, the pairing code can be blank.
4. Tap `Connect desktop`.
5. Confirm the desktop logs `pairing authenticated; desktop input enabled` and the window title
   switches to `input paired`.
6. Tap `Start mirroring` and approve screen capture.
7. Confirm the desktop logs `hello`, `video`, and `frame` messages.
8. Enable the Android accessibility service.
9. Validate left-click tap, left-button drag, wheel scroll, text input, Back, Home, Recents, and Lock.
