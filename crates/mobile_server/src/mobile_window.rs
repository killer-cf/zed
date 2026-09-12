use std::{net::IpAddr, sync::Arc};

use anyhow::{Result, anyhow, ensure};
use gpui::{
    App, AppContext, ClipboardItem, Context, RenderImage, Subscription, Task, TaskExt, Window,
    WindowOptions, actions, div, img, prelude::*, px,
};
use mobile_protocol::PairingOffer;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    DeviceGrant, MobileBinding, MobileServer, MobileServerStatus, pairing_offer_svg,
    tailscale_addresses, validate_mobile_binding,
};

const DEFAULT_PORT: u16 = 6769;

// Opens the native Mobile control window.
actions!(zed_mobile, [OpenControl]);

/// The redaction-safe data displayed for an ephemeral pairing offer.
///
/// The pairing URL and secret stay in the corresponding [`PairingOffer`] held by the model and are
/// only read by the explicit copy action. The rendered text contains the expiry but never the URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingDisplay {
    pub qr_svg: String,
    pub visible_text: String,
    pub expires_at: OffsetDateTime,
}

const QR_RENDER_SCALE: f32 = 8.0;

fn rasterize_pairing_svg(qr_svg: &str, cx: &App) -> Result<Arc<RenderImage>> {
    cx.svg_renderer()
        .render_single_frame(qr_svg.as_bytes(), QR_RENDER_SCALE)
        .map_err(|error| anyhow!(error))
}

/// State owned by the Mobile control surface.
///
/// This model deliberately stores only addresses returned by [`tailscale_addresses`]. A binding is
/// validated again immediately before enabling so a stale or programmatically supplied selection
/// cannot bypass the server's Tailscale-only boundary.
pub struct MobileControlModel {
    addresses: Vec<IpAddr>,
    selected_address: Option<IpAddr>,
    port: u16,
    pairing_offer: Option<PairingOffer>,
    pairing_display: Option<PairingDisplay>,
    pairing_qr_image: Option<Arc<RenderImage>>,
    discovery_error: Option<String>,
    error: Option<String>,
}

impl Default for MobileControlModel {
    fn default() -> Self {
        Self {
            addresses: Vec::new(),
            selected_address: None,
            port: DEFAULT_PORT,
            pairing_offer: None,
            pairing_display: None,
            pairing_qr_image: None,
            discovery_error: None,
            error: None,
        }
    }
}

impl MobileControlModel {
    /// Builds the model from the currently discovered, safe Tailscale addresses.
    pub fn new<C: AppContext>(cx: &mut C) -> Self {
        let _ = cx;
        match tailscale_addresses() {
            Ok(addresses) => Self::with_addresses(addresses),
            Err(error) => {
                let mut model = Self::default();
                model.discovery_error = Some(format!("Could not discover Tailscale addresses: {error:#}"));
                model
            }
        }
    }

    /// Builds a model from an address fixture, retaining only Tailscale addresses.
    pub fn with_addresses(addresses: Vec<IpAddr>) -> Self {
        let mut addresses = addresses
            .into_iter()
            .filter(|address| crate::is_tailscale_address(*address))
            .collect::<Vec<_>>();
        addresses.sort();
        addresses.dedup();
        let selected_address = addresses.first().copied();
        Self {
            addresses,
            selected_address,
            ..Self::default()
        }
    }

