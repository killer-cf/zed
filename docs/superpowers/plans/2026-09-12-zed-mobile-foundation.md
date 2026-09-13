# Zed Mobile Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver a secure, opt-in Tailscale-only Zed Mobile foundation in which an iOS/Android app pairs by QR code with Zed and shows the authenticated host status.

**Architecture:** Add a small Rust wire-contract crate and an opt-in `mobile_server` crate that owns its complete desktop control window and self-registering action. The desktop generates an expiring one-use pairing offer, keeps its private signing key in the platform credential store, persists only non-secret grant metadata in the existing database KVP store, and serves a mutually authenticated WebSocket endpoint bound to an explicitly selected Tailscale address. A new Expo application owns the QR/deep-link flow, secure device storage, connection lifecycle, and host dashboard. The sole Zed application integration is MobileServer initialization; no `settings_ui`, Agent Panel, terminal, project, or Git source changes are part of this foundation. Thread, terminal, Git, worktree, account, browser, and push adapters are deliberately deferred to the four follow-on plans because they depend on this contract and server lifecycle.

**Tech Stack:** Rust 2024, GPUI, `db::KeyValueStore`, `CredentialsProvider`, Tokio, Axum WebSocket, Ed25519, `qrcodegen`, React Native, Expo 55, TypeScript, Zod, Expo Camera, Expo SecureStore, Expo Linking, Vitest, pnpm 9, Node 24.

**Spec:** `docs/superpowers/specs/2026-09-12-zed-mobile-companion-design.md`

## Global Constraints

- Modify `README.md` with the exact mandatory two-line review marker before modifying any Rust, TypeScript, JavaScript, YAML, or test source file; never remove it.
- Mobile is opt-in and binds only an explicitly selected Tailscale address. Reject loopback, unspecified, LAN, public, and wildcard addresses; never expose a public listener, Relay, or LAN fallback.
- The port is stable per host. Set the initial default to `6769`, persist the selected port, and require the user to update the saved endpoint or re-pair after changing it.
- The QR is a short-lived, one-use credential. Do not log, persist, render in titles, or send telemetry containing the pairing secret, host private key, grant token, terminal content, or prompt content.
- Store only the host private signing key in the platform `CredentialsProvider`. Store grant IDs, device public keys, token digests, labels, capabilities, revocation state, host binding configuration, and non-sensitive timestamps in `db::KeyValueStore`.
- Use `zed-mobile://pair?code=<base64url-json>` as the mobile-only deep-link scheme. Do not overload the existing desktop `zed://` scheme or use Orca branding/schemes/assets.
- Each device has a distinct revocable grant. A client proves the pinned server key before sending its token, then proves possession of its device key. Revocation must close live streams and reject future authentication.
- Use JSON WebSocket frames with the versioned `mobile_protocol` types. Do not add the companion protocol to the cloud collaboration `rpc::Peer`/`proto` contract or reuse cloud account authentication.
- Mutating calls reserve a client operation ID now even though the foundation exposes only read and pairing operations; later plans use it to make retries idempotent.
- Keep GPUI entities and `Rc` values on the foreground thread. Network tasks exchange only owned protocol DTOs through channels or `cx.update` boundaries.
- When copying substantial code from the MIT-licensed Orca mobile project, retain its copyright and MIT notice in `mobile/THIRD_PARTY_NOTICES.md`. Do not copy Orca marks, icons, splash artwork, or `orca://` URLs.
- Do not run project-wide tests, formatters, or linters while implementing a task. Run only the named focused checks; run the plan’s manual Tailscale smoke test after the focused checks pass.
- Keep new behavior in `mobile_protocol`, `mobile_server`, future mobile adapter crates, and `mobile/`. For this foundation, upstream files may change only in `README.md`, root `Cargo.toml`, `crates/zed/Cargo.toml`, one startup call in `crates/zed/src/main.rs`, `.gitignore`, and generated-workflow source/output. Do not modify `settings_ui`, `agent_ui`, `terminal`, `project`, `git`, or their tests.
- The desktop control surface is a native window and `zed_mobile::OpenControl` action owned by `mobile_server`; do not add a Settings page, menu item, or settings-schema field.
- Later plans may alter an upstream crate only to add the narrow public adapter method or DTO that the mobile crate cannot otherwise obtain. Keep the original behavior unchanged, test that seam in place, and prohibit broad refactors, relocations, formatting churn, or mobile-specific policy in the upstream crate.

---

## Scope boundary and follow-on plans

This approved feature contains five independently testable subsystems. This plan implements **Epic 1: Foundation**. It produces a real paired mobile host and the stable interface required by the later work; it is not a visual mock or a disposable scaffold.

1. `zed-mobile-thread-control` — Agent Threads ACP, Terminal Threads, terminal/Chat UI streams, prompt/permission replies, terminal input, stop/resume, and OMP restoration.
2. `zed-mobile-worktrees-git` — worktree catalogue/creation, files, Git status/diffs, staging, unstaging, commits, and trust/askpass policy.
3. `zed-mobile-operations` — Quick Commands, accounts/usage, attachments, browser Mobile view, and notifications.
4. `zed-mobile-parity` — accessibility, device UI tests, iOS/Android release workflows, offline/background resilience, and compatibility migrations.

Each plan must consume the protocol and pairing interfaces defined below. Do not expose placeholder RPC methods for later epics; capabilities advertise only functionality that exists on the host.

## File Structure

