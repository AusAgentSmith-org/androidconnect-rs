# MVP 3 Implementation Plan

Last updated: 2026-05-23.
Status: **planning** — no code work has begun. Implementation is blocked on MVP 2 entry criteria
(physical-device validation of utility messages, input path, and reconnect/session recovery).

This document is the concrete implementation plan for [MVP3.md](MVP3.md). It pins the open
architectural decisions, lists protocol gaps that must close before UI work, and breaks the three
delivery lanes into ordered tasks against the current code surface.

## Decisions (locked)

| Decision | Choice | Rationale |
| --- | --- | --- |
| Desktop UI framework | **`egui`** via `eframe` (`egui-wgpu` backend) | Pure Rust, no new runtime dep, embeds in the existing `winit`/`pixels` stack. Streaming canvas stays `pixels`-backed inside an `egui` paint callback. Doc-recommended option. Re-evaluate at the end of lane 3B if notification/file panels feel constrained. |
| Companion shell vs streaming canvas | Single `eframe` window hosts the companion shell. Streaming opens in a separate `winit` window owned by the same process. | Avoids forcing the streaming hot path through `egui` repaint cadence. The companion shell can stay open without a stream. |
| QR payload encoding | JSON inside a `androidconnect://pair?...` URI, base64url'd into the QR. | URI scheme works for deep-links from camera apps; JSON keeps the payload extensible without bumping protocol version. |
| Pairing token lifetime | Match MVP 2 pairing-code TTL (no change). | QR rotation key is the same token, so the lifetimes are tied. |

## Entry Criteria Gate

MVP 3 implementation **does not start** until all of the following are confirmed on a physical
Android device:

- [ ] `DeviceStatus` updates arrive within 2 s of state change.
- [ ] Desktop-to-Android input (pointer + key) is correct across letterboxed scaling.
- [ ] A desktop client survives a 30 s network drop and resumes without re-pairing
      (`trusted_session_auth_response` path).
- [ ] Notification mirror posts/removes and basic action dispatch round-trip cleanly.
- [ ] Clipboard text sync is reliable in both directions on the target Android version.

`docs/MVP2_IMPLEMENTATION.md` currently lists these as "skipped for this pass." That validation is
the prerequisite. Until it lands, this plan is reviewed only — not implemented.

## Protocol Gaps to Close First (Lane 3A prework)

These are additive changes to `crates/protocol/src/lib.rs`. They land before any UI work so the
shell can subscribe to real data on day one.

1. **Extend `DeviceStatus`** (`lib.rs:317`) with optional fields:
   - `wifi_state: Option<WifiState>` — `{ connected: bool, ssid: Option<String>, signal_strength: Option<i32> }`
   - `bluetooth_state: Option<BluetoothState>` — `{ enabled: bool, connected_devices: u8 }`
   - `dnd_state: Option<DndState>` — `{ enabled: bool, mode: DndMode }` where `DndMode` is
     `Off | Priority | Alarms | TotalSilence`.
   - `volume: Option<VolumeState>` — `{ media_percent: u8, ring_percent: Option<u8>, max_percent: u8 }`
   All `Option<…>` to preserve wire compatibility with current MVP 2 senders.

2. **Extend `AudioControl`** (`lib.rs:532`):
   - Replace the bare `AudioControlCommand` enum with a struct that supports `SetVolume { percent: u8 }`,
     `SetDnd { mode: DndMode }`, `SetBluetooth { enabled: bool }`, keeping the existing
     `Start/Stop/Mute/Unmute` variants for backward compat.
   - Rename to `QuickSetting` if we end up with a mix that's not strictly "audio". For MVP 3A keep
     the `AudioControl` name to avoid touching the payload tag.

3. **Notification per-app disable**: add a new `NotificationFilterUpdate` payload:
   - `{ package_name: String, enabled: bool }` — desktop tells Android to suppress mirroring for
     that package. Android persists the list locally.

4. **QR payload schema** (`crates/protocol/src/lib.rs::qr_pairing` or new `qr.rs` module):
   ```rust
   pub struct QrPairingPayload {
       pub protocol_version: u16,
       pub addresses: Vec<String>,  // IP:port pairs (IPv4 + IPv6 if available)
       pub pairing_token: String,   // same string the user would otherwise type
       pub desktop_id: String,
       pub desktop_name: String,
       pub issued_unix_ms: u64,
       pub expires_unix_ms: u64,
   }
   ```
   With `encode_qr_payload(&QrPairingPayload) -> String` returning the
   `androidconnect://pair?v=…&data=<base64url-json>` URI used inside the QR image.

