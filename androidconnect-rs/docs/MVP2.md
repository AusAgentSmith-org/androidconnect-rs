# MVP 2 Scope

Last reviewed: 2026-05-22.

MVP 2 is the first broad integration release after mirroring and desktop control. It should extend
the trusted AndroidConnect session with utility features, deeper phone-integration surfaces, optional
remote connectivity, and multiple trusted desktop clients.

MVP 2 is not allowed to weaken the MVP 1 promise: Android screen mirroring and desktop control must
remain the core product.

## Product Goal

Add opt-in integrations around an already paired AndroidConnect session:

- show basic device and battery status
- control active Android media playback
- forward Android audio to the desktop where Android permits it
- sync text and image clipboard content within Android platform limits
- send individual files through explicit user actions
- browse permitted Android files remotely
- browse and manage recent photos/gallery media
- mirror Android notifications with privacy controls
- support SMS/MMS/RCS where Android roles, providers, and permissions allow it
- support call surfaces where Android and desktop transports allow it
- open app-specific Android windows where a public or documented device-supported path exists
- support opt-in relay/NAT traversal for non-LAN sessions
- support multiple simultaneous authenticated desktop clients with clear control ownership

This is intentionally larger than MVP 1. Implement it in lanes, but treat every item above as MVP 2
scope unless the implementation proves impossible on stock Android without root or restricted OEM
APIs.

## Entry Criteria

MVP 2 should not start until MVP 1 is coherent enough to demo repeatedly:

- Physical Android device validation has passed for mirroring and desktop input.
- Reconnect/session recovery works beyond the current manual connect/disconnect flow.
- The final MVP 1 transport security direction is implemented or explicitly accepted for local
  testing. The current trusted-session input gate alone is not enough for public utility features.
- Rotation/resolution renegotiation and status/error reporting are good enough that utility features
  will not hide core mirroring failures.
- Android permission UX is explicit and reversible.
- The protocol has authenticated feature negotiation so each utility can be enabled, disabled, and
  audited independently.
- The desktop app has a place to expose device utility surfaces without cluttering the mirror window.

## Delivery Lanes

MVP 2 can be built in lanes without moving features out of MVP 2:

| Lane | Features | Release bar |
| --- | --- | --- |
| MVP 2A | Battery/device status, media controls, text clipboard, one-shot file transfer, notification mirror | Permission UX and authenticated utility messages are proven. |
| MVP 2B | Image clipboard, remote file browser, photos/gallery, notification actions, audio forwarding | Larger payload and richer Android permission flows work reliably. |
| MVP 2C | SMS/MMS/RCS, calls, app-specific windows, Internet relay/NAT traversal, multiple simultaneous desktop clients | High-privacy and multi-session behavior has explicit security review and clear limits. |

## Included

### Battery And Device Status

P0 for MVP 2.

Scope:

- Android reports battery percentage and charging state to the desktop.
- Android reports stable device identity already used by pairing, plus user-facing device name/model
  where available.
- Desktop shows status in a compact surface that does not compete with the mirroring window.
- Status updates are read-only; no power-management commands.

Out of scope:

- Battery optimization automation.
- OEM-specific device management.
- Cross-device account inventory.

### Media Controls

P0 for MVP 2.

Scope:

- Desktop can show current active media session metadata when Android exposes it.
- Desktop can send play/pause, previous, next, and stop where Android allows it.
- Failure states should be quiet and clear when no controllable media session exists.

Out of scope:

- Per-app audio routing.
- Media library browsing.
- DRM bypass or capture of protected audio/video.

### Clipboard Sync

P0/P1 for MVP 2, with Android limitations documented in-product and in docs.

Scope:

- Plain-text clipboard sync.
- Image clipboard sync where Android exposes clipboard image data or shareable content URIs.
- Prefer explicit or recently foregrounded sync behavior that respects Android clipboard access
  restrictions.
- Desktop and Android can each disable clipboard sync independently.
- Clipboard sync should have a clear privacy toggle and should not keep a clipboard history.

Out of scope:

- Clipboard history.
- Background polling that fights Android platform restrictions.
- Password-manager or sensitive-field bypasses.

### File Transfer And Share

P0 for MVP 2.

Scope:

- Send individual files from desktop to Android.
- Receive Android share intents into AndroidConnect and forward the selected file/content to the
  paired desktop when the user explicitly chooses AndroidConnect from the Android share sheet.
- Store received files in a predictable user-selected or app-specific location.
- Show transfer progress, completion, cancellation, and failure state.
- Share transfer infrastructure with the remote file browser where practical.

Out of scope:

- Folder sync.
- Drag/drop integration with every desktop file manager.
- Background backup or photo gallery sync.

### Notifications

P0/P1 for MVP 2.

P0 scope:

- User can grant and revoke Android notification listener access.
- Desktop can show mirrored notification app name, title, text summary, and timestamp.
- User can hide sensitive content globally.
- User can disable notification mirroring per app.

P1 scope:

- Dismiss notification from desktop where Android permits it.
- Trigger basic notification actions where Android exposes safe actions.
- Reply from desktop where Android exposes a safe reply action and the notification app supports it.

Out of scope:

- Notification storage/history beyond the current visible session.

### Audio Forwarding

P1 for MVP 2.

Scope:

- Forward Android playback audio to the paired desktop for supported app, mirror, or device audio
  paths.
- Keep audio forwarding independently toggleable from media controls and screen mirroring.
- Show active audio state, mute state, and failure state on desktop.
- Prefer low-latency streaming, but document latency separately from video latency.
- Stop audio forwarding immediately when the session is unauthenticated, disconnected, or disabled.

Limits:

- Do not bypass DRM, protected content, or Android privacy restrictions.
- If Android only exposes audio capture for selected devices, OS versions, or playback categories,
  document those limits as supported-device behavior rather than silently failing.

### App-Specific Android Windows

P1/P2 for MVP 2.

Scope:

- Open an Android app in a separate desktop window where a public Android API or documented
  supported-device path makes this possible.
- Prefer an app-focused surface built on the existing mirror/control stack before adding a separate
  app-streaming architecture.
- Support mouse and keyboard input against the app-specific window with the same authentication gate
  as the main mirror.
- Make device/API limitations visible in the UI and docs.

Limits:

- No root-only task control.
- No dependency on Microsoft Phone Link or restricted OEM packages.
- If true independent app windows require selected-device support, the feature can ship with an
  explicit supported-device matrix.

### SMS, MMS, And RCS

P2 for MVP 2.

Scope:

- Show SMS/MMS conversations where Android permissions and roles allow access.
- Send SMS/MMS from desktop where Android permissions, roles, and carrier behavior allow it.
- Receive new-message events and surface them through the desktop utility UI.
- Support RCS only where the Android device, messaging provider, and public integration surface allow
  it.
- Keep message access behind a separate permission and feature toggle.

Limits:

- RCS parity is best-effort and provider/device-limited.
- Do not scrape notification text as a substitute for a real messaging integration.
- Do not require becoming the default SMS app unless the implementation and UX explicitly accept
  that tradeoff.

### Calls

P2 for MVP 2.

Scope:

- Show incoming call state on desktop.
- Allow answer, decline, mute, and hang up where Android permissions and transport support it.
- Allow outbound dialing from desktop where Android permissions and roles allow it.
- Coordinate with media controls and audio forwarding so calls do not produce conflicting state.

Limits:

- Bluetooth, telecom, and permission behavior may vary by device and Android version.
- Emergency calling and carrier-specific call features are out of scope.
- No call recording.

### Photos And Gallery

P1 for MVP 2.

Scope:

- Browse recent photos and videos exposed through Android media APIs.
- Preview thumbnails and metadata on desktop.
- Download selected media to desktop.
- Upload selected images/videos to Android media storage where Android permits it.
- Delete or manage media only with explicit confirmation and clear failure behavior.

Limits:

- No background photo backup.
- No cloud photo-provider integration unless the provider exposes media through normal Android APIs.
- No hidden or app-private media access.

### Remote File Browser

P1 for MVP 2.

Scope:

- Browse Android storage locations exposed through Android storage APIs, SAF, MediaStore, or
  app-granted document tree access.
- Download, upload, rename, delete, and create folders where permissions allow it.
- Show permission boundaries clearly when a path cannot be accessed.
- Reuse the file-transfer protocol for large file payloads.

Limits:

- No root filesystem browsing.
- No private app data access outside public Android APIs.
- No continuous folder sync in MVP 2.

### Internet Relay And NAT Traversal

P2 for MVP 2.

Scope:

- Keep LAN/manual connection as the default path.
- Add opt-in remote connectivity for users who cannot reach the phone and desktop on the same LAN.
- Prefer direct peer-to-peer traversal before relay.
- Relay traffic must remain authenticated and encrypted end to end.
- Remote access must be disabled by default and visible when active.

Limits:

- No unauthenticated relay discovery.
- No cloud account requirement for local LAN use.
- Relay availability must not become a dependency for MVP 1 mirroring or local MVP 2 utilities.

### Multiple Simultaneous Desktop Clients

P2 for MVP 2.

Scope:

- Pair and remember multiple trusted desktops.
- Allow more than one authenticated desktop client to connect at the same time.
- Define roles for viewer, controller, file-transfer peer, and notification/media utility client.
- Give exactly one client active input-control ownership at a time unless an explicit collaboration
  mode is added.
- Show connected clients on Android and allow revocation per desktop.

Limits:

- No shared account graph is required.
- No silent background client access.
- Multi-client behavior must not make input arbitration ambiguous.