| Path | Responsibility |
| --- | --- |
| `README.md` | Mandatory review marker before the first source/test edit. |
| `Cargo.toml` | Adds `mobile_protocol` and `mobile_server` workspace members and common dependencies. |
| `crates/mobile_protocol/Cargo.toml` | Independent wire-schema crate manifest with named library root. |
| `crates/mobile_protocol/src/mobile_protocol.rs` | Canonical versioned JSON DTOs, capabilities, request/response/event envelopes, protocol validation, and fixtures. |
| `crates/mobile_server/Cargo.toml` | Local MobileServer crate manifest and named library root. |
| `crates/mobile_server/src/mobile_server.rs` | GPUI global lifecycle, host binding configuration, endpoint validation, ephemeral offers, authenticated WebSocket sessions, live-connection registry, and server tests. |
| `crates/mobile_server/src/mobile_store.rs` | Credential/KVP-backed host identity and durable device-grant repository. |
| `crates/mobile_server/src/mobile_window.rs` | Self-contained GPUI Mobile control window and `zed_mobile::OpenControl` action: enable/disable, safe address/port selection, QR, copy code, device list, and revoke. |
| `crates/zed/Cargo.toml` | Adds the direct MobileServer dependency. |
| `crates/zed/src/main.rs` | Initializes MobileServer once after database and credentials exist; stops it on application teardown. |
| `mobile/package.json` | Isolated Expo package, pinned Node/pnpm metadata, deterministic scripts and dependencies. |
| `mobile/pnpm-lock.yaml` | Locked mobile JavaScript dependency graph. |
| `mobile/app.json` | Expo identity, `zed-mobile` URL scheme, required camera and notification metadata, and original app assets. |
| `mobile/src/protocol.ts` | Exact TypeScript mirror of version 1 mobile frames and runtime Zod validation. |
| `mobile/src/storage/paired-host-store.ts` | Expo SecureStore persistence and atomic deletion of device identity/token/host pin. |
| `mobile/src/transport/mobile-rpc-client.ts` | Server-key-pinned pairing/authentication, heartbeat, foreground reconnect, observable connection state, and `status.get`. |
| `mobile/src/transport/mobile-rpc-client.test.ts` | Deterministic fake-WebSocket tests for handshake, revoke, version block, half-open recovery, and retry idempotency. |
| `mobile/app/index.tsx` | Paired-host dashboard and offline/incompatible/revoked state. |
| `mobile/app/pair.tsx` | Camera scan, deep-link, paste pairing, permission fallback, and secure persistence flow. |
| `mobile/src/features/pairing/pairing-code.ts` | Base64url pairing-code parser shared by scan, paste, and deep-link entry points. |
| `mobile/src/features/pairing/pairing-code.test.ts` | Pairing URL and malformed/expired offer parsing tests. |
| `mobile/THIRD_PARTY_NOTICES.md` | MIT notice for copied Orca portions plus npm/Expo attribution process. |
| `mobile/README.md` | Local prerequisites, focused commands, Tailscale-only real-device smoke instructions, and app-store prerequisites. |
| `.gitignore` | Ignores `mobile/node_modules`, `.expo`, platform build output, and local Expo environment files. |
| `tooling/xtask/src/tasks/workflows/run_tests.rs` | Adds mobile-path change detection and a Node 24/pnpm 9 mobile test/typecheck CI job. |
| `.github/workflows/run_tests.yml` | Regenerated workflow generated from the xtask source. |

### Task 1: Define the versioned companion wire contract

**Files:**
- Modify: `README.md:1`
- Modify: `Cargo.toml:3-266,513-927`
- Create: `crates/mobile_protocol/Cargo.toml`
- Create: `crates/mobile_protocol/src/mobile_protocol.rs`
- Test: `crates/mobile_protocol/src/mobile_protocol.rs`

**Interfaces:**
- Consumes: workspace `serde`, `serde_json`, `time`, `uuid`, `base64`, `thiserror`, and `zeroize` dependency conventions.
- Produces: `mobile_protocol::{authentication_payload, pairing_payload, Capability, ClientFrame, ServerFrame, PairingOffer, ProtocolError, Status, MOBILE_PROTOCOL_VERSION, MIN_COMPATIBLE_MOBILE_VERSION}` used identically by `mobile_server` and the TypeScript mirror.

- [ ] **Step 1: Add the review marker and crate manifests**

Ensure `README.md` begins with:

```markdown
> [!IMPORTANT]
> Remove this line to confirm you've reviewed this PR before submitting.
```

Add `"crates/mobile_protocol"` and `"crates/mobile_server"` to the workspace member list. Register `mobile_protocol` and `mobile_server` as path dependencies. Add the new external dependencies once at workspace scope:

```toml
ed25519-dalek = { version = "2", features = ["rand_core", "serde"] }
if-addrs = "0.14"
qrcodegen = "1.8"
rand_core = { version = "0.6", features = ["getrandom"] }
subtle = "2.6"
```

Create the protocol manifest with a descriptive library root rather than `lib.rs`:

```toml
[lib]
path = "src/mobile_protocol.rs"

[dependencies]
base64.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
time.workspace = true
uuid.workspace = true
zeroize.workspace = true
```

- [ ] **Step 2: Write failing protocol round-trip and rejection tests**

Add tests for a complete version-one pairing offer, a `status.get` request with a UUID operation ID, an authenticated status response, and every invalid envelope case. Build a real offer in the test fixture instead of using a placeholder:

```rust
let offer = test_pairing_offer();
let encoded_offer = offer.encode_url()?;
assert_eq!(
    PairingOffer::decode_url(&encoded_offer)?.endpoint,
    "ws://100.88.4.2:6769"
);
assert!(PairingOffer::decode_url("orca://pair?code=abc").is_err());
assert!(PairingOffer::decode_url("zed-mobile://pair?code=not-json").is_err());
assert!(ClientFrame::from_json(
    r#"{"type":"request","request_id":"00000000-0000-0000-0000-000000000000","operation_id":null,"method":"status.get","params":{}}"#
).is_ok());
assert!(ClientFrame::from_json(
    r#"{"type":"request","method":"status.get","params":{}}"#
).is_err());
```

Cover `Capability` serialization in stable snake case and reject unknown fields with `serde(deny_unknown_fields)` on every externally decoded struct.

Add these exact `ServerFrame` variants:

