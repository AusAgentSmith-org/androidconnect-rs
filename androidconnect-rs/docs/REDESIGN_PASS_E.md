# Pass E — Framework follow-ups (parking lot)

**Why this pass exists:** the B-stage shipped backdrop blur as
Linux/WGSL-only and a pragmatic Label::eyebrow workaround stood in for real
GPUI letter-spacing. None of these block the app today, but they're real
debt that should be paid down when there's a concrete driver (a second
platform target, a new design that demands wider letter-spacing on body
text, a slow-GPU complaint).

**Estimated effort:** open-ended; do each only when needed.

**Driver files:** `crates/gpui/src/{color.rs, style.rs, text_system/*.rs,
platform/blade/{blade_renderer.rs, shaders.wgsl}}`,
`crates/gpui/src/platform/{mac/shaders.metal, windows/shaders.hlsl}`,
fluent-app `WindowOptions` plumbing on the Windows side.

---

## Items

### E1 — Real GPUI `Styled::letter_spacing()`

**Trigger to do this:** the design adds long-form uppercase or
letter-spaced body text outside of short eyebrows.

**Approach:**
1. Add `letter_spacing: Option<Pixels>` to `TextStyle` (and let the
   `Refineable` derive propagate to `TextStyleRefinement`).
2. Add `fn letter_spacing(self, spacing: Pixels) -> Self` to GPUI's
   `Styled` trait, mirroring the pattern of `text_size()`.
3. Add `letter_spacing: Option<Pixels>` to `CacheKey` and `CacheKeyRef`
   in `line_layout.rs`. Pass through `layout_line()` call sites
   (`text_system.rs:397, 576`, `line_layout.rs:504`, `line_wrapper.rs:215`).
4. After `platform_text_system.layout_line()` returns, walk
   `layout.runs[].glyphs[]` and add `letter_spacing * glyph_index` to each
   `glyph.position.x`. The existing `force_width` path at
   `line_layout.rs:568` is the template — same shape, different formula.
5. Update `Label::eyebrow` in fluent-primitives to use the real helper
   instead of per-character spans:
   ```rust
   div().text_size(...).letter_spacing(px(font_size * 0.08))
       .child(text.to_uppercase())
   ```
6. Add tests in `gpui/src/text_system/tests.rs` for: zero spacing
   (unchanged layout), positive spacing (advance widths shift correctly),
   negative spacing (kerning-like reduction).

**Effort:** 2 days including cache invalidation testing across font
fallbacks.

### E2 — Backdrop blur on Metal (macOS)

**Trigger:** anyone runs the viewer on macOS and hits the WGSL-only panic.

**Approach:**
1. Open `crates/gpui/src/platform/mac/shaders.metal`. Find the existing
   `fs_quad` and `fs_video_frame` for reference structure.
2. Add `fs_blur` mirroring `shaders.wgsl`'s 17-tap Gaussian (lines 1373+).
3. Extend `fs_quad`'s background switch to handle tag == 3 (BackdropBlur)
   sampling `t_backdrop_blurred` — same logic as WGSL inline branch.
4. Mac's renderer is in `crates/gpui/src/platform/blade/blade_renderer.rs`
   too (Blade is cross-backend via Metal-on-Blade). No Rust changes
   needed beyond the shader file.

**Effort:** 1 day.

### E3 — Backdrop blur on HLSL (Windows)

**Trigger:** anyone runs the viewer on Windows.

**Approach:**
1. `crates/gpui/src/platform/windows/shaders.hlsl` is the file. Same
   mechanical port as E2.
2. Verify with DXC compiler that the new entry points compile.

**Effort:** 1 day.

### E4 — Dual-filter Gaussian blur

**Trigger:** a window-wide blur surface (modal overlays, full-screen
Acrylic-ish effects) or a slow integrated-GPU complaint about Mirror chip
performance.

**Approach:**
1. Allocate two half-resolution scratch textures (`backdrop_half_a`,
   `backdrop_half_b`) sized at `surface_width/2 × surface_height/2`.
2. New WGSL pipelines:
   - `blur_down`: full → half (4 box-filter taps)
   - `blur_up`: half → full (9 tent-filter taps)
3. Modify the blur dispatch in `blade_renderer.rs:draw()`:
   - capture full → backdrop_half_a (downsample)
   - blur_down: backdrop_half_a → backdrop_half_b
   - blur_up: backdrop_half_b → backdrop_blurred
4. Repeat down/up N times for higher-radius blur (Apple/KWin do ~2-3
   passes). N derived from `BlurParams.radius`.
5. Update `BlurParams` to carry pass index (so the same shader can be
   invoked at different scales).

**Effort:** 2 days.

### E5 — Per-quad blur regions (not full surface)

**Trigger:** profiler shows blur copy+pass is hot on a large window.

**Approach:**
1. Walk the quads in a batch, compute the union bounds of all blur quads.
2. In the capture step, copy only that rectangle (Blade supports partial
   `copy_texture_to_texture` via `TexturePiece.origin` + smaller `Extent`).
3. Render blur pass with a viewport scoped to the same rect.
4. The quad fs_quad already computes UVs from device-space position, so no
   shader changes needed.

**Effort:** half day. Pairs naturally with E4.

### E6 — Mica on Windows

**Trigger:** Windows users matter for the product.

**Approach:**
1. Add `WindowOptions::backdrop_material: Option<BackdropMaterial>` enum
   (None / Mica / Acrylic / Tabbed / MicaAlt).
2. In `crates/gpui/src/platform/windows/window.rs`'s `create_window` (or
   wherever `DwmExtendFrameIntoClientArea` is called), translate the enum
   to the right `DWMWA_SYSTEMBACKDROP_TYPE` value via
   `DwmSetWindowAttribute`.
3. The window background must be transparent for Mica to show — set
   `gpu::AlphaMode::PreMultiplied` + clear-to-transparent.
4. Expose at the FluentApp layer: `.backdrop_material(Mica)` builder.
5. Pair with `DWMWA_USE_IMMERSIVE_DARK_MODE` for theme-following.

**Effort:** 1 day.

### E7 — Cross-platform parity test
Once E2/E3 land, add a `blade_renderer` smoke test that creates a backdrop
blur surface on every platform and confirms it doesn't panic. Currently if
someone touches the WGSL without porting Metal/HLSL the failure is at
window creation time, not in CI.

**Effort:** half day.

---

## Verification (when items land individually)

Each item lands with its own verification (above). At the framework level,
the bar is:
- `cargo fmt && cargo clippy -- -D warnings && cargo test` clean in
  `crates/gpui`.
- The FluentGUI demo app (`examples/demo_app`) renders the new feature
  end-to-end on the platforms it targets.
- The androidconnect-rs desktop-viewer continues to build and run.

No single big-bang verification — each item is independently shippable.
