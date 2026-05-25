# QUIC Migration Plan

Replace the current plaintext TCP transport with an encrypted QUIC connection using the
`quinn` crate and `tokio` async runtime. QUIC is TLS 1.3 by design, so this fixes the
unencrypted transport gap and sets up the architecture for separate video/control/input streams.

**Status:** Planning — not yet started.  
**Protocol version:** v9 → v10 at the start of Phase 1.  
**Target:** MVP 1 completion blocker (encryption is an explicit DoD requirement).

---

## Current state summary

| Aspect | Current (TCP) |
|---|---|
| Transport | `TcpStream` (std, synchronous) |
| Framing | 4-byte BE length prefix + bincode `Envelope` |
| Encryption | None — plaintext on the wire |
| Auth | HMAC-SHA256 challenge/response; session key derived but unused for encryption |
| Multiplexing | Single stream — video, control, and input share one TCP connection |
| Android connect direction | Android initiates (`TcpStream::connect` → desktop `TcpListener::accept`) |
| Desktop network model | Two `std::thread` threads per client (reader + writer), `mpsc` for cross-thread commands |
| Android network model | Static `Mutex<SessionState>` holding the write-side stream; dedicated `input_reader_loop` thread for reads |

---

## Target state (after migration)

| Aspect | After (QUIC) |
|---|---|
| Transport | `quinn` QUIC connection over UDP |
| Framing | Same 4-byte + bincode `Envelope` within a QUIC stream |
| Encryption | TLS 1.3 (built into QUIC) — encrypted from first byte |
| Auth | Existing HMAC-SHA256 handshake preserved (runs inside the encrypted stream) |
| Multiplexing | Phase 1: single bidirectional stream (same behaviour as TCP). Phase 3: separate streams |
| Desktop TLS | Self-signed Ed25519 certificate generated at first launch, stored in trust dir |
| Android TLS | Phase 1: server cert not verified (encryption without auth). Phase 4: cert fingerprint pinned during pairing |
| Runtime | tokio Runtime created for the network worker; GPUI main loop unchanged |

---

## TLS strategy

QUIC requires a TLS certificate on the server (desktop) side. The migration handles this in
two stages:

**Phase 1 — Encrypt now, pin later.**  
The desktop generates a self-signed Ed25519 cert at first launch (stored alongside the trust
store). Android connects with a `NoVerification` TLS client config, meaning the traffic is
encrypted against passive eavesdropping but not yet protected against active MitM. The existing
HMAC-SHA256 application-level auth still gates all input — an MitM attacker cannot send input.
This is a significant security improvement over plaintext TCP and ships as part of Phase 1.

**Phase 4 — Certificate pinning.**  
During first pairing, the desktop includes its TLS certificate fingerprint in `AuthResult` (new
field). Android stores it in the trust store alongside `paired_secret`. On subsequent connections,
Android's QUIC client verifies the server cert fingerprint before the app-level auth handshake.
This closes the remaining MitM gap.

---

## Phases

### Phase 0 — Dependency and scaffold (prerequisite)

**Goal:** Add dependencies without changing behaviour. All existing tests still pass.

#### Protocol crate (`crates/protocol/`)

- Bump `PROTOCOL_VERSION` from 9 to 10.
- Add `tls_cert_fingerprint: Option<[u8; 32]>` field to `AuthResult` (used in Phase 4, serialised
  as `None` for now).
- No new crate dependencies yet.

#### Desktop viewer (`apps/desktop-viewer/`)

Dependencies to add:
```toml
quinn      = "0.11"
rustls     = { version = "0.23", default-features = false, features = ["ring"] }
rcgen      = "0.13"    # self-signed cert generation
tokio      = { version = "1", features = ["rt-multi-thread", "net", "time", "sync"] }
```

Note: `rfd` already pulls in tokio transitively. Make the dependency explicit.

#### Android native (`crates/android-native/`)

