# Feature Positioning

Last reviewed: 2026-05-22.

This document fixes the product target for AndroidConnect RS against Microsoft Phone Link and KDE
Connect. The goal is not immediate feature parity with either product. The goal is a focused,
permissively licensed Android screen mirroring and desktop-control stack that can later grow selected
device-integration features without inheriting KDE Connect's GPL boundary or Phone Link's proprietary
platform lock-in.

## Current Status

AndroidConnect RS is in the MVP 1 / M3 slice:

- Screen capture is implemented with Android `MediaProjection`.
- H.264 encoding is implemented with Android `MediaCodec`.
- Android-to-desktop transport is implemented over TCP with `DeviceHello`, `VideoFormat`, and
  `VideoFrame` envelopes.
- Desktop decode/render is implemented with OpenH264, `winit`, and `pixels`.
- Desktop-to-Android input transport is implemented over the current TCP connection.
- Android input dispatch is implemented through an accessibility service for tap, drag, scroll,
  text replacement, and global actions.
- Ephemeral pairing-code authentication gates desktop input.
- `cargo test` passes for the Rust workspace.

The remaining MVP blockers are product-critical:

- Validate input end-to-end on emulator and physical Android devices.
- Persist paired desktop identity, derive per-session keys, and move to encrypted/authenticated
  transport beyond the current input gate.
- Add reconnect/session recovery and clearer status/error reporting.
- Add rotation/resolution renegotiation.
- Replace accessibility `ACTION_SET_TEXT` with an IME-backed text path for reliable editing.

Physical-device validation is not complete until a real phone test pass is recorded. The emulator
pass that validated M1 and M2 does not substitute for physical-device validation of the full input
path.

## Product Thesis

AndroidConnect should be:

- **Phone Link-like where Phone Link is strongest:** live Android screen on desktop with mouse and
  keyboard control.
- **KDE Connect-like where KDE Connect is strongest later:** optional LAN utilities such as file
  transfer, clipboard, notifications, battery status, media controls, and find-phone.
- **Different from both:** open, Rust-first, local-first, non-root Android mirroring/control that is
  not limited to Windows, Microsoft accounts, or selected OEM integrations.

## Target Slices

| Slice | Goal | Features | Release bar |
| --- | --- | --- | --- |
| MVP 1 | Prove local mirroring and input | LAN connection, screen capture, H.264 stream, desktop render, click/drag/scroll/text/navigation input, pairing/auth, reconnect | A non-rooted Android device can be mirrored and controlled safely on a trusted LAN |
| MVP 1 polish | Make the core usable repeatedly | IME text input, rotation handling, connection settings UI, metrics, logs, QA scripts | Real-device QA pass; clear failures; no unauthenticated input |
| Utility pack | Add KDE Connect-style basics | Clipboard, file send/share, notification mirror, battery, media controls, find phone | Each feature has explicit Android permission UX and can be disabled independently |
| Advanced integration | Compete with deeper Phone Link surfaces | Audio forwarding, file browser, photos view, SMS, calls, app-specific windows, camera/webcam, hotspot | Only after core mirroring is solid; each item requires separate privacy/security review |

## Feature Matrix

| Capability | AndroidConnect target | Current status | Phone Link | KDE Connect | Decision |
| --- | --- | --- | --- | --- | --- |
| LAN/manual connection | Required | Manual host/port implemented | Supported, plus Microsoft account pairing and device cloud plumbing | Supported with LAN discovery and manual IP | Keep local-first; add discovery after auth |
| Pairing/authentication | Required | Partial: ephemeral pairing-code input gate | Required through Microsoft account/QR/manual code | Required pair request/accept | Persist identity/session keys before public testing |
| Android screen mirroring | Core differentiator | Implemented, emulator validated | Supported as "phone screen" on supported Android devices/markets | Android "Virtual Display" listed as missing | P0/P1; make this broadly Android-compatible |
| Desktop mouse/touch control of Android | Core differentiator | Implemented, pending validation | Supported for phone screen/apps with mouse, trackpad, keyboard, pen, touch | Android mouse receiver exists, but no Android virtual display pairing | P0; accessibility first, optional advanced backends later |
| Desktop keyboard text into Android | Required | Basic accessibility text replacement | Supported in phone screen/apps | Limited/feature-specific | P0/P1; move to IME path |
| Rotation/resolution changes | Required | Partial format tolerance | Supported in Phone Link flows | Not a screen-mirroring target | P1 |
| Audio forwarding | Later | Not implemented | Supported for some app/phone-screen scenarios on supported devices | Media control exists; audio forwarding is not the core model | P2/P3 after video/input latency work |
| App-specific Android windows | Later/optional | Not implemented | Supported, with multiple-app support on selected devices | Not a core feature | P3, not MVP |
| Notifications | Later utility | Not implemented | Supported with actions/dismiss sync | Supported, including replies where notifications allow | Utility pack |
| SMS/MMS/RCS | Later optional | Not implemented | SMS/MMS supported; RCS limited to select devices | SMS supported through phone bridge | Advanced integration; permission-heavy |
| Calls | Later optional | Not implemented | Supported via Bluetooth | Incoming-call notifications and media pause; not full dialer parity | Advanced integration |
| Photos/gallery | Later optional | Not implemented | Recent photo view/manage supported | File/share plugins can move files | Utility or advanced, not core |
| File transfer/share | Later utility | Not implemented | Supported through share sheet, drag/drop, and File Explorer surfaces | Supported through Share and Receive and remote file browser | Utility pack |
| Remote file browser | Later utility | Not implemented | Supported in Windows File Explorer on Windows 11 | Supported via SFTP/Dolphin integration | Utility pack after simple file transfer |
| Clipboard sync | Later utility | Not implemented | Supported on selected OEM devices for text/images | Supported for text, with Android 10+ limitations | Utility pack; document Android clipboard limits |
| Media controls | Later utility | Not implemented | Supported for now-playing controls | Supported | Utility pack |
| Battery/device status | Later utility | Not implemented | Supported in Phone Link status | Supported | Utility pack |
| Find/ring phone | Later utility | Not implemented | Not a headline feature | Supported | Utility pack |
| Remote commands/presentation/phone-as-PC-input | Not this stack's core | Not implemented | Not Phone Link's main posture | Supported | Defer unless users ask; avoid scope creep |
| Phone camera as webcam | Optional | Not implemented | Supported on Windows 11 with linked Android camera | Not a core KDE Connect feature | P3 only |
| Instant hotspot | Out of scope for now | Not implemented | Supported on selected OEM/device versions | Not a core KDE Connect feature | Do not target until the app has OS integration hooks |
| Internet relay/NAT traversal | Out of scope for MVP | Not implemented | Phone Link can use Microsoft account/cloud/mobile-data flows | VPN/manual network setups are documented | Local-first; consider relay only after LAN product is solid |
| Multi-client/device accounts | Out of scope for MVP | Not implemented | Microsoft account/device graph | Multi-device pairing | Defer |

