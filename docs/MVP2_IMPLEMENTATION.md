# MVP 2 Implementation Notes

Last updated: 2026-05-22.

## Assumptions

- Emulator and physical-device validation are intentionally skipped for this pass.
- Local verification means Rust tests/builds and Android Gradle debug assembly.
- MVP 2 utility traffic uses typed protocol payloads. Java may pass JSON across JNI as a binding
  detail, but Rust converts it into protocol structs before anything is sent to a desktop.
- Platform-limited features are exposed as explicit `Available`, `Disabled`,
  `PermissionRequired`, or `Unsupported` feature states instead of silent no-ops.

## Implemented Local Surfaces

- Battery/device status is collected on Android and sent as `DeviceStatus` after desktop
  authentication.
- Media status is collected through Android notification-listener media sessions. Desktop media keys
  send `MediaControl`; Android dispatches public media key events.
- Text clipboard sync is explicit and foreground-driven on Android. Desktop-to-Android text uses the
  Android clipboard manager.
- File transfer uses `FileTransferStart`, `FileTransferChunk`, and `FileTransferComplete`.
  Android share intents and picked files stream to the desktop. Dragging a desktop file onto the
  viewer sends it to Android app storage.
- Remote file browsing and mutation are implemented for AndroidConnect app-private storage, including
  received desktop files. Broader storage remains behind future SAF document-tree grants.
- Notification mirroring uses a notification listener service and sends posted/removed notification
  payloads. Basic action/reply dispatch is wired where Android exposes safe notification actions.
- Recent photo/video listing uses Android media permissions and `MediaStore`, returning
  `PhotoAssetList` metadata.
- SMS/MMS/RCS, calls, app-specific windows, audio forwarding, relay, and multi-client state have
  typed protocol/status surfaces with explicit Android permission or support boundaries.

## Current Limits

- Audio forwarding is not streaming audio frames yet. It is exposed as disabled until a supported
  MediaProjection playback-capture path is enabled.
- SMS/MMS/RCS is not enabled as a default-SMS-app flow. RCS remains provider-limited.
- Call control is limited to public dial/status surfaces unless the device grants the required phone
  role and permissions.
- App-specific windows currently mean launching/focusing an app and controlling it through the mirror
  stack; independent app streaming is still device/API-limited.
- Multiple trusted desktops can be represented, but this transport still has one active Android
  connection at a time. Multi-client input ownership is represented in protocol state for the next
  transport step.
- Remote relay/NAT traversal is disabled by default and has typed status only; no relay service is
  configured in this repository.

## Local Verification

Run from the repository root unless noted:

```bash
cargo test --workspace
```

```bash
cd apps/android
./gradlew :app:assembleDebug
```

No `adb`, emulator, or physical-device validation was run for this milestone.