5. **Protocol version bump**: `PROTOCOL_VERSION` (`lib.rs:7`) goes from `4` → `5`. Capability
   negotiation already handles forward-compat; older clients ignore the new optional fields.

Each of the above lands as its own PR with unit tests in `crates/protocol/tests/`.

## Lane 3A — QR Pairing + Permission Onboarding + Shell Skeleton

Release bar: shell opens, device connects, status bar shows battery/wifi/BT. No streaming required.

### Desktop work (`apps/desktop-viewer`)

1. **Crate refactor** — split `main.rs` (1162 lines) into modules:
   - `app/` — `eframe::App` impl, navigation rail, panels.
   - `streaming/` — extract current `pixels`/`winit` streaming loop into a `StreamingWindow` that
     can be opened/closed independently from the shell.
   - `network.rs` stays as-is; expose a clean channel-based API for the UI to consume.
2. **Add `eframe` + `qrcode` deps**:
   ```toml
   eframe = { version = "0.27", default-features = false, features = ["default_fonts", "wgpu"] }
   egui_extras = { version = "0.27", features = ["image"] }
   qrcode = "0.14"
   image = { version = "0.25", default-features = false, features = ["png"] }
   rfd = "0.14"  # native file pickers / drag targets
   ```
3. **Shell layout** (matches MVP3.md ASCII):
   - Top bar: app name + connected device name + connection dot.
   - Left nav rail: Notifications · Messages · Files · Phone · Mirror.
   - Center: active panel; empty-state shows QR pairing screen when no device.
   - Bottom status bar: battery · wifi · bluetooth · DND · volume, sourced from `DeviceStatus`.
4. **Window state persistence** — save last size/position to
   `~/.config/androidconnect/window.json` (use the existing `trust.rs` config dir helper).
5. **QR pair screen**:
   - Render `QrPairingPayload` URI as a 256×256 QR via `qrcode` → `image::DynamicImage` →
     `egui::TextureHandle`.
   - Show the same payload as a copyable IP+token line for the manual fallback path.
   - Auto-refresh QR when the pairing token rotates.

### Android work (`apps/android`)

1. **Onboarding activity** — new `OnboardingActivity` shown on first launch and re-openable from
   settings. Sections:
   - Required: Notification listener, Accessibility service.
   - Optional per feature: SMS (RECEIVE_SMS, READ_SMS, SEND_SMS), Contacts (READ_CONTACTS),
     Storage (READ_MEDIA_IMAGES/VIDEO on API 33+, MANAGE_EXTERNAL_STORAGE fallback prompt),
     Microphone (RECORD_AUDIO), Bluetooth (BLUETOOTH_CONNECT), Phone (READ_PHONE_STATE,
     CALL_PHONE).
   - Each card: live status chip + button that either triggers the system prompt or deep-links
     to the relevant `Settings` screen.
   - Accessibility card: instructional copy + `Settings.ACTION_ACCESSIBILITY_SETTINGS` intent.
2. **Permission status polling** — `MainActivity` and `OnboardingActivity` share a
   `PermissionStatusRepository` that re-queries on `onResume`. Feed grant state into
   `UtilityFeature` → `FeatureStatus` so the desktop shell shows the same truth.
3. **Required-permission gates** — when a feature is invoked without its permission, surface an
   in-app card with the same deep-link instead of a silent failure (touches
   `AndroidUtilityBridge.java`, `AndroidConnectNotificationService.java`).
4. **QR scan flow**:
   - `Scan to connect` button on first screen → `ScanQrActivity` (CameraX + ML Kit barcode).
   - On scan: parse `androidconnect://pair?...` → `QrPairingPayload` → pre-fill the existing
     pairing flow and auto-submit.
   - Manual entry remains as a secondary action.
5. **WiFi / BT / DND / Volume publishers** — extend `AndroidUtilityBridge.java`:
   - WiFi state via `WifiManager.getConnectionInfo()` + `ConnectivityManager.NetworkCallback`.
   - Bluetooth via `BluetoothAdapter` and `BluetoothManager` (gated on `BLUETOOTH_CONNECT`).
   - DND via `NotificationManager.getCurrentInterruptionFilter()` + listener registration.
   - Volume via `AudioManager.STREAM_MUSIC` levels + `VOLUME_CHANGED_ACTION` broadcast.
   All push into the `DeviceStatus` payload sent on the existing utility channel.

### Tests

- `crates/protocol/tests/qr_payload.rs` — round-trip QR encode/decode, URI escaping, version
  mismatch rejection.
- `crates/protocol/tests/device_status.rs` — bincode round-trip with the new optional fields,
  plus a "old-client decoding new payload" compat case.
