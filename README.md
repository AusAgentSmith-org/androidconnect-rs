# AndroidConnect RS

Open, local-first, Rust-based Android screen mirroring and desktop control for non-rooted Android
devices. Not a Phone Link clone and not a KDE Connect port — a permissively licensed
(`MIT OR Apache-2.0`) stack built around the core mirroring and input use case, without the GPL
boundary or proprietary platform requirements of either alternative.

See [`docs/FEATURE_POSITIONING.md`](docs/FEATURE_POSITIONING.md) for the full product thesis and
feature roadmap.

> **Status:** MVP 1 in progress. End-to-end frame streaming is validated on emulator. Physical-device
> input QA pass is deferred. The transport is **not yet encrypted** — use only on a trusted LAN.

## What Exists

### Shared Rust crates (`crates/`)

- **`crates/protocol`** — length-prefixed binary protocol with video metadata/frame messages, input
  event schemas, identity-bound pairing, trusted-session authentication, and session-key derivation.
- **`crates/android-native`** — JNI bridge loaded by the Android app. Manages the TCP session,
  sends encoded video envelopes, receives authenticated input events, and persists paired desktop
  identity in app storage.

### Android app (`apps/android`)

Kotlin app using `MediaProjection`, a foreground service, `MediaCodec` H.264 surface encoding, and
an `AccessibilityService` for remote input. Includes:

- Screen capture consent flow and foreground service with keep-awake support.
- QR-code pairing scanner (ML Kit) and manual code entry.
- Battery, Wi-Fi, Bluetooth, DND, volume, and call-state status panel.
- File browser, file transfer, and share-sheet receive.
- Notification mirror, SMS/MMS thread list and read/reply, media controls.
- Automatic TCP reconnect with backoff. Heartbeat counters visible in the status panel.

> The Android app requests all permissions it will eventually use (MVP 2 features). At first launch
> only screen capture and accessibility are needed for mirroring. See
> [`docs/MVP2.md`](docs/MVP2.md) for which features the additional permissions enable.

### Desktop viewer (`apps/desktop-viewer`)

Rust desktop app (Fluent 2 / GPUI UI framework) that:

- Decodes H.264 frames with OpenH264 and renders the live Android screen.
- Letterbox-scales to any window size.
- Sends desktop mouse, scroll, keyboard, and navigation input back to Android after authentication.
- Shows phone status (battery, Wi-Fi, Bluetooth, DND, volume, keep-awake), storage breakdown, media
  controls, file browser, notifications, messages, and activity log in a sidebar.
- Supports QR-code pairing and manual pairing-code entry.
- Mirrors screen from the desktop (sends a `MirrorRequest` to wake the Android capture prompt).

The desktop viewer is a **standalone Rust workspace** under `apps/desktop-viewer/` — it is not a
member of the root Cargo workspace and must be built from its own directory.

## Security Notice

The current TCP transport is **not encrypted**. Identity-bound pairing (HMAC-SHA256 challenge) and
per-session key derivation gate desktop input, so an unknown client cannot inject input. However, the
video stream and control messages are readable on the wire. **Do not use AndroidConnect on an
untrusted network.** Replacing TCP with an encrypted/authenticated QUIC transport is an explicit MVP
1 blocker listed in [`docs/MVP1.md`](docs/MVP1.md).

## Prerequisites

| Requirement | Version | Notes |
|---|---|---|
| Rust | 1.88+ | `rustup update stable` |
| cargo-ndk | any recent | `cargo install cargo-ndk` |
| Android NDK | r27 (27.2.12479018) | Set `ANDROID_NDK_HOME`; matches `ndkVersion` in `app/build.gradle` |
| Android SDK | API 26+ target | `minSdk = 26` in `app/build.gradle` |
| Android device/emulator | API 26+ | Emulator: use a `google_apis_playstore` system image, not `ps16k` |
| Desktop OS | Linux | Tested on Linux. macOS and Windows require GPUI backend support and are untested. |

