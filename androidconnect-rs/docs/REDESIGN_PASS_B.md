# Pass B — Biggest visible wins

**Why this pass exists:** the Overview is the page that opens on launch and
the Mirror panel is where users spend most of their time. Both have marquee
visual elements specified in the design that were skipped during the initial
redesign. This pass shrinks the "still looks generic" gap.

**Estimated effort:** 1 day.

**Driver files:** `apps/desktop-viewer/src/app.rs`, possibly a new
`apps/desktop-viewer/src/widgets/phone_illustration.rs`.

---

## Items

### B1 — Phone illustration in the Overview device hero
The biggest single missing visual.

Add a new local widget (could live inline in `app.rs` or its own module under
`src/widgets/`). Anatomy per `/tmp/design-desktop/design-docs/04-overview.md`:

- Outer: `96 × 180`px column, rounded 16, dark-cool gradient
  `oklch(0.32 0.06 250) → oklch(0.18 0.04 250)`, 1px subtle border, padding 8.
- Inner screen: rounded 8, brighter cool gradient. Contains:
  - Faux status bar: tiny "9:47" + a battery glyph
  - Two stacked placeholder header bars (60% width then 40% width, height 6)
  - 3×2 grid of coloured tile squares — hues `200, 50, 130, 280, 25, 350`
    via `fluent_core::gradient_from_hue`

This is **pure decoration** — no data binding. Bring back the
`oklch → HSL approximation` pattern from `fluent-core::colors`. Render as a
flex_none column inserted as the first child of the device summary card row
in `device_summary_card()`.

### B2 — Mirror disconnected capability card
`render_mirror_panel` currently returns just `disconnected_placeholder(...)`
when not connected. Below the placeholder, add a `max_w(px(640))` capability
card (use the `Card` primitive). Inside, a 2×2 grid with `gap = spacing.lg`:

| Icon | Title | Caption |
|---|---|---|
| `mirror` | Mirror & control | Stream the screen at up to 60 fps |
| `bell` | See notifications | Reply without unlocking your phone |
| `chat` | Send messages | SMS / RCS from your keyboard |
| `folder` | Browse files | Drag and drop both ways |

Each cell: 28×28 rounded-6 `surface_dim` tile with subtle border + 16px
`on_neutral_accent` icon, then `body 600` title and `caption` description.

### B3 — Mirror floating chrome auto-hide
The chrome (control rail + info pill) currently renders permanently. Per
the design they should fade out after 3s of mouse inactivity over the mirror
surface and fade in on movement.

Implementation:
- Add `last_mirror_mouse_move: Option<Instant>` to `AppModel`.
- In `render_mirror_panel`'s `on_mouse_move` listener (already there),
  update the timestamp + `cx.notify()`.
- Spawn a once-per-second tick that calls `cx.notify()` if there's stale
  activity (so the fade triggers without the user moving).
- Compute `opacity = if last_move.elapsed() < 3s { 1.0 } else { fade_to_0 }`.
- Wrap each chip in `div().opacity(opacity)`.
- Use GPUI's animated opacity for the actual transition (200ms ease).

If GPUI doesn't expose `opacity()` on Styled directly, set the chip's
background alpha and text colour alpha — they accept `Hsla { a, .. }`.

### B4 — Skeleton loaders adopted in panels
The library has `Skeleton`, `SkeletonRow`, and `SkeletonWrap`. Currently
nothing uses them. Wire them in:

- **Notifications panel** first-paint: when connected but
  `notifications.is_empty()` AND the panel just opened (track an
  `initial_load: bool` in `NotificationsState`), show 5 `SkeletonRow`s.
- **Messages panel** thread list: while `state.threads.is_empty()` and
  `state.thread_status` is `None`, show 6 `SkeletonRow`s.
- **Files panel** body: while `pending` AND `response.is_none()`, show 8
  list-row skeletons (each: 16px icon skeleton + line skeleton).
- **Recent notifications / Recent messages cards** on Overview: same pattern,
  4 rows each.

---

## Verification

1. `cargo fmt && cargo clippy -- -D warnings && cargo test`.
2. Launch the viewer. Overview should show the phone illustration on the
   left of the device hero card with its faux screen and tile grid.
3. Disconnect a paired device. Mirror panel shows the disconnected
   placeholder *and* the 2×2 capability card below it.
4. With a device paired and mirror streaming: move the mouse over the
   mirror surface → control rail and info pill visible. Stop moving for
   ~3s → they fade out. Move again → they fade back in.
5. Open Notifications immediately after pair (before the phone has streamed
   data) → 5 shimmering skeleton rows. They disappear when real
   notifications arrive.

When all four items work end-to-end, commit & push.
