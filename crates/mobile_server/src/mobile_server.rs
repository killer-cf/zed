pub mod mobile_store;
pub use mobile_store::{
    token_digest, token_matches, DeviceGrant, MobileBinding, MobileStore, MOBILE_CREDENTIALS_URL,
    MOBILE_HOST_KEY_USERNAME,
};

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::{IpAddr, SocketAddr},
    num::NonZeroU16,
    sync::{Arc, Mutex, MutexGuard},
    thread,
};

use anyhow::{Context as _, Result, anyhow, ensure};
use axum::{
    extract::{State, WebSocketUpgrade, ws::{Message, WebSocket}},
    response::IntoResponse,
    routing::get,
    Router,
};
use base64::{Engine as _, engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD}};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use futures::{
    channel::mpsc::UnboundedSender,
    future::{select, Either},
    StreamExt,
};
use gpui::{App, BorrowAppContext as _, Global, SharedString, Task};
use if_addrs::get_if_addrs;
use mobile_protocol::{
    authentication_payload, pairing_payload, Capability, ClientFrame, PairingOffer, ServerFrame,
    Status, MIN_COMPATIBLE_MOBILE_VERSION, MOBILE_PROTOCOL_VERSION,
};
use qrcodegen::{QrCode, QrCodeEcc};
use rand_core::{OsRng, RngCore};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq;
use time::{Duration, OffsetDateTime};
use tokio::sync::{oneshot, watch};
use uuid::Uuid;
use zeroize::Zeroizing;

const OFFER_LIFETIME: Duration = Duration::minutes(5);
const MAX_WEBSOCKET_FRAME_BYTES: usize = 64 * 1024;
const TAILSCALE_IPV4_NETWORK: u32 = u32::from_be_bytes([100, 64, 0, 0]);
const TAILSCALE_IPV4_MASK: u32 = u32::from_be_bytes([255, 192, 0, 0]);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MobileServerStatus {
    Disabled,
    Starting,
    Listening {
        endpoint: String,
        connected_devices: usize,
    },
    Error { message: SharedString },
}

pub struct MobileServer {
    store: Arc<MobileStore>,
    context: Arc<ServerContext>,
    grants: Vec<DeviceGrant>,
    _grant_update_task: Task<()>,
}

impl Global for MobileServer {}

impl MobileServer {
    pub fn init(store: MobileStore, host_name: SharedString, cx: &mut App) {
        let store = Arc::new(store);
        let (grant_updates, mut grant_updates_rx) = futures::channel::mpsc::unbounded();
        let context = Arc::new(ServerContext::new(
            host_name.to_string(),
            store.clone(),
            grant_updates,
        ));
        let grant_update_task = cx.spawn(async move |async_cx| {
            while let Some(update) = grant_updates_rx.next().await {
                async_cx.update(|app| {
                    if !app.has_global::<MobileServer>() {
                        return;
                    }
                    app.update_global::<MobileServer, _>(|server, _| match update {
                        GrantUpdate::Insert(grant) => {
                            if server.grants.iter().all(|existing| existing.id != grant.id) {
                                server.grants.push(grant);
                            }
                        }
                        GrantUpdate::Revoke { id, at } => {
                            if let Some(grant) = server.grants.iter_mut().find(|grant| grant.id == id)
                            {
                                grant.revoked_at = Some(at);
                            }
                        }
                    });
                });
            }
        });

        cx.set_global(Self {
            store,
            context,
            grants: Vec::new(),
            _grant_update_task: grant_update_task,
        });
    }

    pub fn enable(&mut self, binding: MobileBinding, cx: &mut App) -> Task<Result<()>> {
        if let Err(error) = validate_listener_binding(&binding) {
            set_error(&self.context, "mobile binding is not a Tailscale address");
            return Task::ready(Err(error));
        }

        {
            let mut state = lock_state(&self.context);
            if state.shutdown_tx.is_some() {
                let error = anyhow!("mobile server is already enabled");
                state.status = MobileServerStatus::Error {
                    message: SharedString::from("mobile server is already enabled"),
                };
                return Task::ready(Err(error));
            }
            state.endpoint = Some(endpoint_for(binding.address, binding.port));
            state.status = MobileServerStatus::Starting;
            state.host_signing_key = None;
            state.offers.clear();
            state.consumed_offers.clear();
            state.expired_offers.clear();
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (done_tx, done_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        {
            let mut state = lock_state(&self.context);
            state.shutdown_tx = Some(shutdown_tx);
            state.done_rx = Some(done_rx);
        }

        let store = self.store.clone();
        let context = self.context.clone();
        let endpoint_binding = binding.clone();
        cx.spawn(async move |async_cx| {
            if let Err(error) = store.load_binding().await.and_then(|stored| {
                if let Some(stored) = stored {
                    validate_listener_binding(&stored)?;
                }
                Ok(())
            }) {
                let message = "failed to load mobile server storage".to_owned();
                set_error(&context, &message);
                send_done(done_tx, Err(message));
                return Err(error);
            }

            let host_signing_key = match store.load_or_create_host_key(&async_cx).await {
                Ok(key) => key,
                Err(error) => {
                    let message = "failed to load mobile host identity".to_owned();
                    set_error(&context, &message);
                    send_done(done_tx, Err(message));
                    return Err(error);
                }
            };

            let grants = match store.grants().await {
                Ok(grants) => grants,
                Err(error) => {
                    let message = "failed to load mobile server grants".to_owned();
                    set_error(&context, &message);
                    send_done(done_tx, Err(message));
                    return Err(error);
                }
            };

            let mut persisted_binding = endpoint_binding.clone();
            persisted_binding.enabled = true;
            if let Err(error) = store.save_binding(persisted_binding).await {
                let message = "failed to persist mobile server binding".to_owned();
                set_error(&context, &message);
                send_done(done_tx, Err(message));
                return Err(error);
            }

            {
                let mut state = lock_state(&context);
                state.host_signing_key = Some(Arc::new(host_signing_key));
            }

            let worker_context = context.clone();
            let worker = thread::Builder::new()
                .name("zed-mobile-server".to_owned())
                .spawn(move || {
                    run_listener_thread(
                        worker_context,
                        endpoint_binding,
                        ready_tx,
                        done_tx,
                        shutdown_rx,
                    );
                });
            if let Err(error) = worker {
                let message = "failed to start mobile server listener".to_owned();
                set_error(&context, &message);
                return Err(anyhow!(error).context(message));
            }

            let startup = match ready_rx.await {
                Ok(result) => result,
                Err(_) => Err("mobile server listener stopped during startup".to_owned()),
            };
            match startup {
                Ok(()) => {
                    {
                        let mut state = lock_state(&context);
                        if let Some(endpoint) = state.endpoint.clone() {
                            state.status = MobileServerStatus::Listening {
                                endpoint,
                                connected_devices: state.active_connections.values().map(HashMap::len).sum(),
                            };
                        }
                    }
                    async_cx.update(|app| {
                        if app.has_global::<MobileServer>() {
                            app.update_global::<MobileServer, _>(|server, _| {
                                server.grants = grants;
                            });
                        }
                    });
                    Ok(())
                }
                Err(message) => {
                    set_error(&context, &message);
                    Err(anyhow!(message))
                }
            }
        })
    }

    pub fn disable(&mut self, cx: &mut App) -> Task<Result<()>> {
        let (shutdown_tx, done_rx) = {
            let mut state = lock_state(&self.context);
            (state.shutdown_tx.take(), state.done_rx.take())
        };
        let Some(done_rx) = done_rx else {
            clear_runtime(&self.context);
            return Task::ready(Ok(()));
        };
        let context = self.context.clone();
        cx.spawn(async move |_async_cx| {
            if let Some(shutdown_tx) = shutdown_tx {
                match shutdown_tx.send(()) {
                    Ok(()) => {}
                    Err(()) => {}
                }
            }
            let result = match done_rx.await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(message)) => Err(anyhow!(message)),
                Err(_) => Err(anyhow!("mobile server listener stopped unexpectedly")),
            };
            clear_runtime(&context);
            result
        })
    }