## Scope Decisions

MVP 1 must remain narrow: mirroring, input, authentication, recovery, and validation. File transfer,
notifications, SMS, calls, clipboard, media controls, and photos should stay out of MVP 1 even though
they are expected by Phone Link or KDE Connect users.

The first public value proposition should be:

> Android screen mirroring and desktop control for non-rooted Android devices, built as an open,
> local-first Rust stack.

After MVP 1, the next best expansion is the utility pack. These features are easier to explain,
mostly local-first, and align with KDE Connect user expectations without undermining the core
mirroring roadmap.

Avoid trying to clone Phone Link wholesale. Phone Link is deeply tied to Windows UX, Microsoft
accounts, selected OEM packages, device-specific permissions, and proprietary integration surfaces.

## Licence Targets

The current Rust workspace declares:

```toml
license = "MIT OR Apache-2.0"
```

Target licence posture:

- Keep AndroidConnect RS code under `MIT OR Apache-2.0`.
- Add top-level `LICENSE-MIT` and `LICENSE-APACHE` files before any public release.
- Treat bundled docs as project material under the same repository licence unless a separate docs
  licence is added.
- Prefer permissive dependencies: MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-style licences.
- Allow MPL-2.0 only with file-level obligations understood.
- Allow LGPL only when dynamic-linking and redistribution obligations are explicitly reviewed.
- Do not add GPL/AGPL dependencies to the main apps unless the project deliberately changes licence.
- Do not copy KDE Connect code into this tree. KDE Connect can be used as reference behavior only
  unless the receiving component is intentionally GPL-compatible.
- Do not depend on Microsoft Phone Link or Link to Windows proprietary components or restricted
  interop APIs for core product behavior.

Codec note:

- The current desktop decoder uses the `openh264` Rust crate with bundled OpenH264 source. The crate
  and bundled upstream library are BSD-2-Clause.
- H.264 distribution can still raise patent/licensing questions depending on how binaries are
  shipped. Track this separately from source licence compliance.
- Before public binary distribution, choose and document the codec distribution strategy: bundled
  OpenH264 source build, system decoder, Cisco-provided binary, or an alternate codec path.

## Source Notes

- Microsoft Phone Link requirements and feature summary: <https://support.microsoft.com/en-us/topic/phone-link-requirements-and-setup-cd2a1ee7-75a7-66a6-9d4e-bf22e735f9e3>
- Microsoft Phone Link phone screen: <https://support.microsoft.com/en-us/topic/setting-up-and-using-phone-screen-in-the-phone-link-ea853ebc-0856-2f6e-af34-703fe449416b>
- Microsoft Phone Link transfer, clipboard, and drag/drop: <https://support.microsoft.com/en-us/topic/seamlessly-transfer-content-between-your-devices-8a0ead3c-2f15-1338-66ca-70cf4ae81fcb>
- KDE Connect user documentation: <https://userbase.kde.org/KDEConnect>
- KDE Connect application page and licence: <https://apps.kde.org/kdeconnect/>
- Local OpenH264 licence reference: `~/.cargo/registry/src/*/openh264-0.9.3/Cargo.toml` and
  `~/.cargo/registry/src/*/openh264-sys2-0.9.6/upstream/LICENSE`