    /// Returns the addresses that can be selected for a Mobile listener.
    pub fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }

    /// Returns the selected listener address, if one is available.
    pub fn selected_address(&self) -> Option<IpAddr> {
        self.selected_address
    }

    /// Returns the currently selected listener port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Returns the latest redaction-safe pairing display, if one was generated.
    pub fn pairing_display_snapshot(&self) -> Option<&PairingDisplay> {
        self.pairing_display.as_ref()
    }

    /// Returns the address-discovery failure, if interface enumeration failed.
    pub fn discovery_error(&self) -> Option<&str> {
        self.discovery_error.as_deref()
    }

    /// Returns the latest UI/server operation failure, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Selects one of the discovered Tailscale addresses.
    pub fn select_address(&mut self, address: IpAddr) -> Result<()> {
        ensure!(
            self.addresses.contains(&address),
            "address is not a discovered Tailscale address"
        );
        ensure!(
            crate::is_tailscale_address(address),
            "address is not a Tailscale address"
        );
        self.selected_address = Some(address);
        self.error = None;
        Ok(())
    }

    /// Selects the next discovered address for the listener.
    pub fn select_next_address(&mut self) {
        if self.addresses.is_empty() {
            self.selected_address = None;
            return;
        }
        let Some(selected_address) = self.selected_address else {
            self.selected_address = self.addresses.first().copied();
            return;
        };
        let Some(index) = self.addresses.iter().position(|address| *address == selected_address)
        else {
            self.selected_address = self.addresses.first().copied();
            return;
        };
        self.selected_address = self.addresses.get((index + 1) % self.addresses.len()).copied();
    }

    /// Sets the listener port. Validation is deferred until [`Self::selected_binding`] so an
    /// invalid edit is visible to the user and cannot reach `MobileServer::enable`.
    pub fn set_port(&mut self, port: u16) {
        self.port = port;
        self.error = None;
    }

    /// Validates the selected address/port before handing it to `MobileServer::enable`.
    pub fn selected_binding(&self) -> Result<MobileBinding> {
        let address = self
            .selected_address
            .ok_or_else(|| anyhow!("select a discovered Tailscale address first"))?;
        ensure!(
            self.addresses.contains(&address),
            "selected address is not a discovered Tailscale address"
        );
        validate_mobile_binding(&address.to_string(), self.port)
    }

    /// Reads the current server status from the GPUI global.
    pub fn server_status<C: AppContext>(&self, cx: &C) -> MobileServerStatus {
        cx.read_global(|server: &MobileServer, _| server.status())
    }

    /// Returns active paired-device grants for the next model snapshot.
    ///
    /// Revoked grants remain durable server metadata but are not paired devices in this control
    /// surface, so a successful revoke disappears from the next snapshot.
    pub fn grants_snapshot<C: AppContext>(&self, cx: &C) -> Vec<DeviceGrant> {
        cx.read_global(|server: &MobileServer, _| Self::visible_grants(server.grants()))
    }

    fn visible_grants(grants: &[DeviceGrant]) -> Vec<DeviceGrant> {
        grants
            .iter()
            .filter(|grant| grant.revoked_at.is_none())
            .cloned()
            .collect()
    }

    /// Builds a QR SVG and expiry-only text for an offer without exposing its secret or URL.
    pub fn pairing_display(offer: &PairingOffer) -> Result<PairingDisplay> {
        let qr_svg = pairing_offer_svg(offer)?;
        let visible_text = format!("Scan this QR code before {}.", offer.expires_at);
        Ok(PairingDisplay {
            qr_svg,
            visible_text,
            expires_at: offer.expires_at,
        })
    }

    fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    fn clear_error(&mut self) {
        self.error = None;
    }
}

/// The self-contained native Mobile control window.
pub struct MobileControlWindow {
    model: MobileControlModel,
    _server_subscription: Subscription,
}