    pub fn status(&self) -> MobileServerStatus {
        lock_state(&self.context).status.clone()
    }

    pub fn create_pairing_offer(&mut self, now: OffsetDateTime) -> Result<PairingOffer> {
        let mut state = lock_state(&self.context);
        ensure!(
            matches!(state.status, MobileServerStatus::Listening { .. }),
            "mobile server is not listening"
        );
        let endpoint = state
            .endpoint
            .clone()
            .ok_or_else(|| anyhow!("mobile server has no endpoint"))?;
        let host_signing_key = state
            .host_signing_key
            .clone()
            .ok_or_else(|| anyhow!("mobile server host identity is unavailable"))?;

        prune_offers(&mut state, now);
        let previous_offer_ids: Vec<Uuid> = state.offers.keys().copied().collect();
        for offer_id in previous_offer_ids {
            state.offers.remove(&offer_id);
            state.consumed_offers.insert(offer_id);
        }

        let offer_id = Uuid::new_v4();
        let pairing_secret = random_base64_token();
        let secret_digest = Sha256::digest(pairing_secret.as_bytes());
        let mut secret_digest_bytes = [0; 32];
        secret_digest_bytes.copy_from_slice(&secret_digest);
        let offer = PairingOffer {
            offer_id,
            endpoint,
            protocol_version: MOBILE_PROTOCOL_VERSION,
            host_public_key: URL_SAFE_NO_PAD.encode(host_signing_key.verifying_key().to_bytes()),
            pairing_secret,
            expires_at: now + OFFER_LIFETIME,
        };
        state.offers.insert(
            offer_id,
            StoredOffer {
                offer: offer.clone(),
                secret_digest: secret_digest_bytes,
            },
        );
        Ok(offer)
    }

    pub fn grants(&self) -> &[DeviceGrant] {
        &self.grants
    }

    pub fn revoke_grant(&mut self, id: Uuid, cx: &mut App) -> Task<Result<bool>> {
        let store = self.store.clone();
        let context = self.context.clone();
        cx.spawn(async move |async_cx| {
            let changed = store.revoke_grant(id, OffsetDateTime::now_utc()).await?;
            if changed {
                let at = store
                    .grant(id)
                    .await?
                    .and_then(|grant| grant.revoked_at)
                    .unwrap_or_else(OffsetDateTime::now_utc);
                close_connections_for_grant(&context, id);
                emit_grant_update(&context, GrantUpdate::Revoke { id, at });
                async_cx.update(|app| {
                    if app.has_global::<MobileServer>() {
                        app.update_global::<MobileServer, _>(|server, _| {
                            if let Some(grant) = server.grants.iter_mut().find(|grant| grant.id == id) {
                                grant.revoked_at = Some(at);
                            }
                        });
                    }
                });
            }
            Ok(changed)
        })
    }
}

impl Drop for MobileServer {
    fn drop(&mut self) {
        let shutdown_tx = lock_state(&self.context).shutdown_tx.take();
        if let Some(shutdown_tx) = shutdown_tx {
            match shutdown_tx.send(()) {
                Ok(()) => {}
                Err(()) => {}
            }
        }
        close_all_connections(&self.context);
    }
}

pub fn tailscale_addresses() -> Result<Vec<IpAddr>> {
    let mut addresses = get_if_addrs()
        .context("enumerating network interfaces")?
        .into_iter()
        .filter(|interface| interface.is_oper_up())
        .map(|interface| interface.ip())
        .filter(|address| is_tailscale_address(*address))
        .collect::<Vec<_>>();
    addresses.sort();
    addresses.dedup();
    Ok(addresses)
}

pub fn validate_mobile_binding(address: &str, port: u16) -> Result<MobileBinding> {
    let address = address.parse::<IpAddr>().context("invalid mobile binding address")?;
    let port = NonZeroU16::new(port).ok_or_else(|| anyhow!("mobile binding port must be nonzero"))?;
    ensure!(
        is_tailscale_address(address),
        "mobile binding address must be a Tailscale address"
    );
    Ok(MobileBinding {
        enabled: true,
        address,
        port,
    })
}

pub fn pairing_offer_svg(offer: &PairingOffer) -> Result<String> {
    let pairing_url = offer.encode_url().context("encoding pairing offer URL")?;
    let qr_code = QrCode::encode_text(&pairing_url, QrCodeEcc::Medium)
        .map_err(|_| anyhow!("pairing offer is too large for a QR code"))?;
    let border = 4;
    let size = qr_code.size();
    let dimension = size + border * 2;
    let mut path = String::new();
    for y in 0..size {
        for x in 0..size {
            if qr_code.get_module(x, y) {
                path.push_str(&format!("M{} {}h1v1h-1z", x + border, y + border));
            }
        }
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dimension} {dimension}\" role=\"img\"><path fill=\"#fff\" d=\"M0 0h{dimension}v{dimension}H0z\"/><path fill=\"#000\" d=\"{path}\"/></svg>"
    ))
}

fn validate_binding(binding: &MobileBinding) -> Result<()> {
    ensure!(binding.port.get() != 0, "mobile binding port must be nonzero");
    ensure!(
        is_tailscale_address(binding.address),
        "mobile binding address must be a Tailscale address"
    );
    Ok(())
}

fn validate_listener_binding(binding: &MobileBinding) -> Result<()> {
    #[cfg(test)]
    if binding.address.is_loopback() {
        ensure!(binding.port.get() != 0, "mobile binding port must be nonzero");
        return Ok(());
    }
    validate_binding(binding)
}

fn is_tailscale_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            u32::from(address) & TAILSCALE_IPV4_MASK == TAILSCALE_IPV4_NETWORK
        }
        IpAddr::V6(address) => {
            let segments = address.segments();
            segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
        }
    }
}

