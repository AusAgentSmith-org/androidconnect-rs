# AndroidConnect RS

Rust-first Android to desktop mirroring with remote input control.

Open, local-first, Rust-based Android screen mirroring and desktop control for non-rooted Android devices. Not a Phone Link clone and not a KDE Connect port — a permissively licensed stack that covers the core mirroring/input use case first, without the GPL boundary or proprietary platform requirements of either competitor. See `docs/FEATURE_POSITIONING.md` for the full product thesis and feature roadmap.

## What Exists

- `crates/protocol`: length-prefixed binary protocol, video metadata/frame messages, input event schemas, identity-bound pairing messages, trusted-session auth, and session-key derivation helpers.
- `crates/android-native`: JNI bridge loaded by the Android app. It connects to the desktop receiver, sends video protocol envelopes, persists trusted desktop identity in app storage, reads inbound authenticated input envelopes, and tracks capture/heartbeat stats.
- `apps/android`: Android app using `MediaProjection`, a foreground service, `MediaCodec` H.264 surface encoding, and an `AccessibilityService` for remote input. End-to-end frame streaming validated on emulator; the physical-device input pass is deferred. The status panel refreshes from native status callbacks with polling fallback and includes heartbeat counters.
- `apps/desktop-viewer`: Rust desktop viewer that decodes incoming H.264 frames with OpenH264, renders them live in a native window (`winit` + `pixels`), letterbox-scales the Android screen, displays pairing/connection/video/heartbeat state in the window title, and sends desktop input back to Android only after authentication succeeds.

## Planning Docs

- `docs/FEATURE_POSITIONING.md`: product thesis, feature matrix vs Phone Link/KDE Connect, scope decisions, and licence targets.
- `docs/PROJECT_PLAN.md`: structured scope, milestones, architecture, risks, and definition of done.
- `docs/MVP1.md`: short tactical backlog for the current MVP slice.
- `docs/MVP2.md`: expanded post-MVP1 utility, advanced integration, relay, and multi-client scope.
- `docs/INPUT_CONTROL.md`: current desktop input controls, architecture, limitations, and validation checklist.

## Build Rust

```sh
cd androidconnect-rs
cargo test
cargo run -p androidconnect-desktop-viewer -- 0.0.0.0:48172
```

The desktop viewer prints a six-digit pairing code at startup, creates a persistent local desktop
identity, and mirrors connection/pairing state in the window title. For deterministic test runs,
pass one explicitly:

```sh
cargo run -p androidconnect-desktop-viewer -- 0.0.0.0:48172 --pairing-code 123456
```

## Build Android Native Library

Install Android Rust targets and `cargo-ndk`, then run:

```sh
cd androidconnect-rs
./tools/build-android-native.sh
```

The script writes `.so` files into `apps/android/app/src/main/jniLibs`.

## Build Android App

Use the Gradle wrapper in `apps/android`:

```sh
cd androidconnect-rs/apps/android
./gradlew :app:assembleDebug
```

The app can launch without the native library, but encoded frames are dropped until `libandroidconnect_android_native.so` is packaged.

## Current Manual Run Flow

1. Start the desktop receiver:

   ```sh
   cd androidconnect-rs
   cargo run -p androidconnect-desktop-viewer -- 0.0.0.0:48172
   ```

2. Build and install the Android app with the native library packaged.
3. Open AndroidConnect on the phone.
4. Enter the desktop LAN IP and port `48172`.
5. Enter the pairing code printed by the desktop viewer. After a successful first pairing, later
   reconnect attempts can leave the code blank and use the stored desktop trust.
6. Tap `Connect desktop`.
7. Tap `Start mirroring` and approve screen capture.
8. The desktop window shows the live Android screen letterboxed to fit.
9. Enable the Android accessibility service before testing remote input.
10. If the TCP session drops unexpectedly, Android retries the last desktop endpoint with backoff
    until `Disconnect desktop` is tapped.

## Desktop Input Controls

With the Android accessibility service enabled, the desktop window sends:

- left-click taps and left-button drags
- mouse wheel / touchpad scroll actions
- typed text, Backspace, Delete, Enter, and Tab into focused editable fields
- Android Back, Home, Recents, and Lock actions

Fallback keyboard shortcuts:

- `Ctrl+Alt+B`: Back
- `Ctrl+Alt+H`: Home
- `Ctrl+Alt+R`: Recents
- `Ctrl+Alt+L`: Lock Screen

See `docs/INPUT_CONTROL.md` for the full control reference, limitations, and validation checklist.

## MVP 1 Remaining Work

1. Add rotation/resolution renegotiation.
2. Replace accessibility `ACTION_SET_TEXT` with an IME-backed text input path.
3. Replace TCP with encrypted/authenticated QUIC streams.
4. Add a deferred physical-device QA pass for desktop input control.