Dependencies to add:
```toml
quinn      = "0.11"
rustls     = { version = "0.23", default-features = false, features = ["ring"] }
tokio      = { version = "1", features = ["rt-multi-thread", "net", "time", "sync"] }
```

`quinn` requires a tokio runtime. The JNI bridge will create one per session and drive it with
`Runtime::block_on` on the existing native thread. JNI callbacks that need to happen on
JVM-attached threads must be dispatched explicitly — see Phase 2 notes.

**Verification:** `cargo check --workspace` and `cargo test --workspace` green. No behavioural
change.

---

### Phase 1 — Desktop becomes a QUIC server

**Goal:** Desktop accepts QUIC connections. Android still connects via TCP (two transports in
parallel during this phase for rollback safety).

#### Files changed: `apps/desktop-viewer/src/network.rs`

1. **Generate or load the desktop TLS identity** (new function `load_or_create_tls_identity`):
   - Key path: `<trust_dir>/desktop-tls.key` (PEM Ed25519 private key)
   - Cert path: `<trust_dir>/desktop-tls.crt` (PEM self-signed cert, CN=androidconnect-desktop)
   - Generate with `rcgen` on first launch; reload on subsequent launches.
   - Log the cert fingerprint (SHA-256) at startup — users can compare it to what's shown on Android.

2. **Add a parallel QUIC listener** alongside the existing TCP listener:
   ```
   pub fn run_server_quic(bind: &str, ...) — mirrors run_server() but uses QUIC
   ```
   Binds on the same IP but a configurable port (default 48172/udp — same port number, different
   protocol, no conflict since TCP and UDP port namespaces are separate).

3. **QUIC connection handling** (`handle_quic_client`):
   - Accept the QUIC connection.
   - Open (or accept) one bidirectional stream: this is the control stream.
   - Accept one unidirectional stream from Android: video stream.
   - The rest of `read_client_loop` logic is unchanged — it reads `Envelope`s from the control
     stream using the same `read_length_prefixed` / `write_length_prefixed` framing.

4. **Async / sync bridging:**
   GPUI's main loop is not async. The QUIC listener is an async task. Bridge with:
   ```rust
   let rt = tokio::runtime::Runtime::new()?;
   std::thread::spawn(move || rt.block_on(run_server_quic(...)));
   ```
   This keeps the GPUI main thread synchronous and isolates tokio to the network thread.

5. **Protocol version gate:** Reject connections with `envelope.version != PROTOCOL_VERSION` (10)
   as before. TCP connections from v9 Android clients are still accepted by the parallel TCP
   listener during this transition period.

**Verification:** Desktop starts, binds both TCP :48172 and UDP :48172, old Android client (TCP)
still connects and works.

---

### Phase 2 — Android connects via QUIC

**Goal:** Android native uses QUIC. TCP code is removed from both sides once QUIC is stable.

#### Files changed: `crates/android-native/src/lib.rs`

1. **Create tokio Runtime** in `connect_session`:
   ```rust
   let rt = tokio::runtime::Builder::new_multi_thread()
       .worker_threads(2)
       .enable_all()
       .build()?;
   ```
   Store the `Runtime` in `SESSION_STATE` (alongside the stream handle) so it outlives the
   connection.

2. **Replace `TcpStream::connect`** with a `quinn` client connect:
   ```rust
   let endpoint = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
   endpoint.set_default_client_config(insecure_client_config()); // Phase 1 TLS: no verification
   let conn = rt.block_on(endpoint.connect(addr, "androidconnect")?)?.await?;
   ```
   `insecure_client_config()` uses a rustls `ClientConfig` with a custom `ServerCertVerifier`
   that accepts any certificate (encryption without verification — see TLS strategy above).

3. **Open streams:**
   ```rust
   let (send_control, recv_control) = rt.block_on(conn.open_bi())?;
   let send_video = rt.block_on(conn.open_uni())?;
   ```
   The control stream replaces the single TCP stream for auth + utility messages.
   The video stream is a separate unidirectional stream (Android → Desktop).