fn endpoint_for(address: IpAddr, port: NonZeroU16) -> String {
    match address {
        IpAddr::V4(address) => format!("ws://{address}:{port}"),
        IpAddr::V6(address) => format!("ws://[{address}]:{port}"),
    }
}

#[derive(Clone)]
struct StoredOffer {
    offer: PairingOffer,
    secret_digest: [u8; 32],
}

#[derive(Clone, Debug)]
struct PendingPair {
    offer_id: Uuid,
    protocol_version: u16,
    client_nonce: String,
    server_nonce: String,
}

#[derive(Debug)]
struct PendingAuthentication {
    grant_id: Uuid,
    client_nonce: String,
    server_nonce: String,
    token_digest: [u8; 32],
}

enum SessionPhase {
    Unpaired,
    Pairing(PendingPair),
    Authenticating(PendingAuthentication),
    Authenticated { grant_id: Uuid },
}

struct ActiveConnection {
    close_tx: watch::Sender<bool>,
}

enum GrantUpdate {
    Insert(DeviceGrant),
    Revoke { id: Uuid, at: OffsetDateTime },
}
struct ServerContext {
    inner: Mutex<ServerContextInner>,
}

struct ServerContextInner {
    store: Arc<MobileStore>,
    host_name: String,
    host_signing_key: Option<Arc<SigningKey>>,
    endpoint: Option<String>,
    status: MobileServerStatus,
    offers: BTreeMap<Uuid, StoredOffer>,
    consumed_offers: HashSet<Uuid>,
    expired_offers: HashSet<Uuid>,
    active_connections: HashMap<Uuid, HashMap<Uuid, ActiveConnection>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    done_rx: Option<oneshot::Receiver<std::result::Result<(), String>>>,
    grant_updates: UnboundedSender<GrantUpdate>,
}

impl ServerContext {
    fn new(
        host_name: String,
        store: Arc<MobileStore>,
        grant_updates: UnboundedSender<GrantUpdate>,
    ) -> Self {
        Self {
            inner: Mutex::new(ServerContextInner {
                store,
                host_name,
                host_signing_key: None,
                endpoint: None,
                status: MobileServerStatus::Disabled,
                offers: BTreeMap::new(),
                consumed_offers: HashSet::new(),
                expired_offers: HashSet::new(),
                active_connections: HashMap::new(),
                shutdown_tx: None,
                done_rx: None,
                grant_updates,
            }),
        }
    }
}

fn lock_state(context: &ServerContext) -> MutexGuard<'_, ServerContextInner> {
    match context.inner.lock() {
        Ok(state) => state,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn set_error(context: &ServerContext, message: &str) {
    lock_state(context).status = MobileServerStatus::Error {
        message: SharedString::from(message.to_owned()),
    };
}

fn clear_runtime(context: &ServerContext) {
    let mut state = lock_state(context);
    state.shutdown_tx = None;
    state.done_rx = None;
    state.host_signing_key = None;
    state.endpoint = None;
    state.offers.clear();
    state.consumed_offers.clear();
    state.expired_offers.clear();
    state.active_connections.clear();
    state.status = MobileServerStatus::Disabled;
}

fn send_done(
    sender: oneshot::Sender<std::result::Result<(), String>>,
    result: std::result::Result<(), String>,
) {
    match sender.send(result) {
        Ok(()) => {}
        Err(_) => {}
    }
}

fn run_listener_thread(
    context: Arc<ServerContext>,
    binding: MobileBinding,
    ready_tx: oneshot::Sender<std::result::Result<(), String>>,
    done_tx: oneshot::Sender<std::result::Result<(), String>>,
    shutdown_rx: oneshot::Receiver<()>,
) {
    let result = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(run_listener(context.clone(), binding, ready_tx, shutdown_rx)),
        Err(error) => Err(format!("failed to initialize mobile listener runtime: {error}")),
    };
    if let Err(message) = &result {
        set_error(&context, message);
    }
    send_done(done_tx, result);
}

async fn run_listener(
    context: Arc<ServerContext>,
    binding: MobileBinding,
    ready_tx: oneshot::Sender<std::result::Result<(), String>>,
    shutdown_rx: oneshot::Receiver<()>,
) -> std::result::Result<(), String> {
    let socket_address = SocketAddr::new(binding.address, binding.port.get());
    let listener = match tokio::net::TcpListener::bind(socket_address).await {
        Ok(listener) => listener,
        Err(error) => {
            let message = format!("failed to bind mobile listener: {error}");
            match ready_tx.send(Err(message.clone())) {
                Ok(()) => {}
                Err(_) => {}
            }
            return Err(message);
        }
    };
    let listener = match listener.into_std() {
        Ok(listener) => listener,
        Err(error) => {
            let message = format!("failed to configure mobile listener: {error}");
            match ready_tx.send(Err(message.clone())) {
                Ok(()) => {}
                Err(_) => {}
            }
            return Err(message);
        }
    };

    let application = Router::new()
        .route("/", get(websocket_upgrade))
        .with_state(context.clone());
    let server = match axum::Server::from_tcp(listener) {
        Ok(server) => server.serve(application.into_make_service()),
        Err(error) => {
            let message = format!("failed to initialize mobile HTTP server: {error}");
            match ready_tx.send(Err(message.clone())) {
                Ok(()) => {}
                Err(_) => {}
            }
            return Err(message);
        }
    };
    match ready_tx.send(Ok(())) {
        Ok(()) => {}
        Err(_) => return Err("mobile server startup was cancelled".to_owned()),
    }

    let graceful_shutdown = async move {
        match shutdown_rx.await {
            Ok(()) => {}
            Err(_) => {}
        }
        close_all_connections(&context);
    };
    match server.with_graceful_shutdown(graceful_shutdown).await {
        Ok(()) => Ok(()),
        Err(error) => Err(format!("mobile HTTP server failed: {error}")),
    }
}

async fn websocket_upgrade(
    ws: WebSocketUpgrade,
    State(context): State<Arc<ServerContext>>,
) -> impl IntoResponse {
    ws.max_message_size(MAX_WEBSOCKET_FRAME_BYTES)
        .max_frame_size(MAX_WEBSOCKET_FRAME_BYTES)
        .on_upgrade(move |socket| handle_socket(socket, context))
}

