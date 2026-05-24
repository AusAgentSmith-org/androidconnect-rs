# Desktop redesign gap closure plan

**Audit baseline:** androidconnect-rs `1f0dfb3`, FluentGUI `a2e17e2`.

**Why this plan exists:** Passes A-D and the FluentGUI polish work have
closed most of the original critique, but the current app still has a few
real gaps: partially wired interactions, incomplete token/Card discipline,
and framework follow-ups whose docs overstate the implementation.

This plan supersedes the old Pass A-D docs for remaining work. Treat the old
pass docs as historical detail; execute this file top to bottom.

## Completion summary

**Automated close-out completed:** 2026-05-24.

- Desktop interactive gaps were closed: DeviceChip now opens a device popover,
  permission banners route to visible permission steps, Mirror controls are
  wired or hidden, and the reviewed no-op sweep is clean.
- `app.rs` now documents fixed layout dimensions, uses the Card primitive for
  quick action tiles, keeps token-equivalent literals out of the verified
  sweep, and documents remaining illustrative/raw color exceptions.
- FluentGUI backdrop blur comments now describe the actual Blade/WGSL
  single-pass shared-backdrop implementation.
- Card, SectionHeader, and Skeleton have builder-state tests.
- Real GPUI `letter_spacing()` remains deferred as intended in section 3.3;
  `Label::eyebrow()` remains the short-label workaround.
- Automated verification passed. Manual paired-device viewer checks still need
  to be run on a local device or emulator before declaring physical UX parity.

## Assumptions

- Product target remains the current Linux desktop viewer first. macOS and
  Windows visual parity is useful but not blocking unless those platforms
  become active targets.
- Storage protocol work is considered done unless physical-device testing
  proves the Android breakdown inaccurate.
- The goal is to close observable design gaps and remove misleading docs,
  not to perform a full app.rs rewrite.
- Backdrop blur should stay pragmatic for Mirror chips. Full-window acrylic,
  Mica, dual-filter blur, and per-quad blur regions remain framework backlog.

## Milestone 1 - Finish Interactive Wiring

**Goal:** no visible control that looks actionable is a no-op or a misleading
state mutation.

### 1.1 DeviceChip popover

Files:
- `apps/desktop-viewer/src/app.rs`
- `apps/desktop-viewer/src/panels/mod.rs` only if a new panel route is needed

Current issue:
- `topbar-device-chip` is clickable, but it switches to Overview and mutates
  `status.input_authenticated = false` instead of opening a device switcher.

Implementation:
1. Add `device_popover_open: bool` to `AppModel`.
2. Toggle it from `topbar-device-chip`.
3. Render an absolute popover anchored under the chip with:
   - current device name
   - bind/peer address
   - connection badge
   - actions: `Pair new device`, `Refresh status`
4. `Pair new device` should use the existing pair-screen flow without faking
   authentication state. Prefer setting `active_panel = Panel::Overview` and
   adding an explicit `show_pair_screen`/`pairing_requested` flag if needed.
5. Close on action click and when switching panels.

Verification:
- Clicking the chip opens/closes a popover.
- The current device remains paired until the user explicitly starts pairing.
- Pair flow still renders QR/code when requested.

### 1.2 Permission banner action

Files:
- `apps/desktop-viewer/src/app.rs`
- `apps/android/app/src/main/java/dev/androidconnect/OnboardingActivity.java`
  only if a desktop-to-Android command is added later

Current issue:
- `Open onboarding` renders but calls an empty closure.
- The design calls for an accent compact action.

Implementation:
1. Change `permission_banner` action button to
   `ButtonAppearance::Accent`.
2. Add a desktop command payload only if there is already a safe Android-side
   route for opening onboarding. If not, make the button honest:
   `Pair screen help` or `Show permission steps`, and open the Pair/Overview
   instructions instead of pretending it can launch Android settings.
3. Add a short activity/event message when the user clicks it so the action is
   visibly acknowledged.

Verification:
- Permission-required banners show an accent button.
- Clicking the button visibly changes UI state or sends a real command.
- No empty `|_, _, _| {}` remains for permission actions.

### 1.3 Mirror chrome controls

Files:
- `apps/desktop-viewer/src/app.rs`
- `apps/desktop-viewer/src/network.rs` only if refresh needs a command

Current issue:
- Mirror settings/snapshot/refresh icons are hoverable but not clickable.
- Chrome starts visible forever until first mouse movement.

