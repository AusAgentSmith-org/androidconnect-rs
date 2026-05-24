# Pass A — Fundamentals (overdue cleanup)

**Why this pass exists:** the B/C/D library uplift work shipped, but the
app-side polish that *should* have come with it was skipped. This pass closes
the cheapest gaps that signal "this app was thought through" — token
discipline, Card uniformity, hover states, animated chrome, working buttons.

**Estimated effort:** 1 day.

**Driver file:** `apps/desktop-viewer/src/app.rs` (~3000 lines, all changes
local to this file unless noted).

---

## Items

### A1 — Token sweep
Replace literal `px(...)` and theme-token-equivalent constants with the
fluent-core tokens:

| Literal | Token |
|---|---|
| `px(2.0)` | `px(spacing.xs)` |
| `px(4.0)` | `px(spacing.sm)` |
| `px(8.0)` | `px(spacing.md)` |
| `px(12.0)` | `px(spacing.lg)` |
| `px(16.0)` | `px(spacing.xl)` |
| `px(24.0)` | `px(spacing.xxl)` |
| `px(32.0)` | `px(2.0 * spacing.xl)` |
| `px(4.0)` for `rounded(...)` | `px(radii.md)` |
| `px(8.0)` for `rounded(...)` | `px(radii.lg)` |
| `px(2.0)` for `rounded(...)` | `px(radii.sm)` |
| `px(9999.0)` for `rounded(...)` | `px(radii.pill)` |

Each `render_*_panel` already calls `cx.theme()`, so destructure spacing /
radii at the top and use throughout. A grep + visual scan; ~250 substitutions.

**Don't** convert literals that are *not* token-equivalent (`px(36.0)` for the
sidebar item height, `px(208.0)` for the QR square, `px(48.0)` for the
TopBar — those are intentional pixel values, not tokens).

### A2 — Card recipe uniformity
Replace hand-rolled `div().bg(colors.neutral).border_1().border_color(stroke_neutral_subtle).rounded(px(4.0))`
patterns with `Card::new().padding(...).child(...)`. Targets:

- Notification cards in `render_notifications_panel`
- File action dialog in `render_files_panel`
- Permission banner (`permission_banner` helper) — but keep the
  `border_l_2` accent stripe, just put it on a Card
- Storage card outer (when A's #13 / Pass D lands)

Sidebar items, status bar, top bar are intentionally *not* Cards (they're
chrome). Leave those.

### A3 — Wire the no-op TopBar buttons
In `render_top_bar`:
- Refresh button: send a `Payload::SyncRequest` (define if it doesn't exist
  yet — or trigger a re-issue of the most recent feature query). For now, at
  minimum call `self.send_utility(Payload::Heartbeat(...))` so the user sees
  *something* happen.
- Settings button: switch `active_panel` to a new `Panel::Settings` variant
  with a stub "Settings (coming soon)" screen. Add to `Panel::ALL` + label
  + icon mapping in `panels/mod.rs`.

### A4 — Hover states on interactive cards
Notification cards and recent-activity rows need `colors.neutral_hover` on
hover. Wrap each card root in `div().id(...).hover(|s| s.bg(neutral_hover))`
where `id` is unique per card (use the notification_id / thread_id).

### A5 — Animated heartbeat dot
Status bar currently shows `♥ #{nonce}` text. Replace with a 6×6 pulsing
`status_success`-coloured dot. Pattern is identical to
`ConnectionBadge`'s pulsing dot in fluent-primitives — copy the
`with_animation(Animation::new(Duration::from_millis(1400)).repeat().with_easing(pulsating_between(0.35, 1.0)))`
incantation, set `colour.a = delta`.

### A6 — Permission banner action button
`permission_banner` in `app.rs` renders heading + body. Add a third optional
parameter `action: Option<(SharedString, Box<dyn Fn(...)>)>` and render as a
`Button::Subtle` compact on the right. Call sites can pass
`Some(("Open onboarding".into(), Box::new(|_, _, _| { /* TODO: open URL */ })))`
or `None`.

### A7 — Relative timestamp refresh
Currently `relative_timestamp(ms)` is called at render time. To keep "2m ago"
fresh, schedule a 30s tick that calls `cx.notify()` on `AppModel`:

```rust
// In AppModel::new
cx.spawn(async move |this, cx| loop {
    cx.background_executor().timer(Duration::from_secs(30)).await;
    if this.update(cx, |_, cx| cx.notify()).is_err() {
        break;
    }
}).detach();
```

---

## Verification

1. `cargo fmt && cargo clippy -- -D warnings && cargo test` — clean.
2. Launch the viewer. Mouse over a notification card → background shifts to
   `neutral_hover`. Heartbeat dot in the status bar visibly pulses. Click
   the Refresh / Settings buttons in the top-bar — both do *something* now.
3. Trigger a permission-missing state (revoke an Android permission on the
   paired device) → the inline banner shows an action button.
4. Wait 60 seconds on the Overview screen — the "Connected · just now"
   timestamp shifts to "1m ago" / "2m ago" without a window resize.
5. Skim `app.rs` for `px(12.0)`, `px(16.0)`, `px(4.0)` — these should be
   token references except in the explicit-pixel exceptions noted above.

When all five items above are observable, commit & push.