```rust
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerFrame {
    PairChallenge {
        offer_id: Uuid,
        client_nonce: String,
        server_nonce: String,
        host_signature: String,
    },
    PairComplete { grant_id: Uuid, token: String },
    ServerChallenge {
        grant_id: Uuid,
        client_nonce: String,
        server_nonce: String,
        host_signature: String,
    },
    Authenticated { grant_id: Uuid },
    Response {
        request_id: Uuid,
        operation_id: Option<Uuid>,
        result: serde_json::Value,
    },
    Event { event: String, payload: serde_json::Value },
    Pong { nonce: String },
    Error {
        request_id: Option<Uuid>,
        code: String,
        message: String,
    },
}
```

`Status` contains exactly `host_name: String`, `protocol_version: u16`, `minimum_compatible_mobile_version: u16`, and a sorted `Vec<Capability>`. `PairingOffer::encode_url` and `decode_url` must accept only the exact `zed-mobile` scheme, `pair` host, one `code` query parameter, padded or unpadded base64url input, and an RFC 3339 expiry.

Define these function signatures and field order in this crate:

```rust
pub fn pairing_payload(
    protocol_version: u16,
    offer_id: Uuid,
    client_nonce: &str,
    server_nonce: &str,
) -> Result<Vec<u8>, ProtocolError>;
pub fn authentication_payload(
    protocol_version: u16,
    grant_id: Uuid,
    client_nonce: &str,
    server_nonce: &str,
    token_digest: &[u8; 32],
) -> Result<Vec<u8>, ProtocolError>;
```

Both produce the bytes that Ed25519 signs and verifies. Encode no challenge material as JSON: begin with the literal domain (`zed-mobile/pair/v1` or `zed-mobile/auth/v1`), then append protocol version as big-endian `u16`, UUID bytes, and each nonce/digest as a big-endian `u32` byte length followed by its decoded bytes, in the function parameter order above. Add fixed hex vectors for both functions and require the TypeScript mirror to match them byte-for-byte.


- [ ] **Step 3: Run the protocol tests to verify they fail**

Run: `cargo test -p mobile_protocol`

Expected: compile failure because `mobile_protocol` and the asserted DTOs do not yet exist.

- [ ] **Step 4: Implement owned JSON DTOs and URL codec**

Implement no GPUI, terminal, project, or network types. Use only owned strings, byte arrays represented as base64url strings, `Uuid`, and `OffsetDateTime`. Define the following wire shapes:

```rust
pub const MOBILE_PROTOCOL_VERSION: u16 = 1;
pub const MIN_COMPATIBLE_MOBILE_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairingOffer {
    pub offer_id: Uuid,
    pub endpoint: String,
    pub protocol_version: u16,
    pub host_public_key: String,
    pub pairing_secret: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    StatusRead,
    ThreadsRead,
    ThreadsControl,
    TerminalStream,
    TerminalInput,
    ChatSend,
    AttachmentsSend,
    WorktreesRead,
    WorktreesCreate,
    FilesRead,
    SourceControlWrite,
    QuickCommands,
    AccountsRead,
    AccountsSwitch,
    UsageRead,
    Notifications,
    BrowserMobileView,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    PairBegin { offer_id: Uuid, client_nonce: String },
    PairComplete {
        offer_id: Uuid,
        pairing_secret: String,
        client_public_key: String,
        client_proof: String,
        device_label: String,
    },
    Connect { grant_id: Uuid, client_nonce: String },
    Authenticate { grant_id: Uuid, token: String, client_proof: String },
    Request { request_id: Uuid, operation_id: Option<Uuid>, method: String, params: serde_json::Value },
    Ping { nonce: String },
}
```

Add matching `ServerFrame` variants: `pair_challenge`, `pair_complete`, `server_challenge`, `authenticated`, `response`, `event`, `pong`, and `error`. `Status` contains the host display name, protocol and minimum compatible versions, and a sorted `Vec<Capability>`. `PairingOffer::encode_url` and `decode_url` must accept only the exact `zed-mobile` scheme, `pair` host, one `code` query parameter, padded or unpadded base64url input, and an RFC 3339 expiry.

Define `pairing_payload` and `authentication_payload` in this crate; both produce the bytes that Ed25519 signs and verifies. Encode no challenge material as JSON: begin with the literal domain (`zed-mobile/pair/v1` or `zed-mobile/auth/v1`), then append protocol version as big-endian `u16`, UUID bytes, and each nonce/digest as a big-endian `u32` byte length followed by its decoded bytes. `authentication_payload` additionally appends the 32-byte SHA-256 token digest. Add fixed hex vectors for both functions and require the TypeScript mirror to match them byte-for-byte.

- [ ] **Step 5: Run focused protocol tests**

Run: `cargo test -p mobile_protocol`

Expected: PASS. Tests verify byte-stable JSON, only the Zed mobile URL scheme, required IDs, invalid data rejection, and capability compatibility.

- [ ] **Step 6: Commit the protocol contract**

```bash
git add README.md Cargo.toml crates/mobile_protocol/Cargo.toml crates/mobile_protocol/src/mobile_protocol.rs
git commit -m "Add mobile protocol contract"
```

### Task 2: Persist host identity and revocable device grants

**Files:**
- Create: `crates/mobile_server/Cargo.toml`
- Create: `crates/mobile_server/src/mobile_store.rs`
- Create: `crates/mobile_server/src/mobile_server.rs`
- Test: `crates/mobile_server/src/mobile_store.rs`

**Interfaces:**
- Consumes: `db::KeyValueStore`, `credentials_provider::CredentialsProvider`, `ed25519_dalek::{SigningKey, VerifyingKey}`, `mobile_protocol::{Capability, authentication_payload, pairing_payload}`, and GPUI `AsyncApp`.
- Produces:

```rust
pub const MOBILE_CREDENTIALS_URL: &str = "zed://mobile-server";
pub const MOBILE_HOST_KEY_USERNAME: &str = "host-signing-key";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MobileBinding {
    pub enabled: bool,
    pub address: IpAddr,
    pub port: NonZeroU16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeviceGrant {
    pub id: Uuid,
    pub label: String,
    pub client_public_key: String,
    pub token_digest: String,
    pub capabilities: BTreeSet<Capability>,
    pub created_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

pub struct MobileStore;

impl MobileStore {
    pub fn new(
        key_value_store: KeyValueStore,
        credentials_provider: Arc<dyn CredentialsProvider>,
    ) -> Self;
    pub async fn load_or_create_host_key(&self, cx: &AsyncApp) -> anyhow::Result<SigningKey>;
    pub async fn save_binding(&self, binding: MobileBinding) -> anyhow::Result<()>;
    pub async fn load_binding(&self) -> anyhow::Result<Option<MobileBinding>>;
    pub async fn insert_grant(&self, grant: DeviceGrant) -> anyhow::Result<()>;
    pub async fn grant(&self, id: Uuid) -> anyhow::Result<Option<DeviceGrant>>;
    pub async fn grants(&self) -> anyhow::Result<Vec<DeviceGrant>>;
    pub async fn revoke_grant(&self, id: Uuid, at: OffsetDateTime) -> anyhow::Result<bool>;
}
```

`MobileBinding` contains only `enabled`, `IpAddr`, and `NonZeroU16` port. `MobileServer` consumes this repository in Task 3.

- [ ] **Step 1: Write failing storage tests with the in-memory database and fake credentials provider**

Use `AppDatabase::test_new`/`KeyValueStore` patterns from `crates/db/src/kvp.rs` and a test-only `CredentialsProvider` that records passwords. Assert these invariants:

```rust
let first_key = store.load_or_create_host_key(&async_cx).await?;
let second_key = store.load_or_create_host_key(&async_cx).await?;
assert_eq!(first_key.verifying_key(), second_key.verifying_key());
assert_ne!(first_key.to_bytes(), [0; 32]);

store.insert_grant(grant.clone()).await?;
assert_eq!(store.grants().await?, vec![grant.clone()]);
assert!(store.revoke_grant(grant.id, OffsetDateTime::now_utc()).await?);
assert!(store.grant(grant.id).await?.unwrap().revoked_at.is_some());
```

Assert that the KVP contents never contain the host private-key bytes or a raw device token, and that invalid public-key/base64 material fails loading without becoming a new grant.

- [ ] **Step 2: Run the storage tests to verify they fail**

Run: `cargo test -p mobile_server mobile_store`

Expected: compile failure because `MobileStore`, `DeviceGrant`, and the mobile server crate do not exist.

- [ ] **Step 3: Implement secure/private versus durable/non-secret storage**

Create the named library root:

```toml
[lib]
path = "src/mobile_server.rs"

[dependencies]
anyhow.workspace = true
axum = { version = "0.6", features = ["json", "ws"] }
base64.workspace = true
credentials_provider.workspace = true
db.workspace = true
ed25519-dalek.workspace = true
futures.workspace = true
gpui.workspace = true
if-addrs.workspace = true
mobile_protocol.workspace = true
qrcodegen.workspace = true
rand_core.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
subtle.workspace = true
time.workspace = true
tokio = { workspace = true, features = ["net", "rt", "sync", "time"] }
uuid.workspace = true
zeroize.workspace = true
```

Generate the 32-byte Ed25519 secret with `SigningKey::generate(&mut rand_core::OsRng)`, serialize it only into `CredentialsProvider::write_credentials(MOBILE_CREDENTIALS_URL, MOBILE_HOST_KEY_USERNAME, ...)`, and zeroize temporary decoded bytes. Store the public key as base64url. Hash raw grant tokens with SHA-256 before the KVP write; compare digests in constant time. KVP keys must be namespaced below `mobile_server/v1/` and include separate binding and grant-list records. Validate all persisted values before returning them; corruption must return an error and leave the current listener stopped.

- [ ] **Step 4: Run focused storage tests**

Run: `cargo test -p mobile_server mobile_store`

Expected: PASS. Tests prove stable host identity, durable non-secret binding/grant data, grant revocation, and rejection of malformed secrets/data.

- [ ] **Step 5: Commit secure pairing storage**

```bash
git add crates/mobile_server/Cargo.toml crates/mobile_server/src/mobile_server.rs crates/mobile_server/src/mobile_store.rs
git commit -m "Add mobile pairing storage"
```

### Task 3: Serve a Tailscale-bound, mutually authenticated status endpoint

**Files:**
- Modify: `crates/mobile_server/src/mobile_server.rs`
- Modify: `crates/mobile_server/src/mobile_store.rs`
- Test: `crates/mobile_server/src/mobile_server.rs`

**Interfaces:**
- Consumes: `MobileStore`, `PairingOffer`, `ClientFrame`, `ServerFrame`, `DeviceGrant`, Tokio, and the versioned protocol crate.
- Produces:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MobileServerStatus {
    Disabled,
    Starting,
    Listening { endpoint: String, connected_devices: usize },
    Error { message: SharedString },
}

pub struct MobileServer;

impl Global for MobileServer {}

impl MobileServer {
    pub fn init(store: MobileStore, host_name: SharedString, cx: &mut App);
    pub fn enable(&mut self, binding: MobileBinding, cx: &mut App) -> Task<anyhow::Result<()>>;
    pub fn disable(&mut self, cx: &mut App) -> Task<anyhow::Result<()>>;
    pub fn status(&self) -> MobileServerStatus;
    pub fn create_pairing_offer(&mut self, now: OffsetDateTime) -> anyhow::Result<PairingOffer>;
    pub fn grants(&self) -> &[DeviceGrant];
    pub fn revoke_grant(&mut self, id: Uuid, cx: &mut App) -> Task<anyhow::Result<bool>>;
}

