# Pass C — Structurally meaningful

**Why this pass exists:** beyond the marquee visuals, several panels are
functionally shallow compared to the design — Files has no Grid view or
breadcrumb, Notifications doesn't group by recency, Messages has no date
separators. This pass fixes the panels that are closest to "list with
buttons" rather than designed surfaces.

**Estimated effort:** 2 days.

**Driver files:** `apps/desktop-viewer/src/app.rs`,
`apps/desktop-viewer/src/panels/{notifications.rs, files.rs}`.

---

## Items

### C1 — Files Grid view + List/Grid toggle
Currently only List view exists. Per `08-files.md`:

- Toolbar: add a segmented control (List / Grid) using `ToggleButton` from
  fluent-primitives. Store the choice in `FilesState` as a new
  `pub view_mode: FilesViewMode` (enum List / Grid, default List).
- Grid layout: `grid-template-columns: repeat(auto-fill, minmax(140px, 1fr))`,
  gap `spacing.lg`, padding `spacing.xl`. GPUI doesn't have CSS grid — fake
  it with `flex_row().flex_wrap()` and child `flex_basis(140.0)`.
- Each tile: 1px subtle border, neutral bg, 4px radius, padding 12,
  containing:
  - Aspect 1/1 thumbnail panel (use `flex_none().w(px(116.0)).h(px(116.0))`)
  - For images: deterministic gradient based on filename hash
  - For other kinds: `surface_dim` bg with centred 36px filetype icon
  - 12px filename below (single-line ellipsised)
  - 10px caption: `{count} items` for dirs, otherwise size string
- Selection: track `selected_path: Option<String>` in `FilesState`. Clicking
  a tile selects (border becomes `accent`, bg becomes
  `tint(accent, 0.12)`). Double-click enters dir / saves file.

### C2 — Files breadcrumb
Toolbar currently shows raw path text. Replace with clickable segments:

- Always start with literal "Phone" (clickable → navigate to `/`).
- Split `current_path` on `/`, render each segment as a body-size clickable
  text that calls `files_state.navigate_to(prefix_up_to_here)`.
- Between segments: 12px `chevron` icon in `on_subtle_disabled`.
- All segment text in `on_neutral`; chevron is the only dim element.

### C3 — Files selection footer
Visible only when `selected_path.is_some()`. Border-top 1px subtle,
`surface_dim` bg, padding `8px 20px`, caption row showing:

`{filetype icon} {name} · {size or N items} · {modified}`

Name in `on_neutral 600`, the rest in `on_subtle`.

### C4 — Double-click in Files
Add a `last_click_path + last_click_time` to `FilesState`. In each entry's
`on_click`, if same path within 350ms:
- If dir: enter (current behaviour)
- If file: trigger Save to PC (`request_download(path)`)
Otherwise: select only (no navigate). Single-click selects; double-click acts.

### C5 — Notification grouping (Now / Earlier)
Bucket items in `render_notifications_panel`:
- "Now" group: timestamp within last 30 min (1800s).
- "Earlier" group: older.
Render an `Label::eyebrow("Now")` above the first group, an
`Label::eyebrow("Earlier")` above the second. Skip empty groups.

### C6 — Date separators in Messages
In `render_messages_panel`'s history loop, track `last_ts: Option<u64>`. If
`abs(current.ts - last) > 1800000`, render a centred row before the bubble:

```rust
div().flex().items_center().justify_center().py(px(spacing.md))
    .text_color(colors.on_subtle_disabled)
    .text_size(px(typography.caption.size))
    .child(format_full_time(current.ts))
```

`format_full_time` formats as "Today 14:32" / "Yesterday 09:15" / "Tue 11 Mar".
Add this helper near `relative_timestamp`.

### C7 — Enter-to-send in SMS composer
The `sms_composer` is a `fluent_primitives::TextInput` entity. Check if
TextInput exposes a `.on_submit()` or `.on_action(KeyDownEvent)` helper. If
yes — wire it to call the same closure as the Send button. If not, the
fluent-primitives TextInput needs an `on_submit` callback added first.