Implementation:
1. Initialize Mirror chrome as hidden after the same idle threshold when there
   has been no movement. If always-visible-on-entry is desired, set
   `last_mirror_mouse_move = Some(Instant::now())` when entering Mirror.
2. Wire `mirror-refresh` to `send_refresh_ping()`.
3. Wire `mirror-settings` to `Panel::Settings`.
4. For `mirror-snapshot`, either implement a real frame save from
   `frame_data` or hide the button until it can be implemented.

Verification:
- Chrome fades out after about 3 seconds of inactivity.
- Moving the mouse fades it back in.
- Every rendered icon does something observable.

### 1.4 No-op sweep

Files:
- `apps/desktop-viewer/src/app.rs`

Known current no-ops:
- `Find my phone` quick action
- `now-play` Overview media button
- any remaining `|_, _, _| {}` handlers

Implementation:
1. Run:
   ```sh
   rg -n "\\|_, _, _\\| \\{\\}|wired separately|TODO|no-op" apps/desktop-viewer/src
   ```
2. For each result, either:
   - wire a real command,
   - route to the panel where the action is implemented, or
   - remove/hide the control.

Verification:
- The grep above has no user-facing no-op handlers.
- Quick actions and Overview media controls are either real or absent.

## Milestone 2 - Finish Visual Consistency

**Goal:** the app should read as a coherent Fluent surface, with exceptions
documented instead of accidental.

### 2.1 Token sweep with allowlist

Files:
- `apps/desktop-viewer/src/app.rs`

Current issue:
- Many token-equivalent dimensions remain as literal `px(...)`, `hsla(...)`,
  and fixed font sizes.

Implementation:
1. Replace token-equivalent spacing/radii values:
   - `2, 4, 8, 12, 16, 24, 32` spacing values -> `spacing.*`
   - token-equivalent radii -> `radii.*`
   - body/caption/subtitle/display font sizes -> `Label` or typography tokens
2. Keep intentional fixed dimensions as named constants near the top of the
   file. Examples: sidebar width, top bar height, QR size, fake phone size,
   file tile width, mirror chip size.
3. Add an `app.rs` comment block named `Fixed Layout Dimensions` listing why
   those constants are not theme tokens.
4. Convert raw `hsla(...)` where possible to theme colors, `tint(...)`, or
   named helper functions. Keep deliberate illustrative hues in the phone
   illustration and file-type map.

Verification:
```sh
rg -n "px\\([0-9]|hsla\\(" apps/desktop-viewer/src/app.rs
```
Every remaining match is either:
- a named fixed-layout constant,
- an illustrative hue helper,
- a one-off renderer requirement with a nearby comment.

### 2.2 Card recipe audit

Files:
- `apps/desktop-viewer/src/app.rs`
- FluentGUI `crates/fluent-primitives/src/card.rs` if API needs a small
  extension

Current issue:
- Some panel surfaces still hand-roll `bg(neutral) + border + radius`
  instead of using `Card`.

Implementation:
1. Convert content surfaces to `Card::new()`:
   - notification cards
   - file action dialog
   - disconnected capability panels where appropriate
   - storage and overview repeated surfaces
2. Keep chrome as raw `div`: top bar, sidebar, status bar, popovers, floating
   Mirror chips.
3. If `Card` needs hover or accent-stripe variants, add explicit methods such
   as `.hoverable()` or `.accent_stripe(...)` instead of duplicating the recipe.

Verification:
```sh
rg -n "\\.bg\\(colors\\.neutral\\)|\\.border_1\\(\\).*stroke_neutral" apps/desktop-viewer/src/app.rs
```
Remaining matches are chrome, chips, or documented exceptions.

### 2.3 Section headers and typography

Files:
- `apps/desktop-viewer/src/app.rs`
- FluentGUI `crates/fluent-primitives/src/section_header.rs`

Implementation:
1. Use `Label::eyebrow()` for section landmarks that match the design:
   `BATTERY`, `STORAGE`, `NOW`, `EARLIER`, etc.
2. Keep `SectionHeader` for card titles with actions, but update it to use
   design-consistent typography if needed.
3. Ensure numbers that can change use `.tabular_nums()`.

Verification:
- Section labels appear uppercase/letter-spaced where expected.
- Battery, volume, storage, QR countdown, and percentages do not jiggle.

## Milestone 3 - Framework Accuracy And Small Tests

**Goal:** FluentGUI should document exactly what it implements and cover the
small primitives added for the redesign.