- Android JUnit: `OnboardingActivityTest` verifies permission card state transitions.

### Exit criteria for 3A

- Companion shell opens to QR screen when unpaired; closes back to it when device drops.
- Scanning the QR on Android with one tap completes pairing and reaches the dashboard.
- Status bar updates within 2 s of toggling WiFi / BT / DND / volume on the phone.
- First-launch onboarding shows each permission with correct grant status; revoking permissions
  on the phone reflects in both the onboarding screen and the desktop feature chips.

## Lane 3B — Notifications + Quick Settings + SMS

Release bar: notifications visible with dismiss/action; volume + DND controllable; SMS threads
visible and reply works.

### Notifications panel (P0)

- `panels/notifications.rs` — virtualized list (`egui::ScrollArea` + manual cull) of all active
  mirrored notifications.
- Each row: app icon (via `NotificationPosted.icon` bytes already mirrored), app name, title,
  body, age. Inline action buttons for `NotificationActionDescriptor` entries.
- Quick reply: action with `RemoteInput` shows an inline text field; submit sends
  `NotificationAction { … input: Some(text) }`.
- Per-app suppress: kebab menu → "Hide notifications from <app>" → emits
  `NotificationFilterUpdate { package_name, enabled: false }`. Android persists this in
  `SharedPreferences` and gates `AndroidConnectNotificationService.onNotificationPosted`.
- Global "Hide sensitive content" toggle in Settings → desktop redacts body text for entries
  flagged `EXTRA_SENSITIVE` until clicked.

### Quick settings panel (P1)

- `panels/phone.rs` with a vertical stack:
  - Media volume slider bound to `DeviceStatus.volume.media_percent`; commits via
    `AudioControl::SetVolume`.
  - DND segmented control (`Off / Priority / Alarms / Total`); commits via
    `AudioControl::SetDnd`.
  - Bluetooth toggle gated on `bluetooth_state.enabled` permission; commits via
    `AudioControl::SetBluetooth`.
- Each control shows a "pending" spinner until the next `DeviceStatus` confirms the change, then
  snaps to the new value. Failure → inline red error with the reason from
  `FeatureStatus::PermissionRequired { … }`.

### Android quick-setting bridges

- `AudioManager.adjustStreamVolume` / `setStreamVolume` for media volume.
- `NotificationManager.setInterruptionFilter` for DND (requires `ACCESS_NOTIFICATION_POLICY`
  granted via onboarding).
- `BluetoothAdapter.enable/disable` — note: deprecated on Android 13+. On unsupported versions,
  return `FeatureState::Unsupported` and the desktop disables the toggle.

### SMS panel (P1)

- `panels/messages.rs`:
  - Left column: `MessageThreadList` thread summaries (display name, snippet, unread badge).
  - Right column: thread detail; on open, send `MessageThreadOpen { thread_id }` (new payload —
    if Android needs lazy-load of history beyond the live `MessageEvent` stream).
  - Reply composer: sends `MessageSendRequest`.
  - MMS image attachments: render via `SharedContent.uri` resolved through the existing file
    transfer machinery.
- Empty-state when `MessageThreadList.status` is `PermissionRequired` → CTA opens the onboarding
  SMS card.

### Exit criteria for 3B

- Posting a notification on the phone shows it on the desktop within 1 s.
- Dismissing a notification from the desktop removes it from the phone shade.
- Quick-reply round-trips and the reply is visible in the originating app.
- Per-app suppress hides future notifications from that package; toggling back restores them.
- Volume slider changes physical media volume; DND toggle changes the phone's interruption
  filter; both reflect back into the status bar.
- SMS panel lists threads, opens a conversation, and successfully sends an outbound reply.

## Lane 3C — Files + Drag-and-Drop + Mirror Integration

Release bar: files browser usable with drag-out and drag-in; Mirror button starts streaming
inside the shell.

### Files panel

- `panels/files.rs`:
  - Breadcrumb path bar + back/forward stack.
  - Two-column layout: folder tree on the left, current folder contents on the right.
  - Sortable columns: name, size, modified, type icon.
  - Right-click context menu: Download, Upload here, Rename, Delete, New folder, Open in default app.
  - Inline transfer progress bars (one per active `transfer_id`).
- Backed by `FileBrowseRequest` / `FileBrowseResponse` / `FileMutation` already in protocol.

### Drag-and-drop

- **Drop-in** (desktop OS → panel): `egui` exposes `ctx.input(|i| i.raw.dropped_files)`. Each
  dropped path becomes a `FileTransferStart` (Desktop→Android) directed at the current folder.
