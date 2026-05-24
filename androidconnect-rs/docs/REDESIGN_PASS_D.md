# Pass D — Protocol-touching (Storage)

**Why this pass exists:** Overview's 5th row in the design is a Storage
breakdown card with a segmented bar (Photos · Apps · Music · System · Free)
and a legend. Protocol doesn't carry this data today, so it needs a new
payload + Android-side wire-up before the UI can render it for real.

**Estimated effort:** 1 day.

**Driver files:** `crates/protocol/src/lib.rs`,
`apps/desktop-viewer/src/{app.rs, network.rs, status.rs}`,
`apps/android/app/src/main/java/.../` (Kotlin side, exact path TBD).

---

## Items

### D1 — Protocol types
Add to `crates/protocol/src/lib.rs`:

```rust
/// Storage breakdown for the device's primary external storage volume.
/// All values in bytes. Photos+Apps+Music+System+Other should sum to
/// `used`; `free` plus `used` should equal `total`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageBreakdown {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub photos: u64,
    pub videos: u64,
    pub apps: u64,
    pub music: u64,
    pub system: u64,
    pub other: u64,
}

/// Push-style update, fires periodically or on volume mount/unmount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageStatus {
    pub primary: StorageBreakdown,
    pub status: FeatureStatus,
}
```

Add `Payload::StorageStatus(StorageStatus)` variant and bump protocol
version (currently 6 per the connection logs).

### D2 — Desktop status routing
Add `storage: Option<StorageBreakdown>` to `DesktopStatus`. Wire in
`network.rs` (translate the new payload to a `NetworkStatus::Storage`
event); wire in `status.rs` `DesktopStatus::apply`.

### D3 — Android-side query
On the Android app side (this is the Kotlin/Compose app under
`apps/android/`), implement a periodic storage query using:
- `StorageStatsManager.queryStatsForUid(...)` for per-app size
- `MediaStore` queries for photos / videos / music totals
- `StatFs` for free / total

Push a `StorageStatus` once per 5 minutes plus on `ACTION_MEDIA_MOUNTED` /
`ACTION_MEDIA_UNMOUNTED`. Fire `FeatureStatus::PermissionRequired` if
`MANAGE_EXTERNAL_STORAGE` is not granted (it isn't, by default, post-API 30
— surface as the permission-banner UX in the Overview Storage card).

### D4 — Overview Storage card
Per `04-overview.md` row 5. Render in `render_overview_panel` after the
Recent activity grid:

```
┌─ Storage ─────────────────────────────────────────────────────┐
│  108.3  GB used of 128 GB                                     │
│  ────────────────────────────────────────────                 │
│  ████████████░░░░░░░░░░  ← 10px segmented bar, rounded pill,  │
│                            1px subtle border                  │
│  ┌─────────────────────────────────────────────┐              │
│  │ ■ Photos & Video 38%  ■ Apps     21%        │              │
│  │ ■ Music          12%  ■ System    8%        │              │
│  │ ■ Free           21%                        │              │
│  └─────────────────────────────────────────────┘              │
└───────────────────────────────────────────────────────────────┘
```

- Card padding `spacing.xl`, gap `spacing.md`.
- Numeric display uses `LabelSize::Display` for the value, `Label::caption`
  for "GB used of N GB". `.tabular_nums()` on both.
- Segmented bar: nested flex_row of `flex_basis(percent%)` children, each
  with its colour swatch. Photos hue 220, Apps 280, Music 145, System
  neutral 250 (`stroke_neutral`), Free `stroke_neutral_subtle`. Use
  `gradient_from_hue` for the three coloured segments.
- Legend: 2-column grid of `[10px swatch] [Label::body caption] [percent]`
  rows. Compute percent = `category / total * 100`.

### D5 — Storage card empty / unavailable states
- If `status.storage.is_none()` AND we're connected: show
  `SkeletonWrap` of the bar + legend (4 rows skeleton).
- If `storage.status` is `PermissionRequired`: render the
  `permission_banner` ("Enable storage access on the phone.") in place of
  the bar. Hide the legend.
- If we're not connected: don't render the card at all.

---

## Verification

1. `cargo fmt && cargo clippy -- -D warnings && cargo test` in both
   `crates/protocol` and `apps/desktop-viewer`.
2. Bump protocol version + check the Android app refuses old/new mismatches.
3. Plug in an Android device with the new app build: Overview's Storage
   card populates within ~30s with real values. Segmented bar widths look
   right; legend percentages sum to ~100%.
4. Revoke MANAGE_EXTERNAL_STORAGE on the phone → card shows the
   permission-required banner with an "Open onboarding" action button.
5. Disconnect → card disappears (matches the rest of the Overview pattern).
6. Numeric values don't jiggle as the breakdown changes (tabular nums).

When all five verifications pass, commit & push both repos. CI must go
green on each — the protocol bump affects every consuming app.