4. **Adapt send/recv helpers:**
   - `SESSION_STATE.send_payload` → wraps `write_length_prefixed` on the async `SendStream`,
     driven via `rt.block_on(...)`.
   - `input_reader_loop` → reads from `recv_control` using `rt.block_on(read_length_prefixed(...))`.
   - Video frames → written to `send_video` (not the control stream).

5. **Remove TCP code** once QUIC is confirmed stable across emulator + physical device.

#### JNI threading note

Tokio worker threads are not attached to the JVM. Any JNI call (status callbacks, input dispatch)
must happen either:
- On a thread that was attached via `JavaVM::attach_current_thread`, OR
- By posting back to the Java main thread via `Handler.post`.

The existing `input_reader_loop` already attaches via `callbacks.java_vm.attach_current_thread()`.
This works correctly with `rt.block_on` because `block_on` runs on the calling thread (which is
JVM-attached). Worker threads spawned by tokio must also be attached if they make JNI calls —
wrap any such spawn with a `JavaVM::attach_current_thread` guard.

**Verification:** Android connects via QUIC, pairing works, video streams, input works.
TCP listener on desktop can be disabled at this point.

---

### Phase 3 — Stream separation (video off the control stream)

**Goal:** Video frames travel on a dedicated QUIC stream to avoid head-of-line blocking with
control messages. This is the main latency benefit of QUIC over TCP.

Currently (after Phase 2), video is still serialised alongside control messages on the one
bidirectional stream. In Phase 3:

- **Android opens a unidirectional stream** for video (`open_uni`). Frames are sent here.
- **Desktop accepts the unidirectional stream** and decodes frames from it on a separate task.
- The control stream carries only: Hello, AuthChallenge, AuthResponse, AuthResult, DeviceStatus,
  MediaStatus, Notifications, Messages, FileBrowse, Pings, etc.

The protocol `Payload` enum does not need to change — the routing (which stream a payload travels
on) is determined by the sender.

#### Optional: QUIC datagrams for video

QUIC supports unreliable datagrams (like UDP-within-QUIC). Video frames that arrive late are
useless anyway, so switching to datagrams would drop stale frames instead of buffering them.

- Requires `quinn` datagrams feature and `max_datagram_size` configured on the endpoint.
- Maximum datagram size is typically limited by path MTU (~1200 bytes), so large video frames
  must be fragmented and reassembled — adds complexity.
- **Deferred until after Phase 3 is stable.** Reliable streams are fine for LAN use.

---

### Phase 4 — Certificate pinning

**Goal:** Close the remaining MitM gap. After this phase, QUIC provides both encryption and
server identity verification.

#### Protocol change

Add `tls_cert_fingerprint: Option<[u8; 32]>` to `AuthResult` (already scaffolded in Phase 0,
just populate it):

```rust
// In android-native: authenticate_desktop → AuthResult
tls_cert_fingerprint: Some(sha256_of_peer_cert_from_quic_connection),
```

#### Trust store changes (both sides)

- **Desktop trust store** (`apps/desktop-viewer/src/trust.rs`): no change — the desktop's TLS
  cert is already loaded/generated at startup.
- **Android trust store** (`crates/android-native`, `AndroidTrustStore`):
  - Add `tls_cert_fingerprint: [u8; 32]` to the stored pairing record.
  - On first pairing: extract fingerprint from `AuthResult.tls_cert_fingerprint`, store it.
  - On reconnect: pass stored fingerprint to a custom `ServerCertVerifier` that only accepts certs
    matching the pinned fingerprint.

#### Android QUIC client config

Replace `insecure_client_config()` with a fingerprint-pinned config:
```rust
fn pinned_client_config(fingerprint: [u8; 32]) -> ClientConfig {
    // Custom ServerCertVerifier: compare cert's SHA-256 to fingerprint
    ...
}
```
On first pairing (no stored fingerprint), fall back to `insecure_client_config` and then pin
after `AuthResult` is received. On reconnect, use `pinned_client_config`.