pub fn tailscale_addresses() -> anyhow::Result<Vec<IpAddr>>;
pub fn pairing_offer_svg(offer: &PairingOffer) -> anyhow::Result<String>;
```

The foundation status response advertises exactly `[Capability::StatusRead]`. Later plans extend this list only after their host adapter works.

- [ ] **Step 1: Write failing endpoint, pairing, and revocation integration tests**

Add tests that run an ephemeral loopback listener only under `cfg(test)`; production validation must still reject loopback. Cover:

```rust
assert!(validate_mobile_binding("127.0.0.1", 6769).is_err());
assert!(validate_mobile_binding("0.0.0.0", 6769).is_err());
assert!(validate_mobile_binding("192.168.1.4", 6769).is_err());
assert!(validate_mobile_binding("8.8.8.8", 6769).is_err());
assert_eq!(validate_mobile_binding("100.88.4.2", 6769)?.port.get(), 6769);
```

For a valid test listener, verify this exact sequence:

1. `PairBegin` receives a signed `pair_challenge` before the phone sends `pairing_secret`.
2. A valid `PairComplete` consumes the offer, creates one grant and returns one opaque token.
3. Reusing the offer fails with `offer_consumed`; waiting beyond the offer expiry fails with `offer_expired`.
4. `Connect` receives a server challenge signed by the QR-pinned host public key; only then does the client send the token/device proof.
5. `status.get` returns version 1 and only `status_read`.
6. `revoke_grant` closes the established socket and a new authentication attempt returns `grant_revoked`.

- [ ] **Step 2: Run endpoint tests to verify they fail**

Run: `cargo test -p mobile_server mobile_server`

Expected: FAIL because there is no listener, challenge flow, or status handler.

- [ ] **Step 3: Implement endpoint validation and offer lifecycle**

Implement `validate_mobile_binding` with `IpAddr`: accept only IPv4 addresses in `100.64.0.0/10` and IPv6 addresses in `fd7a:115c:a1e0::/48`, which are Tailscale’s assigned address ranges. Reject non-Tailscale addresses even if private. Start a Tokio listener only after validation and retain its cancellation task in `MobileServer`; stopping or application teardown signals the task and waits for listener/subscription shutdown.

Keep only unconsumed offers in memory. An offer expires five minutes after creation. A new offer removes all previous unconsumed offers; consuming removes it before grant creation. Build the advertised endpoint as `ws://<selected-tailscale-address>:<stable-port>` and return the base64url `zed-mobile://pair?code=...` form only to the desktop QR renderer.

- [ ] **Step 4: Implement authentication and framed status RPC**

Use Axum’s WebSocket upgrade only after the TCP listener is bound to a validated address. Decode each text frame as `ClientFrame`, write JSON `ServerFrame`, set a maximum frame size of 64 KiB, and close malformed or unexpected state transitions. Use this state machine:

```text
unpaired → PairBegin → PairChallenge → PairComplete → Authenticated
known     → Connect → ServerChallenge → Authenticate → Authenticated
```

`PairChallenge` and `ServerChallenge` use the `pairing_payload` and `authentication_payload` byte encoders from `mobile_protocol`; do not reconstruct a signing payload independently in server or app code. Verify the signature in test clients against the QR-pinned public key before transmitting `pairing_secret` or `token`. `PairComplete` checks the one-use offer, expiry, secret digest, device label length, client Ed25519 public key, and a signature over the exact pairing payload. It creates a random 256-bit grant token, persists only its digest, and returns the raw token once. `Authenticate` checks the active grant and token digest, then verifies the device signature over the exact authentication payload.

Track each authenticated socket by grant ID. `revoke_grant` persists the revocation first, signals every matching socket to close, removes it from the registry, and emits an updated `MobileServerStatus`. `status.get` is rejected before authentication and responds with the exact `Status` DTO: protocol versions, host name, and the foundation capability list. Endpoint health remains transport-local in `MobileServerStatus`; do not add a wire field outside the Task 1 `Status` contract.

- [ ] **Step 5: Run the focused Rust suites**

Run:

```bash
cargo test -p mobile_protocol
cargo test -p mobile_server
```

Expected: PASS. The protocol vectors, hostile binding cases, one-use offers, host-key pinning, authentication order, grant revocation, and status compatibility all pass.

- [ ] **Step 6: Commit the live MobileServer**

```bash
git add crates/mobile_server/src/mobile_server.rs crates/mobile_server/src/mobile_store.rs
git commit -m "Add Tailscale mobile server"
```

### Task 4: Integrate the self-contained Mobile control window

**Files:**
- Modify: `crates/zed/Cargo.toml:1-240`
- Modify: `crates/zed/src/main.rs:345-355,470-585,630-655`
- Create: `crates/mobile_server/src/mobile_window.rs`
- Modify: `crates/mobile_server/src/mobile_server.rs`
- Test: `crates/mobile_server/src/mobile_window.rs`

**Interfaces:**
- Consumes: `AppDatabase`, `KeyValueStore`, `zed_credentials_provider::global`, `MobileServer`, and GPUI window/clipboard primitives.
- Produces: `mobile_server::OpenControl`, a self-registering command-palette action that opens a native Mobile control window with server state, safe address/port selection, `Enable`, `Disable`, `Generate pairing QR`, `Copy pairing code`, paired-device list, and per-device `Revoke`.

- [ ] **Step 1: Write failing window-state and redaction tests in the new crate**

Install a fake `MobileStore`/credential provider and `MobileServer::init` in `TestAppContext`. Add tests that exercise the view model exposed by `mobile_window.rs`:

```rust
#[gpui::test]
async fn test_mobile_control_shows_disabled_state(cx: &mut TestAppContext) {
    let model = MobileControlModel::new(cx);
    assert!(matches!(model.server_status(cx), MobileServerStatus::Disabled));
}

#[gpui::test]
async fn test_mobile_control_never_exposes_pairing_secret_as_text(cx: &mut TestAppContext) {
    let offer = test_pairing_offer();
    let display = MobileControlModel::pairing_display(&offer)?;
    assert!(display.qr_svg.contains("<svg"));
    assert!(!display.visible_text.contains(&offer.pairing_secret));
    assert!(!display.visible_text.contains(&offer.encode_url()?));
}
```

