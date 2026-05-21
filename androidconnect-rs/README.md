# AndroidConnect RS

Rust-first Android to desktop mirroring with remote input control.

This is a clean MVP scaffold beside the KDE Connect source tree. The Android app uses platform APIs for capture and input permission surfaces; Rust owns the shared protocol, native bridge, and desktop-side process.

## What Exists

- `crates/protocol`: length-prefixed binary protocol, video metadata/frame messages, and input event schemas.
- `crates/android-native`: JNI bridge loaded by the Android app. It connects to the desktop receiver, sends protocol envelopes, and tracks capture stats.
- `apps/android`: Android app using `MediaProjection`, a foreground service, `MediaCodec` H.264 surface encoding, and an `AccessibilityService` for remote input. End-to-end frame streaming validated on emulator.
- `apps/desktop-viewer`: Rust desktop receiver that decodes incoming H.264 frames with OpenH264 and renders them live in a native window (`winit` + `pixels`). Letterbox-scales the Android screen to the window size.

## Planning Docs

- `docs/PROJECT_PLAN.md`: structured scope, milestones, architecture, risks, and definition of done.
- `docs/MVP1.md`: short tactical backlog for the current MVP slice.

## Build Rust

```sh
cd androidconnect-rs
cargo test
cargo run -p androidconnect-desktop-viewer -- 0.0.0.0:48172
```

## Build Android Native Library

Install Android Rust targets and `cargo-ndk`, then run:

```sh
cd androidconnect-rs
./tools/build-android-native.sh
```

The script writes `.so` files into `apps/android/app/src/main/jniLibs`.

## Build Android App

Use a Gradle installation or add a wrapper in `apps/android`, then run:

```sh
cd androidconnect-rs/apps/android
gradle :app:assembleDebug
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
5. Tap `Connect desktop`.
6. Tap `Start mirroring` and approve screen capture.
7. The desktop window shows the live Android screen letterboxed to fit.

## MVP 1 Remaining Work

1. Capture desktop mouse/keyboard events, encode `InputEvent`, and apply them through the Android accessibility service (M3).
2. Add pairing, session authentication, reconnect handling, and rotation/resolution renegotiation (M4).
3. Validate on a physical Android device.