- **Drag-out** (panel → desktop OS): `egui` does not natively support drag-out to the OS. Use
  `rfd::FileDialog::save_file` triggered on drag-start, or fall back to a "Drag to copy" button
  that writes the file to a temp dir and then invokes the platform drag source (windows: COM
  `IDataObject`; linux: XDND via `arboard`/`smithay-clipboard`; macOS: `NSPasteboard`). For 3C
  initial cut, ship the **save-dialog fallback** and treat true OS drag-out as a follow-up if
  egui doesn't grow native support in time.

### Mirror panel + streaming integration

- `panels/mirror.rs` shows a connection summary and a `Start mirroring` button.
- Clicking it opens the existing `winit`/`pixels` streaming window in a new top-level window
  (separate from the `eframe` shell). Closing the streaming window leaves the shell open.
- Streaming window inherits the existing MVP 1 input + heartbeat path verbatim — no rewrites.

### Exit criteria for 3C

- File tree loads within 500 ms of opening the panel on a typical phone.
- Drop-in: dragging a file from the desktop OS file manager onto the panel uploads it to the
  current folder with a visible progress bar.
- Drag-out (save-dialog fallback acceptable): user can pick "Download" or drag a row and pick a
  destination; file lands on disk.
- Mirror button opens the streaming window; closing the streaming window returns to the shell
  without disconnecting the session.
- Rename / delete / create-folder operations succeed and update the panel.

## Cross-Cutting Concerns

### Security

- QR token reuse: a single token may only complete one successful pair. Server invalidates on
  success and rotates.
- Pre-auth surface: the desktop shell must render `None` for notification body, SMS body, and
  file contents until `AuthResult.accepted == true`. Empty-state panels show "Connecting…" /
  "Authenticating…" rather than blanking.
- Drag-out: file content is fetched from Android only after the user confirms a save destination.
  The temp file is `unlink`'d after the drop completes.
- Quick settings: any state-change message from an unauthenticated session is dropped on the
  Android side (already enforced by the auth gate; extend tests in `android-native` to confirm).

### Reconnect behavior

- Shell stays alive on disconnect; status bar shows "Reconnecting…" with a count of attempts.
- All panels switch to their cached-last-known state with a stale indicator.
- On successful reconnect via `trusted_session_auth_response`, panels resume; no QR is shown.
- On token expiry / explicit unpair, shell drops back to QR screen.

### Feature gating

- Each panel reads `DeviceStatus.features` to render an appropriate placeholder when a feature is
  `Disabled`, `PermissionRequired`, or `Unsupported`.
- The placeholder must include an action button that either triggers the Android onboarding deep
  link or surfaces the reason (e.g. "Requires Android 13+").

## Risks and Open Questions

| Risk | Mitigation |
| --- | --- |
| `egui` drag-out to OS is not native | Ship save-dialog fallback in 3C; track upstream `egui` drag-source work and revisit. |
| Two-window architecture (shell + streaming) feels fractured | If user feedback is poor after 3C, evaluate embedding `pixels` framebuffer inside an `egui::PaintCallback`. Defer until we have data. |
| Bluetooth toggle on Android 13+ | Fall back to deep-linking the system Bluetooth settings instead of failing silently. |
| QR payload leaking IP to anyone with the QR | Token TTL + single-use enforcement on the server side. Document that QR images should not be shared. |
| Protocol version bump breaks older paired devices | Optional fields preserve compat; older devices keep working on `v4` decoding. Confirm with the protocol round-trip test matrix before release. |

## Sequencing Summary

```
[Entry gate: MVP 2 physical-device validation passes]
          │
          ▼
[Protocol prework] → DeviceStatus extension, AudioControl extension,
                     NotificationFilterUpdate, QR payload, version 4 → 5
          │
          ▼
[Lane 3A] → Shell skeleton (egui), QR pair, onboarding, status bar
          │
          ▼
[Lane 3B] → Notifications panel, Quick settings panel, SMS panel
          │
          ▼
[Lane 3C] → Files panel + drag-and-drop, Mirror integration
          │
          ▼
[MVP 3 DoD review against docs/MVP3.md §"Definition of Done"]
```

## What This Plan Deliberately Does Not Do

- No new wire-level protocol messages beyond the four listed above. MVP 3 is a consumption layer
  on top of MVP 2 data.
- No system tray, no native OS notification bridge, no folder sync — all listed as out-of-scope
  in MVP3.md and explicitly deferred.
- No second simultaneous mirror window, no always-on background mirror.
- No replacement of `pixels`/`winit` for the streaming canvas; egui is additive, not a rewrite.