Also test that `tailscale_addresses()` filters a mixed interface fixture to only `100.64.0.0/10` and `fd7a:115c:a1e0::/48`, that an unsafe port/address cannot invoke `enable`, and that `revoke_grant` removes the device from the next model snapshot.

- [ ] **Step 2: Run focused window tests to verify they fail**

Run: `cargo test -p mobile_server mobile_window`

Expected: compile failure because the Mobile control action/window/model do not exist.

- [ ] **Step 3: Keep the Zed integration to one initialization boundary**

Add only the direct `mobile_server` and any already-required direct `db`/`zed_credentials_provider` dependencies to `crates/zed/Cargo.toml`. In `main.rs`, after `AppDatabase` and `zed_credentials_provider` are globally available, construct `MobileStore` from the existing `KeyValueStore` and call:

```rust
MobileServer::init(mobile_store, app_name.into(), cx);
```

The initializer owns global action registration, control-window creation, server teardown registration, and every Mobile UI detail. Do not register an action in `zed`, modify an app menu, add a settings field, or touch `settings_ui`. The startup delta must remain one initialization statement plus imports.

- [ ] **Step 4: Implement the Mobile-owned action and native window**

In `mobile_window.rs`, declare:

```rust
actions!(zed_mobile, [OpenControl]);

pub fn init_mobile_window(cx: &mut App) {
    cx.on_action(|_: &OpenControl, cx| {
        cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| MobileControlWindow::new(window, cx))
        })
        .log_err();
    });
}
```

Call `init_mobile_window` only from `MobileServer::init`. `MobileControlWindow` owns its `MobileControlModel`, renders the server/grant snapshot entirely from `MobileServer::try_global`, and calls only `MobileServer::{enable,disable,create_pairing_offer,revoke_grant}`. Address selection comes from `tailscale_addresses()`; it never shells out to `tailscale` or offers a manual unsafe address. The pairing display renders `pairing_offer_svg(&offer)` and its expiry, while `Copy pairing code` is the only control that reads the raw URL for the existing clipboard API. Register the action with the command palette through the normal action inventory; no source change in `command_palette` is allowed.

- [ ] **Step 5: Run focused window and integration checks**

Run:

```bash
cargo test -p mobile_server mobile_window
cargo test -p mobile_server mobile_server
```

Expected: PASS. The action/window is entirely owned by the new crate, unsafe binding is blocked, QR text is redacted, and revoke updates both connection registry and displayed grants.

- [ ] **Step 6: Commit the narrow desktop boundary**

```bash
git add crates/mobile_server/src/mobile_window.rs crates/mobile_server/src/mobile_server.rs crates/zed/Cargo.toml crates/zed/src/main.rs
git commit -m "Add mobile control window"
```

### Task 5: Create the licensed Expo companion package and protocol mirror

**Files:**
- Modify: `.gitignore`
- Create: `mobile/package.json`
- Create: `mobile/pnpm-lock.yaml`
- Create: `mobile/app.json`
- Create: `mobile/tsconfig.json`
- Create: `mobile/vitest.config.ts`
- Create: `mobile/src/protocol.ts`
- Create: `mobile/src/protocol.test.ts`
- Create: `mobile/THIRD_PARTY_NOTICES.md`
- Create: `mobile/README.md`

**Interfaces:**
- Consumes: the JSON serialization vectors in `mobile_protocol` and the QR URL shape from Task 1.
- Produces: a standalone Node 24/pnpm 9 Expo project whose `MobileProtocol` type family is byte-compatible with the Rust protocol.

- [ ] **Step 1: Write failing TypeScript protocol-vector tests**

Put these fixtures in `mobile/src/protocol.test.ts` before implementation:

```ts
expect(decodePairingUrl(validUrl)).toEqual({
  offerId: "018f0aa6-8d3b-7ccf-9fe0-cd23fb40ad40",
  endpoint: "ws://100.88.4.2:6769",
  protocolVersion: 1,
  hostPublicKey: "base64url-key",
  pairingSecret: "base64url-secret",
  expiresAt: "2026-09-12T12:05:00Z",
});
expect(() => decodePairingUrl("orca://pair?code=abc")).toThrow("invalid pairing URL");
expect(() => parseServerFrame({ type: "response" })).toThrow("invalid server frame");
```

Add fixtures matching the Rust JSON test output exactly; check the `type` discriminants, `request_id`, optional `operation_id`, capability names, and ISO timestamp parsing.

- [ ] **Step 2: Create the isolated Expo package and run failing tests**

Create a `mobile/package.json` pinned to Node 24 and pnpm 9 with these scripts:

```json
{
  "scripts": {
    "start": "expo start --dev-client",
    "android": "expo run:android",
    "ios": "expo run:ios",
    "test": "vitest run",
    "typecheck": "tsc --noEmit",
    "lint": "eslint . --max-warnings=0"
  }
}
```

Add Expo 55, React Native 0.83, React 19, `expo-camera`, `expo-clipboard`, `expo-crypto`, `expo-linking`, `expo-network`, `expo-notifications`, `expo-secure-store`, `expo-status-bar`, `expo-router`, `react-native-safe-area-context`, `react-native-screens`, `react-native-svg`, `react-test-renderer`, `tweetnacl`, `zod`, `zustand`, `vitest`, `typescript`, `eslint`, `@testing-library/react-native`, `@types/react-test-renderer`, and their required Expo-compatible versions. Set `zed-mobile` as the only app scheme in `app.json`; request camera permission with a Zed-specific explanation.

Run: `pnpm --dir mobile test`

Expected: FAIL because `src/protocol.ts` is absent.

- [ ] **Step 3: Implement exact Zod protocol parsing**