impl MobileControlWindow {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = MobileControlModel::new(cx);
        let server_subscription =
            cx.observe_global::<MobileServer>(|_window, cx| cx.notify());
        Self {
            model,
            _server_subscription: server_subscription,
        }
    }
    pub fn model(&self) -> &MobileControlModel {
        &self.model
    }

    fn enable(&mut self, cx: &mut Context<Self>) {
        let binding = match self.model.selected_binding() {
            Ok(binding) => binding,
            Err(error) => {
                self.model.set_error(error.to_string());
                cx.notify();
                return;
            }
        };
        let task = cx.update_global::<MobileServer, _>(|server, app| server.enable(binding, app));
        self.observe_server_task(task, "enabling Mobile server", cx);
    }

    fn disable(&mut self, cx: &mut Context<Self>) {
        let task = cx.update_global::<MobileServer, _>(|server, app| server.disable(app));
        self.observe_server_task(task, "disabling Mobile server", cx);
    }

    fn generate_pairing_qr(&mut self, cx: &mut Context<Self>) {
        let offer = cx.update_global::<MobileServer, _>(|server, _| {
            server.create_pairing_offer(OffsetDateTime::now_utc())
        });
        match offer {
            Ok(offer) => match MobileControlModel::pairing_display(&offer) {
                Ok(display) => match rasterize_pairing_svg(&display.qr_svg, cx) {
                    Ok(qr_image) => {
                        self.model.pairing_offer = Some(offer);
                        self.model.pairing_display = Some(display);
                        self.model.pairing_qr_image = Some(qr_image);
                        self.model.clear_error();
                    }
                    Err(error) => self.model.set_error(error.to_string()),
                },
                Err(error) => self.model.set_error(error.to_string()),
            },
            Err(error) => self.model.set_error(error.to_string()),
        }
        cx.notify();
    }

    fn copy_pairing_code(&mut self, cx: &mut Context<Self>) {
        let Some(offer) = self.model.pairing_offer.as_ref() else {
            self.model.set_error("Generate a pairing QR code first");
            cx.notify();
            return;
        };
        // This is intentionally the only control that reads the raw pairing URL.
        let url = match offer.encode_url() {
            Ok(url) => url,
            Err(error) => {
                self.model.set_error(error.to_string());
                cx.notify();
                return;
            }
        };
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        self.model.clear_error();
        cx.notify();
    }

    fn revoke_grant(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let task = cx.update_global::<MobileServer, _>(|server, app| server.revoke_grant(id, app));
        self.observe_server_task(task, "revoking paired device", cx);
    }

    fn observe_server_task<T: 'static>(
        &mut self,
        task: Task<Result<T>>,
        operation: &'static str,
        cx: &mut Context<Self>,
    ) {
        let entity = cx.weak_entity();
        cx.spawn(async move |_, async_cx| {
            let result = task.await;
            let error = result
                .as_ref()
                .err()
                .map(|error| format!("{operation}: {error:#}"));
            entity.update(async_cx, |window, cx| {
                if let Some(error) = error {
                    window.model.set_error(error);
                } else {
                    window.model.clear_error();
                }
                cx.notify();
            })?;
            result.map(|_| ())
        })
        .detach_and_log_err(cx);
    }

    fn render_address_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .model
            .addresses()
            .iter()
            .copied()
            .map(|address| {
                let selected = self.model.selected_address() == Some(address);
                let label = if selected {
                    format!("✓ {address}")
                } else {
                    address.to_string()
                };
                div()
                    .id(format!("mobile-address-{address}"))
                    .cursor_pointer()
                    .child(label)
                    .on_click(cx.listener(move |window, _, _, cx| {
                        if let Err(error) = window.model.select_address(address) {
                            window.model.set_error(error.to_string());
                        }
                        cx.notify();
                    }))
            })
            .collect::<Vec<_>>();

        let addresses = if rows.is_empty() {
            div().child(
                self.model
                    .discovery_error()
                    .map(str::to_owned)
                    .unwrap_or_else(|| "No Tailscale addresses discovered.".to_owned()),
            )
        } else {
            div().children(rows)
        };
        div()
            .child("Listener address (Tailscale only)")
            .child(addresses)
    }

    fn render_port_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let decrement = div()
            .id("mobile-port-decrement")
            .cursor_pointer()
            .child("−")
            .on_click(cx.listener(|window, _, _, cx| {
                window.model.set_port(window.model.port().saturating_sub(1));
                cx.notify();
            }));
        let increment = div()
            .id("mobile-port-increment")
            .cursor_pointer()
            .child("+")
            .on_click(cx.listener(|window, _, _, cx| {
                window.model.set_port(window.model.port().saturating_add(1));
                cx.notify();
            }));
        div()
            .child("Listener port")
            .child(decrement)
            .child(format!(" {} ", self.model.port()))
            .child(increment)
    }

    fn render_pairing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(display) = self.model.pairing_display_snapshot() else {
            return div().child("No pairing QR generated.").into_any_element();
        };
        let Some(qr_image) = self.model.pairing_qr_image.clone() else {
            return div().child("Pairing QR is unavailable.").into_any_element();
        };
        let qr = img(qr_image).size(px(220.));
        div()
            .child(qr)
            .child(display.visible_text.clone())
            .child(
                div()
                    .id("mobile-copy-pairing-code")
                    .cursor_pointer()
                    .child("Copy pairing code")
                    .on_click(cx.listener(|window, _, _, cx| window.copy_pairing_code(cx))),
            )
            .into_any_element()
    }

    fn render_grants(
        &self,
        grants: &[DeviceGrant],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let grants = MobileControlModel::visible_grants(grants)
            .into_iter()
            .map(|grant| {
                let id = grant.id;
                div()
                    .child(format!("{} (paired {})", grant.label, grant.created_at))
                    .child(
                        div()
                            .id(format!("mobile-revoke-{id}"))
                            .cursor_pointer()
                            .child("Revoke")
                            .on_click(cx.listener(move |window, _, _, cx| {
                                window.revoke_grant(id, cx)
                            })),
                    )
            })
            .collect::<Vec<_>>();
        if grants.is_empty() {
            div().child("No paired devices.").into_any_element()
        } else {
            div().children(grants).into_any_element()
        }
    }
}