Android Rust build targets (add once):

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
```

## Build Rust Crates

```sh
# Protocol and native bridge crates (the root workspace)
cargo test --workspace
```

## Build Desktop Viewer

The desktop viewer is a separate workspace under `apps/desktop-viewer/`:

```sh
cd apps/desktop-viewer
cargo run -- 0.0.0.0:48172
```

For deterministic pairing code during testing:

```sh
cd apps/desktop-viewer
cargo run -- 0.0.0.0:48172 --pairing-code 123456
```

## Build Android Native Library

Install prerequisites above, then from the repo root:

```sh
./tools/build-android-native.sh
```

The script writes `.so` files into `apps/android/app/src/main/jniLibs/`.

## Build Android APK

```sh
cd apps/android
./gradlew :app:assembleDebug
```

The app launches without the native library, but encoded frames are silently dropped until
`libandroidconnect_android_native.so` is packaged. If you see no video, check `jniLibs/`.

## Manual Run Flow

1. Start the desktop viewer and note the six-digit pairing code it prints:
   ```sh
   cd apps/desktop-viewer
   cargo run -- 0.0.0.0:48172
   ```
2. Build and install the Android APK with the native library packaged.
3. Open AndroidConnect on the phone.
4. Enter the desktop LAN IP, port `48172`, and the pairing code.  
   On reconnects to an already-trusted desktop, leave the pairing code blank.
5. Tap **Connect desktop**.
6. Tap **Start mirroring** and approve screen capture.
7. The desktop window shows the live Android screen.
8. Enable the Android accessibility service (Android Settings → Accessibility → AndroidConnect)
   before testing remote input.
9. If the TCP session drops, Android retries automatically until **Disconnect desktop** is tapped.

## Desktop Input Controls

With the Android accessibility service enabled:

| Desktop input | Android action |
|---|---|
| Left click | Tap at the mapped coordinate |
| Left-button drag | Accessibility drag gesture |
| Mouse wheel / touchpad scroll | Accessibility scroll on the active window |
| Typed text | Appended to the focused editable field |
| Backspace / Delete | Removes one code point from the focused field |
| Enter / Tab | Sends newline / tab to the focused field |
| `Esc`, Back key, or Browser Back | Android Back |
| Home key | Android Home |
| App Switch key | Android Recents |
| Power / Standby key | Android Lock Screen |
| `Ctrl+Alt+B` | Android Back |
| `Ctrl+Alt+H` | Android Home |
| `Ctrl+Alt+R` | Android Recents |
| `Ctrl+Alt+L` | Android Lock Screen |

See [`docs/INPUT_CONTROL.md`](docs/INPUT_CONTROL.md) for the full reference, limitations, and
validation checklist.

## Known Limitations

- Transport is **not encrypted** (see Security Notice above).
- Text entry uses accessibility `ACTION_SET_TEXT` — does not preserve cursor position, selection, or
  IME composition. An IME-backed input path is planned.
- Physical-device input QA pass is deferred; validated on emulator only.
- Rotation/resolution renegotiation is not yet implemented.
- Desktop viewer is Linux-only in practice (GPUI backend coverage on other platforms is untested).
- `FLAG_SECURE` Android apps may render black frames.

## MVP 1 Remaining Work

1. Encrypted/authenticated transport (QUIC or TLS over TCP).
2. Rotation/resolution renegotiation.
3. IME-backed text input (replace accessibility `ACTION_SET_TEXT`).
4. Physical-device QA pass for desktop input control.

## Documentation

- [`docs/FEATURE_POSITIONING.md`](docs/FEATURE_POSITIONING.md) — product thesis, feature matrix vs Phone Link/KDE Connect, scope decisions, licence targets.
- [`docs/PROJECT_PLAN.md`](docs/PROJECT_PLAN.md) — structured milestones, architecture, risks, and definition of done.
- [`docs/MVP1.md`](docs/MVP1.md) — current tactical backlog and manual run flow.
- [`docs/MVP2.md`](docs/MVP2.md) — post-MVP1 utility features, relay, multi-client scope.
- [`docs/INPUT_CONTROL.md`](docs/INPUT_CONTROL.md) — input controls, architecture, limitations, and QA checklist.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