async fn handle_socket(mut socket: WebSocket, context: Arc<ServerContext>) {
    let store = context_store(&context);
    let mut phase = SessionPhase::Unpaired;
    let mut active_connection: Option<(Uuid, watch::Receiver<bool>)> = None;

    loop {
        let next_message = match active_connection.as_mut() {
            Some((_, close_rx)) => {
                let selected = select(
                    Box::pin(close_rx.changed()),
                    Box::pin(socket.next()),
                )
                .await;
                match selected {
                    Either::Left((signal_result, remaining_message)) => {
                        drop(remaining_message);
                        match signal_result {
                            Ok(()) | Err(_) => break,
                        }
                    }
                    Either::Right((message, remaining_signal)) => {
                        drop(remaining_signal);
                        message
                    }
                }
            }
            None => socket.next().await,
        };
        let Some(message) = next_message else {
            break;
        };
        let Ok(message) = message else {
            send_close(&mut socket).await;
            break;
        };
        match message {
            Message::Text(text) => {
                if text.len() > MAX_WEBSOCKET_FRAME_BYTES {
                    send_close(&mut socket).await;
                    break;
                }
                let frame = match ClientFrame::from_json(&text) {
                    Ok(frame) => frame,
                    Err(_) => {
                        send_failure_and_close(
                            &mut socket,
                            ProtocolFailure {
                                code: "invalid_frame",
                                message: "invalid client frame",
                            },
                            None,
                        )
                        .await;
                        break;
                    }
                };
                match &mut phase {
                    SessionPhase::Unpaired => match frame {
                        ClientFrame::PairBegin {
                            offer_id,
                            client_nonce,
                        } => match begin_pairing(&context, offer_id, client_nonce).await {
                            Ok((pending, challenge)) => {
                                if !send_frame(&mut socket, challenge).await {
                                    break;
                                }
                                phase = SessionPhase::Pairing(pending);
                            }
                            Err(failure) => {
                                send_failure_and_close(&mut socket, failure, None).await;
                                break;
                            }
                        },
                        ClientFrame::Connect {
                            grant_id,
                            client_nonce,
                        } => match begin_authentication(&context, &store, grant_id, client_nonce).await {
                            Ok((pending, challenge)) => {
                                if !send_frame(&mut socket, challenge).await {
                                    break;
                                }
                                phase = SessionPhase::Authenticating(pending);
                            }
                            Err(failure) => {
                                send_failure_and_close(&mut socket, failure, None).await;
                                break;
                            }
                        },
                        ClientFrame::Request { request_id, .. } => {
                            send_failure_and_close(
                                &mut socket,
                                ProtocolFailure {
                                    code: "not_authenticated",
                                    message: "authenticate before making requests",
                                },
                                Some(request_id),
                            )
                            .await;
                            break;
                        }
                        ClientFrame::Ping { .. }
                        | ClientFrame::PairComplete { .. }
                        | ClientFrame::Authenticate { .. } => {
                            send_failure_and_close(
                                &mut socket,
                                ProtocolFailure {
                                    code: "unexpected_transition",
                                    message: "unexpected client frame",
                                },
                                None,
                            )
                            .await;
                            break;
                        }
                    },
                    SessionPhase::Pairing(pending) => match frame {
                        ClientFrame::PairComplete {
                            offer_id,
                            pairing_secret,
                            client_public_key,
                            client_proof,
                            device_label,
                        } if offer_id == pending.offer_id => {
                            let pending_pair = PendingPair {
                                offer_id: pending.offer_id,
                                protocol_version: pending.protocol_version,
                                client_nonce: pending.client_nonce.clone(),
                                server_nonce: pending.server_nonce.clone(),
                            };
                            match complete_pairing(
                                &context,
                                &store,
                                pending_pair,
                                pairing_secret,
                                client_public_key,
                                client_proof,
                                device_label,
                            )
                            .await
                            {
                                Ok((grant, token)) => {
                                    let (connection_id, close_rx) =
                                        register_connection(&context, grant.id);
                                    active_connection = Some((connection_id, close_rx));
                                    phase = SessionPhase::Authenticated { grant_id: grant.id };
                                    if !send_frame(
                                        &mut socket,
                                        ServerFrame::PairComplete {
                                            grant_id: grant.id,
                                            token,
                                        },
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                    if !send_frame(
                                        &mut socket,
                                        ServerFrame::Authenticated { grant_id: grant.id },
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                }
                                Err(failure) => {
                                    send_failure_and_close(&mut socket, failure, None).await;
                                    break;
                                }
                            }
                        }
                        ClientFrame::PairComplete { .. } => {
                            send_failure_and_close(
                                &mut socket,
                                ProtocolFailure {
                                    code: "unexpected_transition",
                                    message: "pairing offer does not match the challenge",
                                },
                                None,
                            )
                            .await;
                            break;
                        }
                        _ => {
                            send_failure_and_close(
                                &mut socket,
                                ProtocolFailure {
                                    code: "unexpected_transition",
                                    message: "complete pairing before sending another frame",
                                },
                                None,
                            )
                            .await;
                            break;
                        }
                    },
                    SessionPhase::Authenticating(pending) => match frame {
                        ClientFrame::Authenticate {
                            grant_id,
                            token,
                            client_proof,
                        } if grant_id == pending.grant_id => {
                            match authenticate(
                                &context,
                                &store,
                                pending,
                                token,
                                client_proof,
                            )
                            .await
                            {
                                Ok(()) => {
                                    let (connection_id, close_rx) =
                                        register_connection(&context, grant_id);
                                    active_connection = Some((connection_id, close_rx));
                                    phase = SessionPhase::Authenticated { grant_id };
                                    if !send_frame(
                                        &mut socket,
                                        ServerFrame::Authenticated { grant_id },
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                }
                                Err(failure) => {
                                    send_failure_and_close(&mut socket, failure, None).await;
                                    break;
                                }
                            }
                        }
                        ClientFrame::Authenticate { .. } => {
                            send_failure_and_close(
                                &mut socket,
                                ProtocolFailure {
                                    code: "unexpected_transition",
                                    message: "authentication grant does not match the challenge",
                                },
                                None,
                            )
                            .await;
                            break;
                        }
                        _ => {
                            send_failure_and_close(
                                &mut socket,
                                ProtocolFailure {
                                    code: "unexpected_transition",
                                    message: "authenticate before sending another frame",
                                },
                                None,
                            )
                            .await;
                            break;
                        }
                    },
                    SessionPhase::Authenticated { grant_id } => match frame {
                        ClientFrame::Request {
                            request_id,
                            operation_id,
                            method,
                            params,
                        } => {
                            if !handle_request(
                                &mut socket,
                                &store,
                                &context,
                                *grant_id,
                                request_id,
                                operation_id,
                                method,
                                params,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        ClientFrame::Ping { nonce } => {
                            if !send_frame(&mut socket, ServerFrame::Pong { nonce }).await {
                                break;
                            }
                        }
                        _ => {
                            send_failure_and_close(
                                &mut socket,
                                ProtocolFailure {
                                    code: "unexpected_transition",
                                    message: "unexpected client frame",
                                },
                                None,
                            )
                            .await;
                            break;
                        }
                    },
                }
            }
            Message::Ping(payload) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    break;
                }
            }
            Message::Pong(_) => {}
            Message::Binary(_) | Message::Close(_) => {
                send_close(&mut socket).await;
                break;
            }
        }
    }
    if let Some((connection_id, _)) = active_connection {
        unregister_connection(&context, connection_id);
    }
}

fn context_store(context: &ServerContext) -> Arc<MobileStore> {
    lock_state(context).store.clone()
}

#[derive(Clone, Copy, Debug)]
struct ProtocolFailure {
    code: &'static str,
    message: &'static str,
}

async fn begin_pairing(
    context: &ServerContext,
    offer_id: Uuid,
    client_nonce: String,
) -> std::result::Result<(PendingPair, ServerFrame), ProtocolFailure> {
    let now = OffsetDateTime::now_utc();
    let (offer, host_signing_key) = {
        let mut state = lock_state(context);
        prune_offers(&mut state, now);
        let Some(stored_offer) = state.offers.get(&offer_id).cloned() else {
            return Err(if state.consumed_offers.contains(&offer_id) {
                ProtocolFailure {
                    code: "offer_consumed",
                    message: "pairing offer has already been consumed",
                }
            } else if state.expired_offers.contains(&offer_id) {
                ProtocolFailure {
                    code: "offer_expired",
                    message: "pairing offer has expired",
                }
            } else {
                ProtocolFailure {
                    code: "offer_not_found",
                    message: "pairing offer was not found",
                }
            });
        };
        let Some(host_signing_key) = state.host_signing_key.clone() else {
            return Err(ProtocolFailure {
                code: "server_unavailable",
                message: "mobile server host identity is unavailable",
            });
        };
        (stored_offer.offer, host_signing_key)
    };
    if offer.protocol_version != MOBILE_PROTOCOL_VERSION {
        return Err(ProtocolFailure {
            code: "unsupported_protocol",
            message: "pairing offer uses an unsupported protocol version",
        });
    }
    let server_nonce = random_base64_token();
    let payload = pairing_payload(
        offer.protocol_version,
        offer.offer_id,
        &client_nonce,
        &server_nonce,
    )
    .map_err(|_| ProtocolFailure {
        code: "invalid_nonce",
        message: "pairing nonce is invalid",
    })?;
    let host_signature = host_signing_key.sign(&payload);
    let pending = PendingPair {
        offer_id: offer.offer_id,
        protocol_version: offer.protocol_version,
        client_nonce: client_nonce.clone(),
        server_nonce: server_nonce.clone(),
    };
    Ok((
        pending,
        ServerFrame::PairChallenge {
            offer_id: offer.offer_id,
            client_nonce,
            server_nonce,
            host_signature: URL_SAFE_NO_PAD.encode(host_signature.to_bytes()),
        },
    ))
}

async fn complete_pairing(
    context: &ServerContext,
    store: &Arc<MobileStore>,
    pending: PendingPair,
    pairing_secret: String,
    client_public_key: String,
    client_proof: String,
    device_label: String,
) -> std::result::Result<(DeviceGrant, String), ProtocolFailure> {
    let now = OffsetDateTime::now_utc();
    let stored_offer = {
        let mut state = lock_state(context);
        prune_offers(&mut state, now);
        let Some(stored_offer) = state.offers.get(&pending.offer_id).cloned() else {
            return Err(if state.consumed_offers.contains(&pending.offer_id) {
                ProtocolFailure {
                    code: "offer_consumed",
                    message: "pairing offer has already been consumed",
                }
            } else if state.expired_offers.contains(&pending.offer_id) {
                ProtocolFailure {
                    code: "offer_expired",
                    message: "pairing offer has expired",
                }
            } else {
                ProtocolFailure {
                    code: "offer_not_found",
                    message: "pairing offer was not found",
                }
            });
        };
        stored_offer
    };
    if now >= stored_offer.offer.expires_at {
        return Err(ProtocolFailure {
            code: "offer_expired",
            message: "pairing offer has expired",
        });
    }
    let pairing_digest = Sha256::digest(pairing_secret.as_bytes());
    if !bool::from(pairing_digest.as_slice().ct_eq(&stored_offer.secret_digest)) {
        return Err(ProtocolFailure {
            code: "invalid_pairing_secret",
            message: "pairing secret is invalid",
        });
    }
    if device_label.is_empty() || device_label.len() > 128 {
        return Err(ProtocolFailure {
            code: "invalid_device_label",
            message: "device label must be between 1 and 128 bytes",
        });
    }
    let client_key_bytes = decode_fixed(&client_public_key, 32).map_err(|_| ProtocolFailure {
        code: "invalid_client_key",
        message: "client public key is invalid",
    })?;
    let client_key_array = array_32(&client_key_bytes);
    let client_key = VerifyingKey::from_bytes(&client_key_array).map_err(|_| ProtocolFailure {
        code: "invalid_client_key",
        message: "client public key is invalid",
    })?;
    let proof_bytes = decode_fixed(&client_proof, 64).map_err(|_| ProtocolFailure {
        code: "invalid_client_proof",
        message: "client proof is invalid",
    })?;
    let proof = Signature::from_slice(&proof_bytes).map_err(|_| ProtocolFailure {
        code: "invalid_client_proof",
        message: "client proof is invalid",
    })?;
    let payload = pairing_payload(
        pending.protocol_version,
        pending.offer_id,
        &pending.client_nonce,
        &pending.server_nonce,
    )
    .map_err(|_| ProtocolFailure {
        code: "invalid_nonce",
        message: "pairing nonce is invalid",
    })?;
    client_key.verify(&payload, &proof).map_err(|_| ProtocolFailure {
        code: "invalid_client_proof",
        message: "client proof is invalid",
    })?;

    {
        let mut state = lock_state(context);
        let Some(current_offer) = state.offers.remove(&pending.offer_id) else {
            return Err(ProtocolFailure {
                code: "offer_consumed",
                message: "pairing offer has already been consumed",
            });
        };
        if !bool::from(current_offer.secret_digest.ct_eq(&stored_offer.secret_digest)) {
            return Err(ProtocolFailure {
                code: "offer_consumed",
                message: "pairing offer has already been consumed",
            });
        }
        state.consumed_offers.insert(pending.offer_id);
    }

    let token = random_base64_token();
    let grant = DeviceGrant {
        id: Uuid::new_v4(),
        label: device_label,
        client_public_key: URL_SAFE_NO_PAD.encode(client_key.to_bytes()),
        token_digest: token_digest(token.as_bytes()),
        capabilities: [Capability::StatusRead].into_iter().collect(),
        created_at: now,
        revoked_at: None,
    };
    if store.insert_grant(grant.clone()).await.is_err() {
        return Err(ProtocolFailure {
            code: "storage_error",
            message: "mobile server storage is unavailable",
        });
    }
    emit_grant_update(context, GrantUpdate::Insert(grant.clone()));
    Ok((grant, token))
}

async fn begin_authentication(
    context: &ServerContext,
    store: &Arc<MobileStore>,
    grant_id: Uuid,
    client_nonce: String,
) -> std::result::Result<(PendingAuthentication, ServerFrame), ProtocolFailure> {
    let grant = match store.grant(grant_id).await {
        Ok(Some(grant)) => grant,
        Ok(None) => {
            return Err(ProtocolFailure {
                code: "grant_not_found",
                message: "device grant was not found",
            });
        }
        Err(_) => {
            return Err(ProtocolFailure {
                code: "storage_error",
                message: "mobile server storage is unavailable",
            });
        }
    };
    if grant.revoked_at.is_some() {
        return Err(ProtocolFailure {
            code: "grant_revoked",
            message: "device grant has been revoked",
        });
    }
    let token_digest = decode_digest(&grant.token_digest).map_err(|_| ProtocolFailure {
        code: "storage_error",
        message: "mobile server storage is unavailable",
    })?;
    let host_signing_key = lock_state(context)
        .host_signing_key
        .clone()
        .ok_or(ProtocolFailure {
            code: "server_unavailable",
            message: "mobile server host identity is unavailable",
        })?;
    let server_nonce = random_base64_token();
    let payload = authentication_payload(
        MOBILE_PROTOCOL_VERSION,
        grant_id,
        &client_nonce,
        &server_nonce,
        &token_digest,
    )
    .map_err(|_| ProtocolFailure {
        code: "invalid_nonce",
        message: "authentication nonce is invalid",
    })?;
    let host_signature = host_signing_key.sign(&payload);
    let pending = PendingAuthentication {
        grant_id,
        client_nonce: client_nonce.clone(),
        server_nonce: server_nonce.clone(),
        token_digest,
    };
    Ok((
        pending,
        ServerFrame::ServerChallenge {
            grant_id,
            client_nonce,
            server_nonce,
            host_signature: URL_SAFE_NO_PAD.encode(host_signature.to_bytes()),
        },
    ))
}

async fn authenticate(
    _context: &ServerContext,
    store: &Arc<MobileStore>,
    pending: &PendingAuthentication,
    token: String,
    client_proof: String,
) -> std::result::Result<(), ProtocolFailure> {
    let grant = match store.grant(pending.grant_id).await {
        Ok(Some(grant)) => grant,
        Ok(None) => {
            return Err(ProtocolFailure {
                code: "grant_not_found",
                message: "device grant was not found",
            });
        }
        Err(_) => {
            return Err(ProtocolFailure {
                code: "storage_error",
                message: "mobile server storage is unavailable",
            });
        }
    };
    if grant.revoked_at.is_some() {
        return Err(ProtocolFailure {
            code: "grant_revoked",
            message: "device grant has been revoked",
        });
    }
    if !token_matches(&grant.token_digest, token.as_bytes()) {
        return Err(ProtocolFailure {
            code: "invalid_token",
            message: "device token is invalid",
        });
    }
    let token_digest = decode_digest(&grant.token_digest).map_err(|_| ProtocolFailure {
        code: "storage_error",
        message: "mobile server storage is unavailable",
    })?;
    if token_digest != pending.token_digest {
        return Err(ProtocolFailure {
            code: "storage_error",
            message: "mobile server storage is unavailable",
        });
    }
    let public_key_bytes = decode_fixed(&grant.client_public_key, 32).map_err(|_| ProtocolFailure {
        code: "storage_error",
        message: "mobile server storage is unavailable",
    })?;
    let public_key = VerifyingKey::from_bytes(&array_32(&public_key_bytes)).map_err(|_| ProtocolFailure {
        code: "storage_error",
        message: "mobile server storage is unavailable",
    })?;
    let proof_bytes = decode_fixed(&client_proof, 64).map_err(|_| ProtocolFailure {
        code: "invalid_client_proof",
        message: "client proof is invalid",
    })?;
    let proof = Signature::from_slice(&proof_bytes).map_err(|_| ProtocolFailure {
        code: "invalid_client_proof",
        message: "client proof is invalid",
    })?;
    let payload = authentication_payload(
        MOBILE_PROTOCOL_VERSION,
        pending.grant_id,
        &pending.client_nonce,
        &pending.server_nonce,
        &token_digest,
    )
    .map_err(|_| ProtocolFailure {
        code: "invalid_nonce",
        message: "authentication nonce is invalid",
    })?;
    public_key.verify(&payload, &proof).map_err(|_| ProtocolFailure {
        code: "invalid_client_proof",
        message: "client proof is invalid",
    })
}

async fn handle_request(
    socket: &mut WebSocket,
    store: &Arc<MobileStore>,
    context: &ServerContext,
    grant_id: Uuid,
    request_id: Uuid,
    operation_id: Option<Uuid>,
    method: String,
    params: serde_json::Value,
) -> bool {
    let grant = match store.grant(grant_id).await {
        Ok(Some(grant)) => grant,
        Ok(None) => {
            send_failure_and_close(
                socket,
                ProtocolFailure {
                    code: "grant_not_found",
                    message: "device grant was not found",
                },
                Some(request_id),
            )
            .await;
            return false;
        }
        Err(_) => {
            send_failure_and_close(
                socket,
                ProtocolFailure {
                    code: "storage_error",
                    message: "mobile server storage is unavailable",
                },
                Some(request_id),
            )
            .await;
            return false;
        }
    };
    if grant.revoked_at.is_some() {
        send_failure_and_close(
            socket,
            ProtocolFailure {
                code: "grant_revoked",
                message: "device grant has been revoked",
            },
            Some(request_id),
        )
        .await;
        return false;
    }
    if method != "status.get" {
        return send_frame(
            socket,
            ServerFrame::Error {
                request_id: Some(request_id),
                code: "unsupported_method".to_owned(),
                message: "mobile status server does not support this method".to_owned(),
            },
        )
        .await;
    }
    if !params.is_object() {
        return send_frame(
            socket,
            ServerFrame::Error {
                request_id: Some(request_id),
                code: "invalid_params".to_owned(),
                message: "status.get params must be an object".to_owned(),
            },
        )
        .await;
    }
    let host_name = lock_state(context).host_name.clone();
    let status = Status::new(
        host_name,
        MOBILE_PROTOCOL_VERSION,
        MIN_COMPATIBLE_MOBILE_VERSION,
        vec![Capability::StatusRead],
    );
    let result = match serde_json::to_value(status) {
        Ok(result) => result,
        Err(_) => {
            send_failure_and_close(
                socket,
                ProtocolFailure {
                    code: "serialization_error",
                    message: "mobile status could not be serialized",
                },
                Some(request_id),
            )
            .await;
            return false;
        }
    };
    send_frame(
        socket,
        ServerFrame::Response {
            request_id,
            operation_id,
            result,
        },
    )
    .await
}

fn register_connection(context: &ServerContext, grant_id: Uuid) -> (Uuid, watch::Receiver<bool>) {
    let connection_id = Uuid::new_v4();
    let (close_tx, close_rx) = watch::channel(false);
    let mut state = lock_state(context);
    state
        .active_connections
        .entry(grant_id)
        .or_default()
        .insert(connection_id, ActiveConnection { close_tx });
    update_connected_status(&mut state);
    (connection_id, close_rx)
}

fn unregister_connection(context: &ServerContext, connection_id: Uuid) {
    let mut state = lock_state(context);
    let grant_id = state
        .active_connections
        .iter()
        .find_map(|(grant_id, connections)| connections.contains_key(&connection_id).then_some(*grant_id));
    if let Some(grant_id) = grant_id {
        if let Some(connections) = state.active_connections.get_mut(&grant_id) {
            connections.remove(&connection_id);
            if connections.is_empty() {
                state.active_connections.remove(&grant_id);
            }
        }
        update_connected_status(&mut state);
    }
}

fn close_connections_for_grant(context: &ServerContext, grant_id: Uuid) {
    let mut state = lock_state(context);
    if let Some(connections) = state.active_connections.remove(&grant_id) {
        for connection in connections.into_values() {
            match connection.close_tx.send(true) {
                Ok(()) => {}
                Err(_) => {}
            }
        }
    }
    update_connected_status(&mut state);
}

fn close_all_connections(context: &ServerContext) {
    let mut state = lock_state(context);
    let connections = std::mem::take(&mut state.active_connections);
    for grant_connections in connections.into_values() {
        for connection in grant_connections.into_values() {
            match connection.close_tx.send(true) {
                Ok(()) => {}
                Err(_) => {}
            }
        }
    }
    update_connected_status(&mut state);
}

fn update_connected_status(state: &mut ServerContextInner) {
    let connected_devices = state.active_connections.values().map(HashMap::len).sum();
    if let Some(endpoint) = state.endpoint.clone() {
        if matches!(state.status, MobileServerStatus::Listening { .. }) {
            state.status = MobileServerStatus::Listening {
                endpoint,
                connected_devices,
            };
        }
    }
}

fn emit_grant_update(context: &ServerContext, update: GrantUpdate) {
    let sender = lock_state(context).grant_updates.clone();
    match sender.unbounded_send(update) {
        Ok(()) => {}
        Err(_) => {}
    }
}

fn prune_offers(state: &mut ServerContextInner, now: OffsetDateTime) {
    let expired_ids: Vec<Uuid> = state
        .offers
        .iter()
        .filter_map(|(offer_id, stored)| (now >= stored.offer.expires_at).then_some(*offer_id))
        .collect();
    for offer_id in expired_ids {
        state.offers.remove(&offer_id);
        state.expired_offers.insert(offer_id);
    }
}

fn random_base64_token() -> String {
    let mut bytes = Zeroizing::new([0_u8; 32]);
    OsRng.fill_bytes(bytes.as_mut_slice());
    URL_SAFE_NO_PAD.encode(bytes.as_slice())
}

fn decode_fixed(value: &str, expected_length: usize) -> Result<Zeroizing<Vec<u8>>> {
    let decoded = URL_SAFE
        .decode(value)
        .or_else(|_| URL_SAFE_NO_PAD.decode(value))
        .context("invalid base64url data")?;
    ensure!(
        decoded.len() == expected_length,
        "base64url data has an invalid length"
    );
    Ok(Zeroizing::new(decoded))
}

fn array_32(bytes: &[u8]) -> [u8; 32] {
    let mut array = [0; 32];
    array.copy_from_slice(bytes);
    array
}

fn decode_digest(value: &str) -> Result<[u8; 32]> {
    let bytes = decode_fixed(value, 32)?;
    Ok(array_32(&bytes))
}

async fn send_frame(socket: &mut WebSocket, frame: ServerFrame) -> bool {
    let json = match frame.to_json() {
        Ok(json) => json,
        Err(_) => return false,
    };
    if json.len() > MAX_WEBSOCKET_FRAME_BYTES {
        return false;
    }
    socket.send(Message::Text(json)).await.is_ok()
}

async fn send_failure_and_close(
    socket: &mut WebSocket,
    failure: ProtocolFailure,
    request_id: Option<Uuid>,
) {
    let failure_was_sent = send_frame(
        socket,
        ServerFrame::Error {
            request_id,
            code: failure.code.to_owned(),
            message: failure.message.to_owned(),
        },
    )
    .await;
    if !failure_was_sent {
        send_close(socket).await;
        return;
    }
    send_close(socket).await;
}

async fn send_close(socket: &mut WebSocket) {
    match socket.send(Message::Close(None)).await {
        Ok(()) => {}
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_tailscale_bindings() {
        assert!(validate_mobile_binding("127.0.0.1", 6769).is_err());
        assert!(validate_mobile_binding("0.0.0.0", 6769).is_err());
        assert!(validate_mobile_binding("192.168.1.4", 6769).is_err());
        assert!(validate_mobile_binding("8.8.8.8", 6769).is_err());
    }

    #[test]
    fn accepts_tailscale_ipv4_binding() {
        assert_eq!(
            validate_mobile_binding("100.88.4.2", 6769)
                .expect("Tailscale address should be accepted")
                .port
                .get(),
            6769
        );
    }

    #[test]
    fn accepts_only_tailscale_ranges() {
        assert!(is_tailscale_address("100.64.0.0".parse().expect("valid address")));
        assert!(is_tailscale_address(
            "100.127.255.255".parse().expect("valid address")
        ));
        assert!(!is_tailscale_address(
            "100.128.0.1".parse().expect("valid address")
        ));
        assert!(is_tailscale_address(
            "fd7a:115c:a1e0:abcd::1".parse().expect("valid address")
        ));
        assert!(!is_tailscale_address(
            "fd7a:115c:a1e1::1".parse().expect("valid address")
        ));
    }

    #[test]
    fn pairing_offer_svg_contains_an_svg_document() {
        let offer = PairingOffer {
            offer_id: Uuid::from_u128(1),
            endpoint: "ws://100.88.4.2:6769".to_owned(),
            protocol_version: MOBILE_PROTOCOL_VERSION,
            host_public_key: URL_SAFE_NO_PAD.encode([1; 32]),
            pairing_secret: URL_SAFE_NO_PAD.encode([2; 32]),
            expires_at: OffsetDateTime::UNIX_EPOCH + Duration::minutes(5),
        };
        let svg = pairing_offer_svg(&offer).expect("offer should render");
        assert!(svg.starts_with("<svg "));
        assert!(svg.contains("viewBox"));
        assert!(svg.contains("path"));
    }

    #[gpui::test]
    async fn pairing_challenge_is_pinned_and_offer_is_one_use(cx: &gpui::TestAppContext) {
        use std::{future::Future, pin::Pin};

        use credentials_provider::CredentialsProvider;

        struct EmptyCredentials;

        impl CredentialsProvider for EmptyCredentials {
            fn read_credentials<'a>(
                &'a self,
                _url: &'a str,
                _cx: &'a gpui::AsyncApp,
            ) -> Pin<Box<dyn Future<Output = Result<Option<(String, Vec<u8>)>>> + 'a>> {
                Box::pin(async { Ok(None) })
            }

            fn write_credentials<'a>(
                &'a self,
                _url: &'a str,
                _username: &'a str,
                _password: &'a [u8],
                _cx: &'a gpui::AsyncApp,
            ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
                Box::pin(async { Ok(()) })
            }

            fn delete_credentials<'a>(
                &'a self,
                _url: &'a str,
                _cx: &'a gpui::AsyncApp,
            ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
                Box::pin(async { Ok(()) })
            }
        }

        let database = db::kvp::KeyValueStore::open_test_db("mobile_server_pairing_flow").await;
        let store = Arc::new(MobileStore::new(
            database,
            Arc::new(EmptyCredentials),
        ));
        let (grant_updates, _grant_updates_rx) = futures::channel::mpsc::unbounded();
        let context = Arc::new(ServerContext::new(
            "Zed Desktop".to_owned(),
            store.clone(),
            grant_updates,
        ));
        let host_signing_key = SigningKey::from_bytes(&[3; 32]);
        let client_signing_key = SigningKey::from_bytes(&[7; 32]);
        let now = OffsetDateTime::now_utc();
        let offer_id = Uuid::from_u128(1);
        let pairing_secret = random_base64_token();
        let pairing_digest = Sha256::digest(pairing_secret.as_bytes());
        let mut pairing_digest_bytes = [0; 32];
        pairing_digest_bytes.copy_from_slice(&pairing_digest);
        let offer = PairingOffer {
            offer_id,
            endpoint: "ws://127.0.0.1:6769".to_owned(),
            protocol_version: MOBILE_PROTOCOL_VERSION,
            host_public_key: URL_SAFE_NO_PAD.encode(host_signing_key.verifying_key().to_bytes()),
            pairing_secret: pairing_secret.clone(),
            expires_at: now + OFFER_LIFETIME,
        };
        {
            let mut state = lock_state(&context);
            state.host_signing_key = Some(Arc::new(host_signing_key.clone()));
            state.offers.insert(
                offer_id,
                StoredOffer {
                    offer: offer.clone(),
                    secret_digest: pairing_digest_bytes,
                },
            );
        }

        let client_nonce = random_base64_token();
        let (pending, challenge) = begin_pairing(&context, offer_id, client_nonce.clone())
            .await
            .expect("pairing challenge should be produced");
        let ServerFrame::PairChallenge {
            offer_id: challenged_offer_id,
            client_nonce: challenged_client_nonce,
            server_nonce,
            host_signature,
        } = challenge
        else {
            panic!("expected a pairing challenge");
        };
        assert_eq!(challenged_offer_id, offer_id);
        let host_public_key = VerifyingKey::from_bytes(&array_32(
            &decode_fixed(&offer.host_public_key, 32).expect("host key should decode"),
        ))
        .expect("host key should be valid");
        let host_signature = Signature::from_slice(
            &decode_fixed(&host_signature, 64).expect("host signature should decode"),
        )
        .expect("host signature should be valid");
        let challenge_payload = pairing_payload(
            MOBILE_PROTOCOL_VERSION,
            offer_id,
            &challenged_client_nonce,
            &server_nonce,
        )
        .expect("pairing payload should encode");
        host_public_key
            .verify(&challenge_payload, &host_signature)
            .expect("QR-pinned host key should verify the challenge");

        let client_proof = client_signing_key.sign(&challenge_payload);
        let client_public_key = URL_SAFE_NO_PAD.encode(client_signing_key.verifying_key().to_bytes());
        let client_proof = URL_SAFE_NO_PAD.encode(client_proof.to_bytes());
        let (grant, token) = complete_pairing(
            &context,
            &store,
            pending.clone(),
            pairing_secret.clone(),
            client_public_key.clone(),
            client_proof.clone(),
            "Phone".to_owned(),
        )
        .await
        .expect("valid pairing should create a grant");
        assert!(token_matches(&grant.token_digest, token.as_bytes()));
        assert_eq!(store.grants().await.expect("grants should load").len(), 1);

        let (pending_authentication, challenge) =
            begin_authentication(&context, &store, grant.id, random_base64_token())
                .await
                .expect("active grants should receive an authentication challenge");
        let ServerFrame::ServerChallenge {
            grant_id: challenged_grant_id,
            client_nonce: challenged_client_nonce,
            server_nonce,
            host_signature,
        } = challenge
        else {
            panic!("expected an authentication challenge");
        };
        assert_eq!(challenged_grant_id, grant.id);
        let authentication_digest =
            decode_digest(&grant.token_digest).expect("grant digest should decode");
        let authentication_payload = authentication_payload(
            MOBILE_PROTOCOL_VERSION,
            grant.id,
            &challenged_client_nonce,
            &server_nonce,
            &authentication_digest,
        )
        .expect("authentication payload should encode");
        let host_signature = Signature::from_slice(
            &decode_fixed(&host_signature, 64).expect("host signature should decode"),
        )
        .expect("host signature should be valid");
        host_public_key
            .verify(&authentication_payload, &host_signature)
            .expect("QR-pinned host key should verify authentication challenge");
        let authentication_proof = client_signing_key.sign(&authentication_payload);
        authenticate(
            &context,
            &store,
            &pending_authentication,
            token.clone(),
            URL_SAFE_NO_PAD.encode(authentication_proof.to_bytes()),
        )
        .await
        .expect("valid token and device proof should authenticate");

        store
            .revoke_grant(grant.id, now)
            .await
            .expect("grant should revoke");
        let revoked = begin_authentication(&context, &store, grant.id, random_base64_token())
            .await
            .expect_err("revoked grants must not receive a challenge");
        assert_eq!(revoked.code, "grant_revoked");

        let reused = complete_pairing(
            &context,
            &store,
            pending,
            pairing_secret,
            client_public_key,
            client_proof,
            "Phone".to_owned(),
        )
        .await
        .expect_err("a pairing offer must be one-use");
        assert_eq!(reused.code, "offer_consumed");

        let expired_id = Uuid::from_u128(2);
        let expired_secret = random_base64_token();
        let expired_digest = Sha256::digest(expired_secret.as_bytes());
        let mut expired_digest_bytes = [0; 32];
        expired_digest_bytes.copy_from_slice(&expired_digest);
        {
            let mut state = lock_state(&context);
            state.offers.insert(
                expired_id,
                StoredOffer {
                    offer: PairingOffer {
                        offer_id: expired_id,
                        endpoint: offer.endpoint,
                        protocol_version: MOBILE_PROTOCOL_VERSION,
                        host_public_key: offer.host_public_key,
                        pairing_secret: expired_secret,
                        expires_at: now - Duration::seconds(1),
                    },
                    secret_digest: expired_digest_bytes,
                },
            );
        }
        let expired = begin_pairing(&context, expired_id, random_base64_token())
            .await
            .expect_err("expired offers must be rejected");
        assert_eq!(expired.code, "offer_expired");
        let _ = cx;
    }
}
