# MVP 3 Scope

Last reviewed: 2026-05-23.

MVP 3 is the companion-app pivot. The desktop app stops being a streaming-first window and becomes a
full phone companion application — the primary surface for phone notifications, messages, status, and
settings — where screen mirroring is a feature you launch, not the default view.

The reference posture is Microsoft Phone Link: an always-open desktop panel that keeps the phone
present on the desktop without the user needing to pick up the phone. Streaming remains available and
important, but it moves to an opt-in mode inside the companion shell.

MVP 2 is not a prerequisite to begin MVP 3 design, but the backend protocol messages and Android
bridges introduced in MVP 2 are the data layer that MVP 3's UI surfaces. MVP 3 should not duplicate
that work.

## Product Goal

Transform AndroidConnect from a streaming tool into a desktop phone companion:

- The desktop app opens to a persistent companion panel, not a streaming window.
- Phone status, notifications, messages, and quick controls are visible at a glance.
- Screen mirroring and streaming are triggered from within the companion app.
- Pairing is QR-code-first; manual IP and token entry is retained as a fallback only.
- The Android app proactively guides the user through required permissions on first launch.
- File management has a proper desktop-side browser with drag-and-drop.

## Entry Criteria

MVP 3 should not begin implementation until:

- MVP 2A utility messages are stable: battery/device status, media controls, notifications, and
  clipboard sync work reliably over an authenticated session.
- The desktop-to-Android input path has been validated on at least one physical Android device.
- The reconnect/session-recovery path is solid enough that a companion panel can survive temporary
  disconnections without requiring the user to re-pair.
- The protocol has stable authenticated feature negotiation so companion surfaces can subscribe to
  only the streams they need.

## The Core Change: Companion-First Desktop App

The desktop app must be rebuilt as a companion shell. The current architecture — a single `winit`
window sized for a video frame — cannot host a multi-panel companion UI. MVP 3 requires choosing and
implementing a proper desktop UI framework alongside, or as a replacement of, the current rendering
approach.

The companion shell structure:

```
┌─────────────────────────────────────────────┐
│  AndroidConnect         [device name] ●      │
├──────────┬──────────────────────────────────┤
│          │                                  │
│  Nav     │   Main content panel             │
│  Rail    │   (Notifications / Messages /    │
│          │    Files / Settings / Mirror)    │
│ 📵 Notif │                                  │
│ 💬 SMS   │                                  │
│ 📁 Files │                                  │
│ ⚙  Phone │                                  │
│ 🖥  Mirror│                                  │
│          │                                  │
├──────────┴──────────────────────────────────┤
│  🔋 87%  📶 WiFi  🔵 BT  🔕 DND  Vol ████░ │
└─────────────────────────────────────────────┘
```

Status bar at the bottom is always visible. Navigation rail selects the active panel. Mirror mode
opens the current streaming window behavior inside or alongside the shell.

## Delivery Lanes

| Lane | Focus | Release bar |
| --- | --- | --- |
| MVP 3A | QR pairing + mobile permission onboarding + companion shell skeleton | Shell opens, device connects, status bar shows battery/wifi/BT. No streaming required to open. |
| MVP 3B | Notifications panel + quick phone settings + SMS panel | Notifications visible with dismiss/action. Volume and DND controllable. SMS threads visible and reply works. |
| MVP 3C | File/folder browser with drag-and-drop + streaming-as-feature integration | File browser works with drag-out to desktop and drag-in from desktop. Mirror button triggers streaming inside the shell. |

## Included

### QR Code Pairing

P0 for MVP 3.

The current pairing UX requires the user to find the desktop IP address and type a six-digit code
into the Android app. MVP 3 makes QR the primary pairing flow.

Scope:

- Desktop generates a QR code encoding its reachable IP addresses, port, and current pairing token.
- QR is shown prominently on the desktop companion app when no device is paired or when the user
  taps "Pair new device".
- Android app has a first-screen "Scan to connect" action that opens the camera, reads the QR, and
  initiates connection and pairing automatically.
- Manual IP + token entry remains available as a fallback for users who cannot use the camera flow.
- QR codes expire with the pairing token lifetime and refresh automatically.
- On reconnect to an already-trusted desktop, no QR or token entry is required.

Out of scope:

- Cloud-assisted or Bluetooth-assisted device discovery.
- NFC pairing.

### Mobile Permission Onboarding

P0 for MVP 3.

Currently the Android app silently fails or shows cryptic status text when a required permission is
not granted. MVP 3 adds guided first-launch permission onboarding.

Scope:

- On first launch, the Android app shows an explicit onboarding flow listing all permissions it may
  use and why each is needed.
- Permissions are grouped: required for core function (notification listener, accessibility service),
  optional for specific features (SMS, contacts, storage, microphone).
- Each permission group shows the current grant status and a button to open the correct Android
  Settings screen or trigger the system permission dialog.
- The accessibility service, which cannot be enabled programmatically, gets a dedicated in-app card
  with step-by-step instructions and a deep-link to the Accessibility Settings page.