### 3.1 Fix backdrop blur documentation

Repo:
- `/home/sprooty/Working/Active/apps/FluentGUI`

Files:
- `crates/gpui/src/color.rs`
- `crates/gpui/src/platform/blade/shaders.wgsl`
- `docs/` if blur is described there

Current issue:
- `backdrop_blur` claims dual-filter Gaussian, but the shader is a single-pass
  cross-pattern blur.

Implementation:
1. Update public comments to say single-pass backdrop blur.
2. Note current limitations:
   - WGSL/Blade path only
   - full-surface capture when any blur quad is present
   - all blur quads in a batch share one blurred backdrop snapshot
3. Keep dual-filter/per-quad/Mica in `REDESIGN_PASS_E.md` as backlog.

Verification:
- Docs no longer claim dual-filter or cross-platform support.

### 3.2 Add primitive tests

Repo:
- `/home/sprooty/Working/Active/apps/FluentGUI`

Files:
- `crates/fluent-primitives/src/card.rs`
- `crates/fluent-primitives/src/section_header.rs`
- `crates/fluent-primitives/src/skeleton.rs`

Implementation:
1. Add lightweight tests for builder defaults and configuration state.
2. If render-level testing is impractical, expose small pure helpers or
   `#[cfg(test)]` accessors rather than snapshotting renderer output.
3. Keep existing AppDot/ConnectionBadge tests unchanged.

Verification:
```sh
cargo test -p fluent-primitives
```

### 3.3 Decide GPUI letter spacing scope

Current state:
- `Label::eyebrow()` fakes letter spacing with per-character spans.
- There is no real `Styled::letter_spacing()`.

Decision:
- Do not implement real GPUI letter spacing in this close-out unless a new
  design requires long letter-spaced text.
- Add a note in this file's completion summary if it remains deferred.

Verification:
- `Label::eyebrow()` remains acceptable for short section labels.
- No doc claims real GPUI letter spacing exists.

## Milestone 4 - End-To-End Verification

Run these after Milestones 1-3.

### Rust checks

From `androidconnect-rs`:
```sh
cargo test -p androidconnect-protocol
cargo test -p androidconnect-android-native
```

From `androidconnect-rs/apps/desktop-viewer`:
```sh
cargo fmt --check
cargo check --all-targets
cargo test --all-targets
```

From `FluentGUI`:
```sh
cargo fmt --check
cargo test -p fluent-core -p fluent-primitives -p fluent-app
```

### Android checks

From `androidconnect-rs/apps/android`:
```sh
./gradlew :app:assembleDebug
```

### Manual viewer checks

1. Launch the desktop viewer and pair a device or emulator.
2. Overview:
   - phone illustration renders
   - quick actions are all real or hidden
   - storage card shows skeleton, permission banner, or real breakdown
3. Mirror:
   - chrome fades out/in
   - refresh/settings/snapshot controls are real or hidden
4. Notifications:
   - Now/Earlier groups render
   - hover states render
   - relative timestamps update without resizing the window
5. Messages:
   - Enter sends
   - date separators appear between >30 minute gaps
6. Files:
   - List/Grid toggle works
   - breadcrumb segments navigate
   - single-click selects
   - double-click opens/downloads
   - selection footer updates
7. Pair:
   - QR countdown ticks once per second
   - copy button writes the code to clipboard and shows success feedback
8. Permission-required states:
   - banners show an accent action
   - action is not a no-op

## Milestone 5 - Commit Order

Use small commits so each milestone can be reviewed independently:

1. `desktop-viewer: finish remaining interactive wiring`
2. `desktop-viewer: complete token and Card consistency sweep`
3. `fluentgui: correct blur docs and add primitive tests`
4. `docs: mark redesign gap closure complete`

After each commit:
```sh
git status --short
git log --oneline -n 3
```

Push only after the checks for that milestone pass.

## Done Criteria

This plan is complete when:

- No user-facing control in the reviewed surfaces is a no-op.
- DeviceChip opens a real device popover or a clearly scoped replacement.
- Permission banners have a real accent action.
- Mirror chrome auto-hide and controls behave as designed.
- Token-equivalent literals in `app.rs` are replaced or documented.
- Content cards use the Card recipe unless explicitly exempted.
- FluentGUI backdrop blur docs match the actual renderer.
- Primitive tests cover Card, SectionHeader, and Skeleton basics.
- All verification commands in Milestone 4 pass.