---

## Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `quinn` version constraints conflict with rustls version pulled by other deps | Medium | Build breaks | Pin versions explicitly; check with `cargo tree -d` |
| tokio Runtime lifecycle on Android — Runtime dropped too early causing panics | Medium | Crash | Store `Runtime` in SESSION_STATE, only drop on explicit disconnect |
| JNI threads not attached to JVM when tokio worker makes a callback | Medium | Crash | Audit every JNI call path; only call JNI from the JNI-attached thread |
| QUIC UDP blocked on some networks / corporate firewalls | Low for LAN | Connection failure | Keep TCP as a fallback option (configurable) or document the limitation |
| QUIC path MTU issues on some Android Wi-Fi drivers | Low | Degraded performance | Configure `quinn` min MTU; fall back to smaller packets |
| Physical device validation reveals quinn issues not caught on emulator | Medium | Regression | Run Phase 2 validation on a real device before removing TCP code |
| rcgen / rustls WASM/Android cross-compile issues | Low | Build failure | Test android-native build early in Phase 0 |

---

## File-level impact summary

| File | Phase | Change |
|---|---|---|
| `crates/protocol/src/lib.rs` | 0 | PROTOCOL_VERSION 9→10, `AuthResult.tls_cert_fingerprint` field |
| `crates/protocol/Cargo.toml` | 0 | No new deps (protocol stays dep-light) |
| `apps/desktop-viewer/Cargo.toml` | 0 | Add quinn, rustls, rcgen, tokio |
| `apps/desktop-viewer/src/network.rs` | 1–3 | Add QUIC listener, handle_quic_client, stream split |
| `apps/desktop-viewer/src/trust.rs` | 0, 4 | load_or_create_tls_identity; (Phase 4) show cert fingerprint |
| `crates/android-native/Cargo.toml` | 0 | Add quinn, rustls, tokio |
| `crates/android-native/src/lib.rs` | 2–4 | Replace TcpStream with quinn, tokio Runtime, stream split, cert pinning |
| `docs/INPUT_CONTROL.md` | 4 | Update security notice once pinning lands |
| `README.md` | 4 | Remove "transport not encrypted" warning once Phase 4 ships |

---

## Definition of done

- [ ] Phase 0: `cargo check` and `cargo test --workspace` pass with new deps; no behaviour change.
- [ ] Phase 1: Desktop accepts QUIC connections; old TCP Android client still works in parallel.
- [ ] Phase 2: Android connects via QUIC; pairing, video, and input work on emulator.
- [ ] Phase 2 (physical device): Same validation on a real Android device (physical-device QA is
  already a deferred MVP 1 item — do it here).
- [ ] Phase 3: Video and control use separate QUIC streams; no regression in latency or stability.
- [ ] Phase 4: TLS cert fingerprint is stored on Android during pairing; reconnects reject a mismatched cert.
- [ ] TCP listener removed from desktop once Phase 2 is confirmed on physical device.
- [ ] `README.md` security notice updated to reflect encrypted transport.
- [ ] All existing `cargo test --workspace` tests pass throughout.

---

## What this plan explicitly defers

- **QUIC datagrams for video** — latency optimisation after reliable streams are stable (end of Phase 3).
- **QUIC for file transfers on dedicated streams** — fine to keep on the control stream for MVP.
- **Multiple simultaneous desktop clients** — QUIC makes this much cleaner (one connection per client), but multi-client is MVP 2 scope.
- **mTLS / client certificates** — server cert pinning (Phase 4) is sufficient for MVP 1. Client certs add mutual TLS verification but require more UX surface.
- **Relay / NAT traversal** — QUIC's connection migration and ICE-style path selection are useful here, but relay is explicitly MVP 2 scope.