impl Render for MobileControlWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (status, grants) = MobileServer::try_global(cx)
            .map(|server| (server.status(), server.grants().to_vec()))
            .unwrap_or((MobileServerStatus::Disabled, Vec::new()));
        let status_text = match &status {
            MobileServerStatus::Disabled => "Disabled".to_owned(),
            MobileServerStatus::Starting => "Starting".to_owned(),
            MobileServerStatus::Listening {
                endpoint,
                connected_devices,
            } => format!("Listening at {endpoint} ({connected_devices} connected)"),
            MobileServerStatus::Error { message } => format!("Error: {message}"),
        };

        let enable = div()
            .id("mobile-enable")
            .cursor_pointer()
            .child("Enable")
            .on_click(cx.listener(|window, _, _, cx| window.enable(cx)));
        let disable = div()
            .id("mobile-disable")
            .cursor_pointer()
            .child("Disable")
            .on_click(cx.listener(|window, _, _, cx| window.disable(cx)));
        let generate = div()
            .id("mobile-generate-pairing-qr")
            .cursor_pointer()
            .child("Generate pairing QR")
            .on_click(cx.listener(|window, _, _, cx| window.generate_pairing_qr(cx)));

        div()
            .id("mobile-control-window")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child("Mobile control")
            .child(format!("Status: {status_text}"))
            .child(self.render_address_controls(cx))
            .child(self.render_port_controls(cx))
            .child(div().flex().gap_2().child(enable).child(disable))
            .child(generate)
            .child(self.render_pairing(cx))
            .child("Paired devices")
            .child(self.render_grants(&grants, cx))
            .when_some(self.model.error(), |element, error| {
                element.child(format!("Error: {error}"))
            })
    }
}

