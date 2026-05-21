# Desktop Input Control

This document describes the current M3 input-control path. It is implemented but still needs
end-to-end validation on emulator and physical Android devices.

## Security Note

Input is gated by an ephemeral pairing-code challenge/response. The desktop viewer prints a pairing
code, Android sends a challenge, and Android ignores input until the desktop proves knowledge of the
same code with HMAC-SHA256.

This is still not the final security model. The TCP stream is not encrypted, paired desktop identity
is not persisted, and this is not hardened against active man-in-the-middle attacks. Do not use this
on an untrusted network.

## Requirements

- Android screen mirroring must already be connected to the desktop viewer.
- Android must be connected with the pairing code printed by the desktop viewer.
- Android accessibility service must be enabled from Android Settings.
- The desktop viewer window must have keyboard focus for text and shortcuts.
- The desktop window title should show `input paired` before input validation begins.

## Control Reference

| Desktop input | Android action |
| --- | --- |
| Left click | Tap at the mapped Android display coordinate |
| Left-button drag | Accessibility drag gesture from drag start to drag end |
| Mouse wheel / touchpad scroll | Accessibility scroll action on a scrollable node in the active window |
| Typed text | Appends text to the focused editable field with `ACTION_SET_TEXT` |
| Backspace / Delete | Removes one code point from the focused editable field |
| Enter / Tab | Sends newline / tab text to the focused editable field |
| `Esc`, Back key, or Browser Back key | Android Back global action |
| Home key | Android Home global action |
| App Switch key | Android Recents global action |
| Power / Standby key | Android Lock Screen global action |
| `Ctrl+Alt+B` | Android Back global action |
| `Ctrl+Alt+H` | Android Home global action |
| `Ctrl+Alt+R` | Android Recents global action |
| `Ctrl+Alt+L` | Android Lock Screen global action |

## Coordinate Mapping

The desktop viewer letterbox-scales the Android frame into the window. Pointer coordinates are
mapped from desktop-window pixels back into Android-frame pixels by removing the letterbox offset
and applying the inverse scale.

Pointer input outside the visible Android frame is ignored. If a drag starts inside the frame and
then leaves it, move/up points are clamped to the frame edge so Android still receives a completed
gesture.

## Implementation Path

```text
winit window events
  -> desktop InputEvent channel
  -> pairing-authenticated desktop TCP writer
  -> Android native input reader thread
  -> pairing gate
  -> JNI static calls
  -> RemoteControlAccessibilityService
  -> Android accessibility gestures/globals/text actions
```

Key files:

- `apps/desktop-viewer/src/main.rs`: captures window input and maps pointer coordinates.
- `apps/desktop-viewer/src/network.rs`: writes `InputEvent` envelopes back to Android.
- `crates/android-native/src/lib.rs`: reads inbound `InputEvent` envelopes and dispatches through JNI.
- `apps/android/app/src/main/java/dev/androidconnect/RemoteControlAccessibilityService.java`:
  performs accessibility gestures, scroll, text edits, and global actions.

## Current Limitations

- Text entry uses accessibility `ACTION_SET_TEXT`, so it replaces the full focused node text after
  appending locally. It does not preserve cursor position, selection ranges, IME composition, or
  app-specific editor behavior.
- Wheel scrolling depends on accessibility exposing a scrollable node in the active window.
- Accessibility gestures can fail in secure screens, system permission prompts, and some custom UI.
- Right/middle/back/forward mouse buttons are serialized but ignored by Android dispatch for now.
- Pairing is ephemeral. Trusted desktop identity, encrypted transport, and per-session keys are not
  implemented yet.
- Only one desktop client is expected.

## Validation Checklist

1. Start the desktop viewer on the LAN bind address.
2. Build and install Android with the native library packaged.
3. Enable the Android accessibility service.
4. Connect Android to the desktop host with the desktop pairing code and start mirroring.
5. Confirm the desktop logs `pairing authenticated; desktop input enabled` and the window title
   shows `input paired`.
6. Confirm the desktop renders the live Android frame.
7. Left-click a visible Android button and confirm the tap lands correctly.
8. Drag across a scrollable view and confirm it swipes.
9. Use the wheel or touchpad over a scrollable view and confirm it scrolls.
10. Focus an editable text field and type letters, Backspace, Delete, Enter, and Tab.
11. Trigger Back, Home, Recents, and Lock with hardware keys or `Ctrl+Alt+B/H/R/L`.
12. Disconnect Android and confirm input stops.
13. Reconnect and repeat at least click and text checks.

## Troubleshooting

- If video works but input does not, confirm Android accessibility service status in the app and
  Android Settings, then confirm the desktop logged pairing success and the window title shows
  `input paired`.
- If clicks land in the wrong place, verify the desktop window is showing the current frame size and
  test without rotating the device during the session.
- If text does not enter, confirm the focused Android node is editable. Some fields block
  accessibility text replacement.
- If shortcuts do nothing, click the desktop viewer window first so it has keyboard focus.