## Optional Stretch

These can enter MVP 2 only if the required lanes are stable and permission UX remains simple:

- Find/ring phone.
- A small transfer history for the current session.
- Camera-as-webcam exploration behind a separate privacy review.
- Instant hotspot exploration if a public Android or OS integration path exists.

## MVP 2 Included Feature Commitments

These features are part of MVP 2 scope:

| Capability | MVP 2 decision | Reason |
| --- | --- | --- |
| Audio forwarding | Include in MVP 2 | Independent audio stream with clear latency and protected-content limits. |
| App-specific Android windows | Include in MVP 2 | App-focused windows are an advanced MVP 2 surface with documented device/API limits. |
| SMS/MMS/RCS | Include in MVP 2 | Messaging is permission-heavy, so it gets separate review and toggles inside MVP 2. |
| Calls | Include in MVP 2 | Call state/control is part of the advanced phone-integration lane. |
| Photos/gallery | Include in MVP 2 | Recent media browsing and explicit media transfer are MVP 2 surfaces. |
| Remote file browser | Include in MVP 2 | Browser follows one-shot file transfer and uses Android storage permission boundaries. |
| Image clipboard | Include in MVP 2 | Image clipboard is supported where Android exposes content safely. |
| Internet relay/NAT traversal | Include in MVP 2 | Remote connectivity is opt-in and must preserve end-to-end authentication/encryption. |
| Multiple simultaneous desktop clients | Include in MVP 2 | Multi-client support uses explicit roles, revocation, and input ownership. |

## Security And Privacy Requirements

Every MVP 2 feature must be independently disableable. Pairing must establish transport trust, but
feature permissions must still be feature-scoped:

- Notification access is separate from file transfer.
- Clipboard sync is separate from notification access.
- File transfer must require an explicit send/share action.
- SMS, calls, photos, relay, app windows, and file browsing each require separate user-visible
  enablement.
- Sensitive notification content must be hideable without disabling all notifications.
- Unknown or unauthenticated clients must not receive utility data.
- Multi-client sessions must expose connected clients and revocation on Android.
- Relay sessions must be visibly remote, authenticated, and encrypted end to end.
- Utility traffic should use separate protocol message types and must not be tunneled through ad hoc
  strings.

No MVP 2 feature should require root, Microsoft account services, OEM-only Phone Link integrations,
or copied KDE Connect implementation code. Features that can only work on selected devices or Android
versions must state that support boundary explicitly.

## Protocol Direction

Add explicit protocol messages for utility features instead of overloading video or input messages:

- `DeviceStatus`
- `MediaStatus`
- `MediaControl`
- `ClipboardText`
- `ClipboardImage`
- `FileTransferStart`
- `FileTransferChunk`
- `FileTransferComplete`
- `FileBrowseRequest`
- `FileBrowseResponse`
- `FileMutation`
- `NotificationPosted`
- `NotificationRemoved`
- `NotificationAction`
- `AudioFormat`
- `AudioFrame`
- `AudioControl`
- `AppWindowOpen`
- `AppWindowClose`
- `AppWindowInput`
- `MessageThreadList`
- `MessageEvent`
- `MessageSendRequest`
- `CallState`
- `CallAction`
- `PhotoAssetList`
- `PhotoAssetTransfer`
- `RelayOffer`
- `RelayStatus`
- `ClientList`
- `ClientRoleUpdate`

Large payloads, especially file transfer, should use a stream separate from low-latency input and
control traffic once the transport supports it.

## MVP 2 Definition Of Done

MVP 2 is done when all of the following are true:

- A paired desktop can show Android battery/device status.
- A paired desktop can control active Android media playback where Android exposes a controllable
  session.
- Audio forwarding works for at least one documented supported Android audio path and can be
  disabled independently.
- Text and image clipboard sync work within documented Android version limits and can be disabled.
- A user can send a file from desktop to Android and share a file from Android to desktop.
- A user can browse permitted Android files remotely and transfer selected files both directions.
- A user can browse recent Android photos/gallery media and transfer selected items.
- A user can mirror Android notifications, hide sensitive content, and disable notification mirroring
  per app.
- A user can view and send SMS/MMS where Android permissions and roles allow it, with RCS limits
  documented.
- A user can see call state and use supported call actions where Android and desktop transport allow
  it.
- A user can open at least one app-specific Android window on a documented supported path.
- A user can opt into non-LAN connectivity through authenticated relay or NAT traversal without
  changing the LAN-first default.
- Android can manage multiple paired desktops, more than one desktop can connect at once, and input
  control ownership is unambiguous.
- Each feature has clear permission setup, disabled state, and failure state.
- Utility traffic is authenticated and unavailable to unknown clients.
- The docs clearly state which MVP 2 features are broadly Android-compatible and which are limited
  by Android version, device support, carrier behavior, or provider behavior.