Create `mobile/src/protocol.ts` with Zod schemas that reject unknown fields and mirror every Task 1 `ClientFrame`/`ServerFrame` discriminant. Base64url decoding must restore padding before parsing UTF-8 JSON. Mirror `pairing_payload` and `authentication_payload` with `Uint8Array` using the exact domain bytes, big-endian version/length prefixes, UUID bytes, nonce bytes, and token digest specified in Task 1; assert their fixed hex vectors in `protocol.test.ts`. `decodePairingUrl` accepts only `zed-mobile://pair?code=...`; scan results, paste input, and deep links share this one function. Export `protocolVersion = 1` and `minimumCompatibleDesktopVersion = 1`; do not duplicate raw literals across screens.

- [ ] **Step 4: Add app policy, notices, and ignored output**

Add `.expo/`, `mobile/node_modules/`, `mobile/android/.gradle/`, `mobile/android/app/build/`, `mobile/ios/Pods/`, `mobile/ios/build/`, and `mobile/.env*` to `.gitignore`. Create original app icon/splash placeholders only if they are actual Zed-owned artwork; do not import Orca image files. `mobile/THIRD_PARTY_NOTICES.md` must contain the full MIT notice for any copied Orca source and list each direct npm dependency attribution source. `mobile/README.md` must name Node 24, pnpm, Xcode/Android Studio, an Expo development client, Tailscale on host/phone, the focused commands, and the fact that Expo Go cannot load native development-client dependencies when used.

- [ ] **Step 5: Run focused mobile project checks**

Run:

```bash
pnpm --dir mobile test
pnpm --dir mobile typecheck
pnpm --dir mobile lint
```

Expected: PASS. The package is independent of Cargo, the mirror accepts the Rust vectors, rejects Orca/invalid URLs, and has reproducible lockfile/tooling metadata.

- [ ] **Step 6: Commit the mobile project base**

```bash
git add .gitignore mobile/package.json mobile/pnpm-lock.yaml mobile/app.json mobile/tsconfig.json mobile/vitest.config.ts mobile/src/protocol.ts mobile/src/protocol.test.ts mobile/THIRD_PARTY_NOTICES.md mobile/README.md
git commit -m "Add mobile companion project"
```

### Task 6: Implement pairing, secure host persistence, and host status UI

**Files:**
- Modify: `mobile/vitest.config.ts`
- Create: `mobile/src/storage/paired-host-store.ts`
- Create: `mobile/src/storage/paired-host-store.test.ts`
- Create: `mobile/src/transport/mobile-rpc-client.ts`
- Create: `mobile/src/transport/mobile-rpc-client.test.ts`
- Create: `mobile/src/features/pairing/pairing-code.ts`
- Create: `mobile/src/features/pairing/pairing-code.test.ts`
- Create: `mobile/app/_layout.tsx`
- Create: `mobile/app/index.tsx`
- Create: `mobile/app/pair.tsx`
- Test: `mobile/app/index.test.tsx`
- Test: `mobile/app/pair.test.tsx`

**Interfaces:**
- Consumes: `PairingOffer`/frame schemas, Expo SecureStore, Expo Camera/Linking/AppState/Network, WebSocket, TweetNaCl, and the Rust status handshake from Task 3.
- Produces:

```ts
export type PairedHost = {
  id: string;
  endpoint: string;
  hostPublicKey: string;
  grantId: string;
  token: string;
  deviceSecretKey: string;
  displayName: string;
};

export type ConnectionState =
  | "disconnected"
  | "pairing"
  | "connecting"
  | "connected"
  | "reconnecting"
  | "unreachable"
  | "revoked"
  | "incompatible"
  | "key_mismatch";

export class MobileRpcClient {
  static pair(offer: PairingOffer, label: string): Promise<PairedHost>;
  connect(host: PairedHost): Promise<Status>;
  status(): ConnectionState;
  retry(): void;
  notifyForeground(): void;
  close(): void;
}
```

- [ ] **Step 1: Write failing storage and transport state-machine tests**

Test a fake SecureStore and fake WebSocket implementation. Cover secure store calls and client behavior, not React Native internals:

```ts
await hosts.save(pairedHost);
expect(await hosts.list()).toEqual([pairedHost]);
await hosts.remove(pairedHost.id);
expect(await hosts.list()).toEqual([]);

await client.connect(pairedHost);
expect(client.status()).toBe("connected");
server.closeWithoutWebSocketClose();
client.notifyForeground();
expect(client.status()).toBe("reconnecting");
```

Assert the client verifies the server signature against `hostPublicKey` before it emits `PairComplete` or `Authenticate`; never store an offer secret after pairing; a `grant_revoked` frame deletes that host atomically; a host with `minimum_compatible_mobile_version > 1` reaches `incompatible` without dispatching domain RPC.

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `pnpm --dir mobile test -- src/storage/paired-host-store.test.ts src/transport/mobile-rpc-client.test.ts`

Expected: FAIL because the secure-store adapter and client do not exist.

- [ ] **Step 3: Implement key-pinned handshake and recovery**

Generate a TweetNaCl signing keypair on the phone during pairing and encode the public key/device proof exactly as the Rust `ClientFrame` expects. On each server challenge, verify the Ed25519 signature against the public key carried in the QR or persisted profile before sending either a pairing secret or token. Persist only the resulting `PairedHost` with Expo SecureStore; keep raw pairing offers and transient nonces only in memory.

Set a heartbeat timer while connected. When the WebSocket stops receiving responses, move through `reconnecting` to `unreachable`. Register `AppState` and Expo Network listeners: foreground or a restored network cancels exhausted backoff, probes immediately, and opens a new socket. `retry()` uses the same reset path. Never replay a request with an `operationId` until a later operation-specific client can determine its result; foundation has no mutating calls.

- [ ] **Step 4: Implement scan, paste, deep-link, and host dashboard screens**