### C8 — Notification AppDot brand colour map
Currently AppDot derives hue from `hue_for(seed)`. The design implies actual
brand colours. Add a small map in `app.rs`:

```rust
fn brand_hue(package: &str, fallback_name: &str) -> f32 {
    match package {
        "com.whatsapp" => 145.0,
        "com.google.android.apps.messaging" => 210.0,
        "com.spotify.music" => 140.0,
        "com.discord" => 235.0,
        "com.instagram.android" => 320.0,
        "com.slack" => 30.0,
        // ... extend as needed
        _ => hue_for(fallback_name),
    }
}
```

Pass it to `AppDot::new(name).hue(brand_hue(&notif.app_package, &notif.app_name))`.

### C9 — Filetype colour map in Files
Currently all non-folder files use `on_subtle` for the icon. Per design:

| Kind | Icon | Colour (oklch → HSL approx) |
|---|---|---|
| dir | folder | `on_neutral_accent` |
| img | image | `hsla(80/360, 0.6, 0.55, 1.0)` |
| vid | video | `hsla(350/360, 0.6, 0.55, 1.0)` |
| doc | doc | `hsla(200/360, 0.45, 0.55, 1.0)` |
| zip | zip | `hsla(110/360, 0.45, 0.55, 1.0)` |

Add a `filetype_colour(entry: &FileEntry, colors: &ColorScheme) -> Hsla`
helper next to where the icon is chosen. Map by extension for files
(`.jpg/.png/.webp → img`, `.mp4/.mov → vid`, `.pdf/.docx → doc`,
`.zip/.tar.gz → zip`, else generic).

### C10 — Pair countdown + copy-to-clipboard
Per `10-pair.md`:

- **Countdown.** After QR is generated, store `qr_expires_at_ms` on AppModel.
  Below the QR card, render `Label::caption(format!("Code expires in {mm}:{ss}"))`.
  Reuse the 30s tick from Pass A item A7 so the timer ticks down (with a
  faster tick — every 1s — when Pair is the active panel).
- **Copy button.** Next to the code chip, a 28×28 Subtle `IconButton`
  showing the `copy` icon. On click: write `self.status.pairing_code` to
  the system clipboard (use the existing `arboard` dependency — see
  `clipboard.rs`). Flip icon to `check` for 1.5s on success.

### C11 — DeviceChip popover (top bar)
Currently `DeviceChip` in `render_top_bar` is non-interactive. Wrap in a
`fluent_layout::popover::Popover` (or `Flyout`). Click opens a panel
showing:
- Current device: name + connection status badge.
- List of remembered devices (read from `trust.rs`).
- "Pair a new device" item that toggles to Pair panel.

If popover wiring is heavy, ship just the click → switch to Pair panel as a
minimal v1.

---

## Verification

1. `cargo fmt && cargo clippy -- -D warnings && cargo test`.
2. Files panel: toggle to Grid view → tiles laid out, click selects, double
   click enters/saves. Breadcrumb at top is clickable segment chain.
   Selecting a non-dir reveals the selection footer.
3. Notifications panel with at least 6 notifications spanning > 30 min →
   "NOW" / "EARLIER" eyebrows visible, items bucketed correctly.
4. Messages panel: open a thread with messages spread over multiple days →
   date separators between gaps.
5. Type a message and press Enter → sends. (Until C7's wire-up the Send
   button is the only path.)
6. Open a notification from WhatsApp → AppDot is recognisable green.
   Browse a photo folder → image files have warm-yellow icons, videos pink.
7. Pair panel: code chip shows "Code expires in 4:38" counting down each
   second. Copy button writes the code to clipboard and flashes a tick.
8. Click DeviceChip in top bar → either popover opens or Pair panel
   activates (depending on v1 scope).

When all eleven items work end-to-end, commit & push.