- Required permissions that are not granted block the feature that needs them with a clear in-app
  prompt, not a silent failure.
- The onboarding flow can be re-opened from the Android app settings at any time.

Out of scope:

- Permissions that require device administrator or root.
- Automated accessibility service enable.

### Companion App Shell

P0 for MVP 3.

Scope:

- The desktop app opens to a companion panel rather than a blank streaming window.
- The shell has a navigation rail with sections: Notifications, Messages, Files, Phone Settings,
  Mirror.
- The shell is resizable and remembers its last size and position.
- When no device is connected, the shell shows the QR pairing screen.
- When a device is connected, the status bar shows battery percentage and charging state, WiFi
  signal, Bluetooth state, and Do Not Disturb state.
- The window title shows the connected device name.
- The companion shell and the streaming window can be open simultaneously; streaming is an optional
  overlay/sub-window, not the app's root view.

Desktop UI framework: **resolved — FluentGUI** (commit `2aa638f`, 2026-05-22). The companion
shell, panels, and streaming canvas all live inside a single FluentGUI window. The streaming
canvas is a `VideoFrame` GPU primitive that the decoded H.264 frames render into directly, so
`winit` + `pixels` are no longer in the picture. The framework comparison table further below
is kept for historical context but the decision is closed.

Out of scope:

- System tray integration.
- Platform notification bridge (showing Android notifications as native OS notifications).

### Phone Status Bar

P0 for MVP 3 (included in companion shell).

Scope:

- Battery percentage and charging indicator, always visible.
- WiFi connection state (connected/disconnected, signal strength if available).
- Bluetooth on/off state.
- Do Not Disturb active indicator.
- These map directly to `DeviceStatus` protocol messages already defined in MVP 2.

Out of scope:

- Cellular signal strength or carrier name (requires privileged Android APIs on most devices).
- Per-network WiFi details.

### Quick Phone Settings Panel

P1 for MVP 3.

Scope:

- Media volume slider: desktop can read and set Android media volume.
- Do Not Disturb toggle: desktop can enable/disable DND where Android permissions allow it.
- Bluetooth toggle: desktop can enable/disable Bluetooth where Android permissions allow it.
- Settings changes are applied immediately and confirmed by a status update from the phone.
- Failure or permission denial is surfaced in the panel, not silently ignored.

Out of scope:

- WiFi on/off toggle (Android restricts this to system apps on API 29+).
- Fine-grained DND mode configuration.
- App-specific notification settings.

Implementation note: volume and DND control require `AudioControl` protocol messages and Android
`AudioManager` / `NotificationManager` bridge work. Bluetooth toggle requires
`BLUETOOTH_CONNECT` permission and is subject to Android version limits.

### Notifications Panel

P0 for MVP 3.

This surfaces MVP 2 notification mirroring in the companion UI:

- A dedicated Notifications panel inside the companion shell shows all active mirrored notifications.
- Each notification shows app icon, app name, title, body text, and timestamp.
- The user can dismiss a notification from the desktop where Android allows it.
- Notification action buttons (reply, mark-read, etc.) are shown where Android exposes safe actions.
- Quick-reply from the desktop is supported where Android exposes a reply action.
- The user can disable notification mirroring per-app from within the panel.
- Sensitive content (e.g. messages from locked apps) can be hidden globally.

Out of scope:

- Notification history beyond the current connected session.
- Native OS notifications for incoming Android notifications.

### SMS Panel

P1 for MVP 3.

This surfaces MVP 2 SMS/MMS/RCS message data in the companion UI:

- A dedicated Messages panel shows SMS/MMS thread list where Android permissions and roles allow
  access.
- Each thread shows contact name/number, snippet, and unread count.
- Tapping a thread opens the conversation with full message history for that thread.
- The user can reply to a thread from the desktop; the reply is sent via Android.
- New incoming messages update the panel in real time.
- The panel shows a clear "SMS access not granted" state when the required Android permission is
  missing, with a link to grant it.
- MMS image attachments are shown as previews.

Out of scope:

- RCS features beyond what Android surfaces through the default messaging app APIs.
- Becoming the Android default SMS app.
- Message deletion.

### Screen Mirroring as a Feature

P1 for MVP 3.

Streaming is retained but moves from the app's primary mode to an explicit feature:

- The Mirror panel in the companion navigation shows a "Start mirroring" button.
- Tapping it triggers the existing screen capture permission flow and opens the streaming canvas.
- The streaming canvas can be a separate window or an embedded panel inside the companion shell —
  exact layout is a UX decision for implementation.
- The streaming canvas retains all existing MVP 1 behavior: letterbox scale, mouse/keyboard input,
  coordinate mapping, heartbeat.
- The Android app still shows its current mirroring status and controls.

Out of scope:

- Always-on background mirroring.
- Multiple simultaneous mirror windows.

### File and Folder Browser with Drag-and-Drop

P1 for MVP 3.

This extends MVP 2 file transfer and remote file browser into a proper desktop UI surface:

- A dedicated Files panel shows the Android file tree using the MVP 2 `FileBrowseRequest` /
  `FileBrowseResponse` protocol.