`app/pair.tsx` requests camera permission only after the user chooses scan, offers a paste alternative when permission is denied, prevents duplicate scans during an active pairing attempt, and routes all values through `decodePairingUrl`. Register `expo-linking` so a `zed-mobile://pair?code=...` link opens the same screen and consumes the same parser.

`app/index.tsx` loads paired hosts, shows host display name/endpoint/connection state/protocol block reason, supports `Pair host`, `Retry`, and `Remove`, and renders an empty state. It contains no fake worktree/thread/account cards; those arrive only when their server capabilities exist.

```bash
pnpm --dir mobile test -- src/storage/paired-host-store.test.ts src/transport/mobile-rpc-client.test.ts src/features/pairing/pairing-code.test.ts app/index.test.tsx app/pair.test.tsx
pnpm --dir mobile typecheck
pnpm --dir mobile lint
```

Expected: PASS. Tests show QR/paste/deep-link convergence, no duplicate pairing, cryptographic pinning order, storage deletion on revocation, version block, explicit reconnect, and dashboard state rendering.

- [ ] **Step 6: Commit the operational pairing client**

```bash
git add mobile/vitest.config.ts mobile/src/storage mobile/src/transport mobile/src/features/pairing mobile/app
git commit -m "Add mobile host pairing"
```

### Task 7: Enforce mobile checks in generated CI and validate a real tailnet flow

**Files:**
- Modify: `tooling/xtask/src/tasks/workflows/run_tests.rs`
- Modify: `.github/workflows/run_tests.yml`
- Modify: `mobile/README.md`
- Test: `tooling/xtask/src/tasks/workflow_checks.rs`

**Interfaces:**
- Consumes: isolated `mobile/pnpm-lock.yaml`, Node 24, pnpm 9, and generated-workflow conventions.
- Produces: CI that runs only mobile dependency install/test/typecheck/lint when `mobile/**` or its workflow source changes; documented real-device acceptance evidence.

- [ ] **Step 1: Write the failing generated-workflow assertion**

Extend the workflow-check fixture/assertion so a generated `run_tests.yml` must include a mobile job with Node 24, pnpm 9, cache dependency path `mobile/pnpm-lock.yaml`, and these commands:

```yaml
pnpm --dir mobile install --frozen-lockfile
pnpm --dir mobile test
pnpm --dir mobile typecheck
pnpm --dir mobile lint
```

The job must run when `mobile/**`, `tooling/xtask/src/tasks/workflows/run_tests.rs`, or the mobile CI configuration changes; it must contribute to the final required test gate.

- [ ] **Step 2: Run the workflow check to verify it fails**

Run: `cargo xtask check-workflows`

Expected: FAIL because the generated workflow has no mobile job.

- [ ] **Step 3: Add path-aware mobile CI and regenerate workflow YAML**

Modify only the workflow source in `tooling/xtask/src/tasks/workflows/run_tests.rs`. Add an independent mobile job rather than making Cargo nextest install JavaScript tooling. Configure Node 24 and pnpm 9 as existing Danger CI does, cache `mobile/pnpm-lock.yaml`, and make the final status gate depend on it only when the orchestrator says mobile changed. Regenerate `.github/workflows/run_tests.yml` with `cargo xtask workflows`; never hand-edit generated YAML.

- [ ] **Step 4: Run focused workflow and mobile checks**

Run:

```bash
cargo xtask check-workflows
pnpm --dir mobile test
pnpm --dir mobile typecheck
pnpm --dir mobile lint
```

Expected: PASS. The workflow source and generated YAML agree, and mobile checks run locally.

- [ ] **Step 5: Perform the real-device Tailscale smoke test**

On a non-production test host and a phone already authenticated to the same tailnet:

1. Enable Mobile in Zed, select its `100.x.x.x` or `fd7a:115c:a1e0::/48` address, and keep port `6769`.
2. Generate the QR and scan it in a development build of Zed Mobile. Confirm one host appears as `connected` and shows version 1 with only the status capability.
3. Background the phone, disable and re-enable its Tailscale route, return to foreground, and confirm the dashboard reconnects or visibly reaches `unreachable` with working `Retry`.
4. Revoke the phone from the Mobile control window. Confirm the phone becomes `revoked`, removes its secure profile, and cannot reconnect.
5. Generate a second offer, pair again, close and reopen Zed without changing binding/port, and confirm the app reconnects to the stable endpoint.

Record results in the pull request description; never capture the QR, URL, token, key, prompt, or terminal contents in the report.

- [ ] **Step 6: Commit CI and the verified foundation**

```bash
git add tooling/xtask/src/tasks/workflows/run_tests.rs .github/workflows/run_tests.yml mobile/README.md
git commit -m "Test mobile companion foundation"
```

## Plan self-review

- **Spec coverage:** Tasks 1–4 cover pairing QR/Tailscale, stable binding, device grants/revocation, server-key pinning, version compatibility, secure credential storage, and the desktop Mobile control window. Tasks 5–7 cover the Expo app, QR/paste/deep-link pairing, secure host persistence, foreground/network recovery, licensing, CI, and real-device acceptance. Thread, terminal, worktree/Git, account, browser, attachment, notification, and accessibility requirements are assigned to named follow-on plans rather than silently omitted.
- **Placeholder scan:** Every task defines concrete interfaces, state transitions, source boundaries, tests, focused commands, and commit scope. No step delegates unspecified error handling or test coverage.
- **Type consistency:** Rust and TypeScript use `PairingOffer`, `Capability`, `ClientFrame`, `ServerFrame`, `Status`, `DeviceGrant`, `PairedHost`, `MobileBinding`, and `MobileServerStatus` consistently. Network and app code exchange only owned DTOs.
- **Upstream boundary:** The foundation creates its protocol/server/window/mobile modules and changes no existing UI/domain crate. The only runtime integration is one initializer in `zed::main`; Cargo and workflow edits are registry/build metadata. The four follow-on plans must retain this rule by adding only tested, additive adapter seams where the established public APIs are insufficient.
