> **Workspace root:** [`~/Working/CLAUDE.md`](/home/sprooty/Working/CLAUDE.md) — infrastructure, CI/CD, shared crates, secrets, commit rules.

# AndroidConnect

Local-first Rust stack for mirroring a non-rooted Android device's screen to a Linux desktop with remote input control. Permissively licensed alternative to Phone Link / KDE Connect, scoped tightly around the mirroring + input use case. **Active, MVP1 in progress.**

## 1. Quick reference

| | |
|---|---|
| Type | Rust workspace (`androidconnect-rs/`) + Android Gradle app + custom CI image |
| Repo | `repo.indexarr.net/indexarr/AndroidConnect` |
| CI pipeline | `.woodpecker.yml` — `rust-checks` (fmt/clippy/test) → `android-build` (native `.so` + `assembleDebug`) |
| Deploys to | — (desktop binary + sideloaded APK; no Komodo stack) |
| Image | — (the only image is `repo.indexarr.net/indexarr/androidconnect-rs-ci` — CI build env, not a runtime artefact) |
| Runtime port(s) | `48172/tcp` — desktop viewer's LAN listener (configurable arg) |
| Public URL | — (LAN only; TCP today, QUIC planned) |
| DB / state | None server-side. Android app persists trusted desktop identity in app storage. |
| Secrets used at runtime | None. CI uses `git_auth_token`, `discord_webhook_url` only (standard org secrets). |

## 2. Architecture

```
apps/android (Kotlin + MediaProjection + MediaCodec H.264
              + AccessibilityService for input)
       │   JNI
       ▼
crates/android-native (libandroidconnect_android_native.so)
       │   binary protocol (length-prefixed envelopes)
       ▼
crates/protocol  ←──── shared schema, pairing, session-key derivation
       ▲
       │   TCP :48172
apps/desktop-viewer (winit + pixels + OpenH264 decode + input forwarding)
apps/gpui-spike     (experimental GPUI-rendered viewer)
```

Frame path validated end-to-end on emulator. Physical-device input pass is deferred (MVP1 remaining work item).

## 3. Build / test / run

```bash
cd androidconnect-rs
cargo test --workspace
cargo run -p androidconnect-desktop-viewer -- 0.0.0.0:48172
# optional deterministic pairing for tests:
cargo run -p androidconnect-desktop-viewer -- 0.0.0.0:48172 --pairing-code 123456

# Android native .so (requires Android Rust targets + cargo-ndk):
./tools/build-android-native.sh

# Android APK:
cd apps/android && ./gradlew :app:assembleDebug
```

## 4. Deployment

No production deploy — this is a desktop + sideloaded mobile pair. CI artefacts:

- **`rust-checks`** runs fmt/clippy/test on every push + PR to `main`.
- **`android-build`** runs in a project-specific image `repo.indexarr.net/indexarr/androidconnect-rs-ci:latest` that has the Android SDK + NDK + Rust Android targets pre-installed. **If that image needs updating, rebuild and push it manually** — there is no pipeline that produces it yet.
- No `build-and-push` of a runtime image, no `komodo-deploy` step.
- APKs are not yet uploaded to the Forgejo generic-package registry. If/when you add that, follow the [MyDevEnv2 pattern](../MyDevEnv2/CLAUDE.md#4-deployment) (`scripts/forgejo-api.sh` wrapper, delete-then-PUT idempotence) — never hand-roll inline `curl ... | jq`.

## 5. Rules for AI agents

- **Two roots to pay attention to.** Most code (and `Cargo.toml`) lives in `androidconnect-rs/`, not the project root. `cd androidconnect-rs` before `cargo` commands; the workspace root only holds `.woodpecker.yml` and `.gitignore`.
- **Don't break the pairing protocol lightly.** `crates/protocol` defines wire format + identity-bound pairing. Schema changes need bumps on both `desktop-viewer` and `android-native` simultaneously — there is no version negotiation yet.
- **Physical-device input is unverified.** Emulator-only validation is the current ground truth. Treat input-control changes as needing a real-device QA pass before merging anything load-bearing.
- **TCP is interim.** Replace-with-QUIC is on the MVP1 list (`docs/MVP1.md`). Don't add new code that hard-assumes TCP semantics (e.g. backpressure model, ordered delivery) without flagging it.
- **The Android app can launch without the `.so`.** Encoded frames are silently dropped if `libandroidconnect_android_native.so` isn't packaged. If a tester reports "no video", check the `jniLibs/` directory first.

## 6. Cross-refs

- `androidconnect-rs/README.md` — user-facing build + run flow
- `androidconnect-rs/docs/FEATURE_POSITIONING.md` — product thesis
- `androidconnect-rs/docs/PROJECT_PLAN.md` — milestones, architecture, DoD
- `androidconnect-rs/docs/MVP1.md`, `docs/MVP2.md` — backlogs
- `androidconnect-rs/docs/INPUT_CONTROL.md` — input architecture + validation checklist