/// Registers `zed_mobile::OpenControl` with GPUI's normal action inventory and opens the native
/// control window. The initializer is called only by `MobileServer::init`.
pub fn init_mobile_window(cx: &mut App) {
    cx.on_action(|_: &OpenControl, cx| {
        let result = cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| MobileControlWindow::new(window, cx))
        });
        if let Err(error) = result {
            Task::ready(Err::<(), _>(error)).detach_and_log_err(cx);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, future::Future, net::IpAddr, pin::Pin, sync::Arc};

    use anyhow::Result;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use credentials_provider::CredentialsProvider;
    use db::kvp::KeyValueStore;
    use gpui::{AsyncApp, TestAppContext};
    use mobile_protocol::{Capability, MOBILE_PROTOCOL_VERSION, PairingOffer};
    use time::{Duration, OffsetDateTime};

    use super::*;

    #[derive(Default)]
    struct TestCredentialsProvider;

    impl CredentialsProvider for TestCredentialsProvider {
        fn read_credentials<'a>(
            &'a self,
            _url: &'a str,
            _cx: &'a AsyncApp,
        ) -> Pin<Box<dyn Future<Output = Result<Option<(String, Vec<u8>)>>> + 'a>> {
            Box::pin(async { Ok(None) })
        }

        fn write_credentials<'a>(
            &'a self,
            _url: &'a str,
            _username: &'a str,
            _password: &'a [u8],
            _cx: &'a AsyncApp,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn delete_credentials<'a>(
            &'a self,
            _url: &'a str,
            _cx: &'a AsyncApp,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    async fn install_server(cx: &TestAppContext, name: &'static str) {
        let database = KeyValueStore::open_test_db(name).await;
        let store = crate::MobileStore::new(database, Arc::new(TestCredentialsProvider));
        cx.update(|app| MobileServer::init(store, "Zed Desktop".into(), app));
    }

    fn test_pairing_offer() -> PairingOffer {
        PairingOffer {
            offer_id: Uuid::from_u128(7),
            endpoint: "ws://100.88.4.2:6769".to_owned(),
            protocol_version: MOBILE_PROTOCOL_VERSION,
            host_public_key: URL_SAFE_NO_PAD.encode([1; 32]),
            pairing_secret: URL_SAFE_NO_PAD.encode([2; 32]),
            expires_at: OffsetDateTime::UNIX_EPOCH + Duration::minutes(5),
        }
    }

    fn test_grant() -> DeviceGrant {
        let client_key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
        DeviceGrant {
            id: Uuid::from_u128(1),
            label: "Test phone".to_owned(),
            client_public_key: URL_SAFE_NO_PAD.encode(client_key.verifying_key().to_bytes()),
            token_digest: crate::token_digest(b"test-device-token"),
            capabilities: [Capability::StatusRead].into_iter().collect(),
            created_at: OffsetDateTime::UNIX_EPOCH,
            revoked_at: None,
        }
    }

    #[gpui::test]
    async fn test_mobile_control_shows_disabled_state(cx: &mut TestAppContext) {
        install_server(cx, "mobile_window_disabled").await;
        let model = MobileControlModel::new(cx);
        assert!(matches!(model.server_status(cx), MobileServerStatus::Disabled));
    }

    #[gpui::test]
    async fn test_mobile_control_never_exposes_pairing_secret_as_text(
        cx: &mut TestAppContext,
    ) {
        install_server(cx, "mobile_window_pairing_redaction").await;
        let offer = test_pairing_offer();
        let display =
            MobileControlModel::pairing_display(&offer).expect("pairing display should render");
        assert!(display.qr_svg.contains("<svg"));
        assert!(!display.visible_text.contains(&offer.pairing_secret));
        assert!(!display
            .visible_text
            .contains(&offer.encode_url().expect("pairing URL should encode")));
    }

    #[gpui::test]
    async fn test_mobile_control_rasterizes_qr_with_distinct_module_colors(
        cx: &mut TestAppContext,
    ) {
        install_server(cx, "mobile_window_qr_raster").await;
        let display = MobileControlModel::pairing_display(&test_pairing_offer())
            .expect("pairing display should render");
        let image = cx
            .read(|app| rasterize_pairing_svg(&display.qr_svg, app))
            .expect("pairing QR should rasterize");
        let pixels = image.as_bytes(0).expect("rasterized QR should have pixels");
        let colors = pixels
            .chunks_exact(4)
            .map(|pixel| (pixel[0], pixel[1], pixel[2], pixel[3]))
            .collect::<HashSet<_>>();
        assert!(
            colors.len() > 1,
            "rasterized QR should preserve both dark modules and its light quiet zone"
        );
    }

    #[gpui::test]
    async fn test_mobile_control_rejects_unsafe_binding_before_enable(
        _cx: &mut TestAppContext,
    ) {
        let mut model = MobileControlModel::with_addresses(vec![
            "100.64.0.1".parse::<IpAddr>().expect("address should parse"),
            "100.127.255.254"
                .parse::<IpAddr>()
                .expect("address should parse"),
            "100.128.0.1".parse::<IpAddr>().expect("address should parse"),
            "fd7a:115c:a1e0::1"
                .parse::<IpAddr>()
                .expect("address should parse"),
            "fd7a:115c:a1e1::1"
                .parse::<IpAddr>()
                .expect("address should parse"),
            "192.168.1.20"
                .parse::<IpAddr>()
                .expect("address should parse"),
        ]);
        assert_eq!(model.addresses().len(), 3);
        assert!(!model.addresses().contains(
            &"100.128.0.1"
                .parse::<IpAddr>()
                .expect("address should parse")
        ));
        assert!(!model.addresses().contains(
            &"fd7a:115c:a1e1::1"
                .parse::<IpAddr>()
                .expect("address should parse")
        ));
        assert!(!model.addresses().contains(
            &"192.168.1.20"
                .parse::<IpAddr>()
                .expect("address should parse")
        ));
        model.set_port(0);
        assert!(model.selected_binding().is_err());
    }

    #[gpui::test]
    async fn test_mobile_control_hides_revoked_grants_from_next_snapshot(
        cx: &mut TestAppContext,
    ) {
        let database = KeyValueStore::open_test_db("mobile_window_revoke_snapshot").await;
        let store = crate::MobileStore::new(database, Arc::new(TestCredentialsProvider));
        let grant = test_grant();
        store
            .insert_grant(grant.clone())
            .await
            .expect("grant should persist");
        cx.update(|app| MobileServer::init(store, "Zed Desktop".into(), app));
        cx.update(|app| {
            app.update_global::<MobileServer, _>(|server, _| server.grants.push(grant.clone()))
        });

        let model = MobileControlModel::new(cx);
        assert_eq!(model.grants_snapshot(cx), vec![grant.clone()]);

        cx.update(|app| {
            app.update_global::<MobileServer, _>(|server, app| server.revoke_grant(grant.id, app))
        })
        .await
        .expect("grant should revoke");
        assert!(model.grants_snapshot(cx).is_empty());
    }
}