- Navigation: folders expand/collapse, breadcrumb path bar, back/forward.
- File metadata: name, size, modified date, MIME type icon.
- Download: user can double-click or right-click → "Download" to transfer files to a desktop folder.
- Upload: user can right-click → "Upload" or drag files from the desktop OS file manager onto the
  panel to transfer them to the current Android folder.
- Drag-out: user can drag a file from the Files panel and drop it onto the desktop OS file manager
  or another application; AndroidConnect fetches the file content and delivers it to the drop target.
- Operations: rename, delete (with confirmation), create folder, where Android permissions allow.
- Transfer progress is shown inline in the panel.
- Permission boundaries are shown explicitly when a path cannot be accessed.

Out of scope:

- Folder sync or background file backup.
- Access to private app data outside public Android APIs.
- Root filesystem browsing.

## Security and Privacy

MVP 3 inherits all MVP 2 security requirements and adds:

- QR codes encode only the information needed to initiate a pairing attempt (IP, port, one-time
  token). They do not encode persistent secrets or device identity.
- QR-initiated connections go through the same HMAC-SHA256 challenge/response as manual pairing.
- The companion shell must not show notification content, SMS body text, or file contents to an
  unauthenticated desktop.
- Drag-and-drop file transfer out of the file panel must use the same authenticated session as all
  other file transfer.
- Quick settings changes (volume, DND, Bluetooth) are only accepted from authenticated desktops.
- The Android permission onboarding flow must be honest about what each permission is used for. It
  must not request permissions that are not actively used.

## Protocol Direction

MVP 3 does not define new protocol message types. It consumes the protocol messages defined in MVP 2:

- `DeviceStatus` — battery, WiFi state, Bluetooth state, DND state, volume level
- `AudioControl` — set volume, set DND
- `NotificationPosted` / `NotificationRemoved` / `NotificationAction`
- `MessageThreadList` / `MessageEvent` / `MessageSendRequest`
- `FileBrowseRequest` / `FileBrowseResponse` / `FileMutation` / `FileTransferStart` / `FileTransferChunk` / `FileTransferComplete`

If a required `DeviceStatus` field (e.g. WiFi signal, Bluetooth state, DND flag) is missing from
the MVP 2 implementation, it should be added to `DeviceStatus` as part of MVP 3 lane A rather than
introducing a new message type.

New protocol work that MVP 3 does require:

- QR payload format: define the structure of the data encoded in the pairing QR (IP list, port,
  token, protocol version). This is not a network protocol message; it is an out-of-band bootstrap
  document.
- Audio control extension: if volume read/write and DND toggle are not yet covered by `AudioControl`
  messages, extend that message type.

## Desktop UI Framework Decision

This is a blocking architectural decision for MVP 3. Options:

| Option | Pros | Cons |
| --- | --- | --- |
| `egui` (immediate-mode) | Pure Rust, integrates with `pixels`/`winit`, lightweight | Not native-looking; custom widget work for rich panels |
| `iced` (Elm-style) | Pure Rust, retained-mode, good layout | Less mature; embedding streaming canvas requires care |
| Tauri (webview shell) | Rich web-stack UI, easy drag-and-drop | Adds webview dependency; Rust/JS bridge overhead |
| `slint` | Declarative, native-feel, used in rdpapp already | Commercial licence for some targets; less community tooling |

Recommendation: evaluate `egui` first given the existing `winit`/`pixels` stack and no additional
runtime dependency. If the notification and file panels prove too complex for immediate-mode, revisit
`iced` or Tauri. The `slint` option is worth considering given existing use in `rdpapp` in this workspace.

The streaming canvas (`pixels` framebuffer) must remain embeddable inside whatever framework is chosen.

## MVP 3 Definition of Done

MVP 3 is done when all of the following are true:

- The desktop app opens to a companion shell, not a streaming window.
- A device can be paired by scanning a QR code displayed on the desktop app with the Android camera;
  no manual IP or token entry is required for the QR flow.
- The Android app shows a first-launch permission onboarding screen that covers all features and
  links to the correct system settings for each.
- The companion shell status bar shows battery, WiFi state, Bluetooth state, and DND state in real
  time from a connected device.
- The desktop can adjust media volume and toggle DND via the Phone Settings panel.
- The Notifications panel shows mirrored phone notifications with dismiss and quick-reply where
  Android allows it, and per-app disable works.
- The Messages panel shows SMS/MMS threads and messages where Android grants access, and the user
  can send replies from the desktop.
- The Files panel shows the Android file tree, supports navigation, and supports file download,
  upload, rename, delete, and create-folder.
- Files can be dragged from the Files panel to the desktop and from the desktop into the Files panel.
- Screen mirroring is accessible via a Mirror button in the companion shell and works as it did in
  MVP 1/2.
- All companion surfaces degrade gracefully when a feature permission is not granted on Android, with
  a clear in-panel prompt rather than a silent blank state.
- Reconnecting after a temporary disconnect does not require re-pairing or re-entering the QR flow.
