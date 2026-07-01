//! Connection state machine: owns the running tunnel and every OS change made
//! for it (routes, DNS, firewall, split tunneling), and broadcasts status
//! changes to clients.
//!
//! After every state or settings change, `sync_protection` recomputes the
//! firewall from scratch, so the kill switch always matches the current state.
//!
//! Connecting does not hold the state lock while resolving endpoints and
//! bringing the tunnel up, so a disconnect (or another connect) can cancel
//! it: each attempt has a generation, and an attempt whose generation is no
//! longer current undoes its own work and gives up.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use submarine_config::{Endpoint, TunnelConfig};
use submarine_ipc::{
    ConnectionState, Event, KillSwitch, PeerStatus, Request, Response, Settings, SplitTunnelMode,
    Status,
};
use submarine_net::{
    DnsManager, FWMARK, Firewall, FirewallPolicy, RouteManager, RouteOptions, SplitMode,
    SplitTunnel, TunnelNetConfig,
};
use submarine_tunnel::{PeerStats, Tunnel, TunnelOptions};
use tokio::sync::{Mutex, broadcast, oneshot, watch};

use crate::store::TunnelStore;

/// Name requested for the tunnel interface.
const INTERFACE_NAME: &str = "submarine0";
/// Interval between peer statistics updates (and network change checks).
const STATS_INTERVAL: Duration = Duration::from_secs(1);
/// Maximum number of apps in the split tunneling list.
const MAX_SPLIT_APPS: usize = 256;

/// The running tunnel and the OS changes made for it.
struct Active {
    /// Generation of the attempt that created it.
    generation: u64,
    /// WireGuard tunnel; dropping it removes the interface.
    tunnel: Tunnel,
    /// Network parameters of the tunnel, shared by routes, DNS and firewall.
    net: TunnelNetConfig,
    /// Routes added for the tunnel.
    routes: RouteManager,
    /// DNS configuration applied for the tunnel.
    dns: DnsManager,
    /// Dropping it stops the monitor task.
    _stop_monitor: oneshot::Sender<()>,
}

/// Mutable state of the service, behind `Service::inner`.
struct Inner {
    /// Status published to the clients.
    status: Status,
    /// Running tunnel, if any.
    active: Option<Active>,
    /// Bumped by every connect and disconnect; identifies the current attempt.
    generation: u64,
    /// Current settings, same as the saved ones.
    settings: Settings,
    /// Endpoint hostnames are being resolved under the kill switch.
    resolving: bool,
    /// Resolved endpoints of the tunnel being connected or running.
    endpoints: Vec<SocketAddr>,
    /// The tunnel dropped unexpectedly with the `on_connect` kill switch:
    /// keep blocking until the user disconnects.
    blocked_after_drop: bool,
    /// Kill switch rules.
    firewall: Firewall,
    /// Split tunneling (per-app routing).
    split: SplitTunnel,
}

/// The daemon service, shared by every client through an `Arc`.
pub struct Service {
    /// Persistent storage.
    store: TunnelStore,
    /// Connection state.
    inner: Mutex<Inner>,
    /// Held while an attempt brings the interface up: only one can exist.
    bringing_up: Mutex<()>,
    /// Events for the clients (status, tunnel list and settings changes).
    events: broadcast::Sender<Event>,
}

/// Result whose error is the message shown to the user.
type Result<T> = std::result::Result<T, String>;

impl Service {
    /// Creates the service: cleans up what a crashed instance may have left
    /// behind, then reconnects the tunnel the user left connected (or the
    /// auto-connect one), or otherwise applies the kill switch for the
    /// disconnected state.
    pub async fn new(store: TunnelStore) -> Arc<Self> {
        // undo whatever a previous instance left behind if it crashed. The
        // firewall is NOT touched here: it is recomputed below or by the
        // reconnect, without a gap in protection
        if let Err(err) = DnsManager::new().reset().await {
            tracing::warn!("dns cleanup failed: {err}");
        }
        match RouteManager::new() {
            Ok(mut routes) => {
                if let Err(err) = routes.reset().await {
                    tracing::warn!("route cleanup failed: {err}");
                }
            }
            Err(err) => tracing::warn!("route cleanup failed: {err}"),
        }
        let mut split = SplitTunnel::new();
        if let Err(err) = split.stop().await {
            tracing::warn!("split tunnel cleanup failed: {err}");
        }

        // initial state: disconnected, settings from disk
        let service = Arc::new(Self {
            inner: Mutex::new(Inner {
                status: Status::default(),
                active: None,
                generation: 0,
                settings: store.settings(),
                resolving: false,
                endpoints: Vec::new(),
                blocked_after_drop: false,
                firewall: Firewall::new(),
                split,
            }),
            store,
            bringing_up: Mutex::new(()),
            events: broadcast::channel(64).0,
        });

        // the tunnel left connected wins over the auto-connect one, which is used
        // only if it still exists; the connection runs in the background so the
        // daemon starts serving clients right away
        let restore = service.store.connected_tunnel();
        let auto = service.inner.lock().await.settings.auto_connect.clone();
        let auto = auto.filter(|id| service.store.get(id).is_ok());
        match restore.or(auto) {
            Some(id) => {
                tracing::info!(%id, "connecting at startup");
                let service = service.clone();
                tokio::spawn(async move {
                    if let Err(err) = service.connect(&id).await {
                        tracing::error!("reconnect failed: {err}");
                    }
                });
            }
            None => {
                let mut inner = service.inner.lock().await;
                service.sync_protection(&mut inner).await;
            }
        }
        service
    }

    /// Subscribes to the events sent to the clients.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Handles a client request.
    ///
    /// `Connect` and `Disconnect` save the user's choice first, so it survives a
    /// restart of the daemon.
    pub async fn handle(self: &Arc<Self>, request: Request) -> Result<Response> {
        match request {
            Request::GetStatus => Ok(Response::Status(self.inner.lock().await.status.clone())),
            Request::ListTunnels => self.store.list().map(Response::Tunnels).map_err(err),
            Request::ImportTunnel { name, config } => self.import(&name, &config),
            Request::GetTunnelConfig { id } => {
                let stored = self.store.get(&id).map_err(err)?;
                // private keys never leave the daemon
                Ok(Response::TunnelConfig {
                    name: stored.name,
                    config: stored.config.to_redacted_conf_string(),
                })
            }
            Request::UpdateTunnel { id, name, config } => self.update(&id, &name, &config).await,
            Request::DeleteTunnel { id } => self.delete(&id).await,
            Request::Connect { id } => {
                self.store.save_connected_tunnel(Some(&id)).map_err(err)?;
                self.connect(&id).await.map(|()| Response::Ok)
            }
            Request::Disconnect => {
                self.store.save_connected_tunnel(None).map_err(err)?;
                self.disconnect().await;
                Ok(Response::Ok)
            }
            Request::GetLogs => Ok(Response::Logs(crate::logbuf::snapshot())),
            Request::ClearLogs => {
                crate::logbuf::clear();
                Ok(Response::Ok)
            }
            Request::GetSettings => {
                Ok(Response::Settings(self.inner.lock().await.settings.clone()))
            }
            Request::SetSettings { settings } => {
                self.set_settings(settings).await.map(Response::Settings)
            }
        }
    }

    /// Parses and stores a new tunnel from the text of a `.conf` file.
    fn import(&self, name: &str, text: &str) -> Result<Response> {
        let parsed = submarine_config::parse(text).map_err(err)?;
        let id = self
            .store
            .insert(name, parsed.config.clone())
            .map_err(err)?;
        self.notify_tunnels();
        Ok(imported(id, name, parsed))
    }

    /// Replaces a tunnel with the edited configuration. `text` is the redacted
    /// form sent by `GetTunnelConfig`: the secrets left out are taken from the
    /// stored configuration.
    async fn update(self: &Arc<Self>, id: &str, name: &str, text: &str) -> Result<Response> {
        let previous = self.store.get(id).map_err(err)?;
        let parsed = submarine_config::parse_edited(text, &previous.config).map_err(err)?;
        self.store
            .update(id, name, parsed.config.clone())
            .map_err(err)?;
        tracing::info!(%id, "tunnel updated");
        self.notify_tunnels();

        // a tunnel in use picks up the new configuration right away; a rename
        // alone does not need a reconnect
        let in_use = {
            let inner = self.inner.lock().await;
            inner.status.tunnel_id.as_deref() == Some(id)
                && matches!(
                    inner.status.state,
                    ConnectionState::Connecting | ConnectionState::Connected
                )
        };
        if in_use && parsed.config != previous.config {
            tracing::info!(%id, "reconnecting with the new configuration");
            let service = self.clone();
            let id = id.to_owned();
            tokio::spawn(async move {
                if let Err(e) = service.connect(&id).await {
                    tracing::error!("reconnect failed: {e}");
                }
            });
        }

        Ok(imported(id.to_owned(), name, parsed))
    }

    /// Deletes a tunnel, refused while it is in use. It is also removed from
    /// the auto-connect setting.
    async fn delete(&self, id: &str) -> Result<Response> {
        if self.inner.lock().await.status.tunnel_id.as_deref() == Some(id) {
            return Err("disconnect before deleting this tunnel".into());
        }
        self.store.delete(id).map_err(err)?;
        let auto = self.inner.lock().await.settings.auto_connect.clone();
        if auto.as_deref() == Some(id) {
            let mut settings = self.inner.lock().await.settings.clone();
            settings.auto_connect = None;
            self.set_settings(settings).await?;
        }
        self.notify_tunnels();
        Ok(Response::Ok)
    }

    /// Sends the updated tunnel list to the clients.
    fn notify_tunnels(&self) {
        if let Ok(list) = self.store.list() {
            let _ = self.events.send(Event::TunnelsChanged(list));
        }
    }

    /// Validates, saves and applies new settings, then notifies the clients.
    async fn set_settings(&self, settings: Settings) -> Result<Settings> {
        validate(&self.store, &settings)?;
        self.store.save_settings(&settings).map_err(err)?;
        let mut inner = self.inner.lock().await;
        let routing_changed = route_options(&inner.settings) != route_options(&settings);
        inner.settings = settings.clone();

        // switching between include and full routing changes routes and DNS
        if routing_changed {
            let opts = route_options(&settings);
            if let Some(active) = inner.active.as_mut() {
                if let Err(e) = apply_routing(active, opts).await {
                    tracing::error!("reconfiguring routes failed: {e}");
                }
            }
        }
        // the block after a drop exists only with the `on_connect` kill switch
        if settings.kill_switch != KillSwitch::OnConnect {
            inner.blocked_after_drop = false;
        }
        self.sync_protection(&mut inner).await;
        let _ = self.events.send(Event::SettingsChanged(settings.clone()));
        Ok(settings)
    }

    /// Connects tunnel `id`, replacing the running one if any.
    ///
    /// Returns Ok also when the attempt is cancelled by a later connect or
    /// disconnect: the caller asked for something that no longer applies.
    async fn connect(self: &Arc<Self>, id: &str) -> Result<()> {
        let stored = self.store.get(id).map_err(err)?;
        // start of a new attempt: the previous tunnel is torn down and the status
        // becomes `Connecting`
        let generation = {
            let mut inner = self.inner.lock().await;
            if let Some(active) = inner.active.take() {
                teardown(active).await;
            }
            inner.generation += 1;
            inner.blocked_after_drop = false;
            inner.endpoints.clear();
            inner.resolving = stored
                .config
                .peers
                .iter()
                .any(|p| matches!(p.endpoint, Some(Endpoint::Host { .. })));
            self.set_status(
                &mut inner,
                Status {
                    state: ConnectionState::Connecting,
                    tunnel_id: Some(id.to_owned()),
                    ..Status::default()
                },
            );
            // block before resolving and handshaking, so nothing leaks meanwhile
            self.sync_protection(&mut inner).await;
            inner.generation
        };

        // a cancelled attempt may still be cleaning up its interface
        let _bringing_up = self.bringing_up.lock().await;
        if self.inner.lock().await.generation != generation {
            return Ok(());
        }

        // resolution of the endpoint hostnames, without holding the state lock
        let resolved = submarine_tunnel::resolve_endpoints(&stored.config).await;
        let opts = {
            let mut inner = self.inner.lock().await;
            if inner.generation != generation {
                tracing::info!("connection attempt cancelled");
                return Ok(());
            }
            if let Ok(resolved) = &resolved {
                // let the firewall allow the endpoints before the first handshake
                inner.resolving = false;
                inner.endpoints = resolved.iter().flatten().copied().collect();
                self.sync_protection(&mut inner).await;
            }
            route_options(&inner.settings)
        };

        // creation of the interface, routes and DNS, still without the lock
        let result = match resolved {
            Ok(resolved) => bring_up(&stored.config, opts, resolved).await,
            Err(e) => Err(err(e)),
        };

        // cancelled meanwhile: undo the work done; the interface goes away when
        // the tunnel is dropped
        let mut inner = self.inner.lock().await;
        if inner.generation != generation {
            drop(inner);
            tracing::info!("connection attempt cancelled");
            if let Ok((_tunnel, _net, mut routes, mut dns)) = result {
                reset_parts(&mut routes, &mut dns).await;
            }
            return Ok(());
        }
        inner.resolving = false;
        match result {
            // success: start of the monitor and `Connected` status
            Ok((tunnel, net, routes, dns)) => {
                let (stop_tx, stop_rx) = oneshot::channel();
                tokio::spawn(self.clone().monitor(generation, tunnel.failure(), stop_rx));
                let status = Status {
                    state: ConnectionState::Connected,
                    tunnel_id: Some(id.to_owned()),
                    interface: Some(tunnel.interface_name().to_owned()),
                    peers: peer_status(tunnel.stats()),
                    ..Status::default()
                };
                let mut active = Active {
                    generation,
                    tunnel,
                    net,
                    routes,
                    dns,
                    _stop_monitor: stop_tx,
                };
                // the settings may have changed while the lock was released
                let now = route_options(&inner.settings);
                if now != opts
                    && let Err(e) = apply_routing(&mut active, now).await
                {
                    tracing::error!("reconfiguring routes failed: {e}");
                }
                inner.active = Some(active);
                self.set_status(&mut inner, status);
                self.sync_protection(&mut inner).await;
                Ok(())
            }
            // failure: `Failed` status, the kill switch stays as configured
            Err(message) => {
                tracing::error!("connect failed: {message}");
                inner.endpoints.clear();
                self.set_status(
                    &mut inner,
                    Status {
                        state: ConnectionState::Failed,
                        tunnel_id: Some(id.to_owned()),
                        error: Some(message.clone()),
                        ..Status::default()
                    },
                );
                self.sync_protection(&mut inner).await;
                Err(message)
            }
        }
    }

    /// Disconnects the running tunnel and clears a previous failure. Also cancels
    /// a connection attempt in progress.
    pub async fn disconnect(&self) {
        let mut inner = self.inner.lock().await;
        inner.generation += 1;
        inner.blocked_after_drop = false;
        inner.resolving = false;
        inner.endpoints.clear();
        if let Some(active) = inner.active.take() {
            let tunnel_id = inner.status.tunnel_id.clone();
            self.set_status(
                &mut inner,
                Status {
                    state: ConnectionState::Disconnecting,
                    tunnel_id,
                    ..Status::default()
                },
            );
            teardown(active).await;
        }
        // also clears a previous failure
        self.set_status(&mut inner, Status::default());
        self.sync_protection(&mut inner).await;
    }

    /// Stops the tunnel on daemon shutdown. The firewall is left as it is, so
    /// an enabled kill switch keeps protecting until the daemon comes back.
    pub async fn shutdown(&self) {
        let mut inner = self.inner.lock().await;
        inner.generation += 1;
        if let Some(active) = inner.active.take() {
            teardown(active).await;
        }
        // the firewall is reset only if the kill switch would not block after
        // the restart anyway
        let blocking = inner.settings.kill_switch != KillSwitch::Off
            && (self.store.connected_tunnel().is_some()
                || inner.settings.kill_switch == KillSwitch::Always);
        if !blocking {
            let _ = inner.firewall.reset().await;
        }
        let _ = inner.split.stop().await;
    }

    /// Publishes live statistics and reacts to the tunnel failing. Runs until
    /// `stop` is dropped or the tunnel of `generation` is no longer the active
    /// one.
    async fn monitor(
        self: Arc<Self>,
        generation: u64,
        mut failure: watch::Receiver<Option<String>>,
        mut stop: oneshot::Receiver<()>,
    ) {
        let mut tick = tokio::time::interval(STATS_INTERVAL);
        loop {
            tokio::select! {
                _ = &mut stop => return,
                _ = failure.changed() => {
                    let reason = failure.borrow().clone().unwrap_or_default();
                    self.on_failure(generation, reason).await;
                    return;
                }
                _ = tick.tick() => {
                    // refresh of the peer statistics and check for a network change
                    let mut inner = self.inner.lock().await;
                    let Some(active) = inner.active.as_ref().filter(|a| a.generation == generation) else {
                        return;
                    };
                    let mut status = inner.status.clone();
                    status.peers = peer_status(active.tunnel.stats());
                    self.set_status(&mut inner, status);
                    let inner = &mut *inner;
                    follow_network_change(inner.active.as_mut().expect("checked above"), &mut inner.split).await;
                }
            }
        }
    }

    /// Handles the failure of a running tunnel: tears it down and reports
    /// `Failed`. With the `on_connect` kill switch traffic stays blocked until the
    /// user disconnects. Ignored if the tunnel of `generation` is gone already.
    async fn on_failure(&self, generation: u64, reason: String) {
        let mut inner = self.inner.lock().await;
        if inner
            .active
            .as_ref()
            .is_none_or(|a| a.generation != generation)
        {
            return;
        }
        tracing::error!("tunnel failed: {reason}");
        let active = inner.active.take().expect("checked above");
        teardown(active).await;
        inner.endpoints.clear();
        inner.blocked_after_drop = inner.settings.kill_switch == KillSwitch::OnConnect;
        let tunnel_id = inner.status.tunnel_id.clone();
        self.set_status(
            &mut inner,
            Status {
                state: ConnectionState::Failed,
                tunnel_id,
                error: Some(reason),
                ..Status::default()
            },
        );
        self.sync_protection(&mut inner).await;
    }

    /// Applies the firewall and split tunneling for the current state, and updates
    /// `blocked` and `protection_error` in the status. Errors are reported in the
    /// status, not returned.
    async fn sync_protection(&self, inner: &mut Inner) {
        // whether the kill switch blocks traffic outside the tunnel right now
        let state = inner.status.state;
        let block = match inner.settings.kill_switch {
            KillSwitch::Off => false,
            KillSwitch::OnConnect => {
                matches!(
                    state,
                    ConnectionState::Connecting | ConnectionState::Connected
                ) || inner.blocked_after_drop
            }
            KillSwitch::Always => true,
        };
        // firewall policy: DNS is allowed only while resolving the endpoints;
        // DNS leaks are blocked when every app uses a tunnel with a default route
        // and its own DNS servers
        let split = split_mode(&inner.settings);
        let policy = FirewallPolicy {
            block,
            allow_lan: inner.settings.allow_lan,
            tunnel_if: inner.active.as_ref().map(|a| a.net.if_name.clone()),
            tunnel_index: inner.active.as_ref().map(|a| a.net.if_index),
            endpoints: inner.endpoints.clone(),
            allow_dns: block && inner.resolving,
            block_dns_leaks: inner.active.as_ref().is_some_and(|a| {
                split != SplitMode::Include
                    && !a.net.dns_servers.is_empty()
                    && a.net.allowed_ips.iter().any(|n| n.prefix_len() == 0)
            }),
            split,
            tunnel_dns: inner
                .active
                .as_ref()
                .map(|a| a.net.dns_servers.clone())
                .unwrap_or_default(),
            split_apps: inner
                .settings
                .split_apps
                .iter()
                .map(|a| PathBuf::from(&a.path))
                .collect(),
        };

        let mut problems = Vec::new();
        if let Err(e) = inner.firewall.apply(&policy).await {
            tracing::error!("firewall: {e}");
            problems.push(format!("firewall: {e}"));
        }
        // the cgroup is needed even without a tunnel: in include mode the
        // kill switch must be able to block the chosen apps
        let split_result = if split == SplitMode::Off {
            inner.split.stop().await
        } else {
            let apps: Vec<PathBuf> = inner
                .settings
                .split_apps
                .iter()
                .map(|a| PathBuf::from(&a.path))
                .collect();
            let tunnel = inner.active.as_ref().map(|a| a.net.clone());
            inner.split.start(split, &apps, tunnel.as_ref()).await
        };
        if let Err(e) = split_result {
            tracing::error!("split tunnel: {e}");
            problems.push(format!("split tunnel: {e}"));
        }

        // traffic is reported as blocked only when the kill switch has no
        // tunnel to let it through
        let mut status = inner.status.clone();
        status.blocked = block && state != ConnectionState::Connected;
        status.protection_error = (!problems.is_empty()).then(|| problems.join("; "));
        self.publish(inner, status);
    }

    /// Replaces the connection part of the status. `blocked` and
    /// `protection_error` belong to `sync_protection`, which callers run next.
    fn set_status(&self, inner: &mut Inner, mut status: Status) {
        status.blocked = inner.status.blocked;
        status.protection_error = inner.status.protection_error.clone();
        self.publish(inner, status);
    }

    /// Stores the status and notifies the clients, only if it changed.
    fn publish(&self, inner: &mut Inner, status: Status) {
        if inner.status != status {
            inner.status = status.clone();
            let _ = self.events.send(Event::StatusChanged(status));
        }
    }
}

/// Split tunneling mode in effect: `Off` when no app is listed.
fn split_mode(settings: &Settings) -> SplitMode {
    match settings.split_mode {
        _ if settings.split_apps.is_empty() => SplitMode::Off,
        SplitTunnelMode::Off => SplitMode::Off,
        SplitTunnelMode::Include => SplitMode::Include,
        SplitTunnelMode::Exclude => SplitMode::Exclude,
    }
}

/// Routing options derived from the settings.
fn route_options(settings: &Settings) -> RouteOptions {
    RouteOptions {
        split: split_mode(settings),
        prefer_tunnel: settings.prefer_tunnel,
    }
}

/// Builds the response to an import or update, with the parser warnings.
fn imported(id: String, name: &str, parsed: submarine_config::Parsed) -> Response {
    let tunnel = submarine_ipc::TunnelInfo::new(id, name.trim().to_owned(), &parsed.config);
    let warnings = parsed
        .warnings
        .into_iter()
        .map(|w| format!("line {}: {}", w.line, w.message))
        .collect();
    Response::Imported { tunnel, warnings }
}

/// Checks the settings sent by a client: the auto-connect tunnel must exist and
/// the split tunneling apps must be at most `MAX_SPLIT_APPS`, with absolute paths.
fn validate(store: &TunnelStore, settings: &Settings) -> Result<()> {
    if let Some(id) = &settings.auto_connect
        && store.get(id).is_err()
    {
        return Err("the tunnel to connect at startup does not exist".into());
    }
    if settings.split_apps.len() > MAX_SPLIT_APPS {
        return Err(format!("at most {MAX_SPLIT_APPS} apps can be listed"));
    }
    for app in &settings.split_apps {
        if !std::path::Path::new(&app.path).is_absolute() {
            return Err(format!("app path must be absolute: {}", app.path));
        }
    }
    Ok(())
}

/// Starts the tunnel and applies its routes and DNS.
///
/// `endpoints`: resolved address of each peer, in the order of the configuration.
/// On a routing or DNS error what was applied is undone; the tunnel is dropped,
/// removing the interface.
async fn bring_up(
    config: &TunnelConfig,
    opts: RouteOptions,
    endpoints: Vec<Option<SocketAddr>>,
) -> Result<(Tunnel, TunnelNetConfig, RouteManager, DnsManager)> {
    let options = TunnelOptions {
        name: Some(INTERFACE_NAME.into()),
        fwmark: Some(FWMARK),
        endpoints: Some(endpoints),
    };
    let tunnel = Tunnel::start(config, options).await.map_err(err)?;
    // network parameters of the interface just created
    let net = TunnelNetConfig {
        if_name: tunnel.interface_name().to_owned(),
        if_index: tunnel.interface_index(),
        addresses: config.interface.addresses.clone(),
        allowed_ips: config
            .peers
            .iter()
            .flat_map(|p| p.allowed_ips.iter().copied())
            .collect(),
        endpoints: tunnel.endpoints().to_vec(),
        dns_servers: config.interface.dns_servers.clone(),
        dns_search: config.interface.dns_search.clone(),
    };
    let mut routes = RouteManager::new().map_err(err)?;
    let mut dns = DnsManager::new();
    if let Err(e) = apply_parts(&mut routes, &mut dns, &net, opts).await {
        let _ = dns.reset().await;
        let _ = routes.reset().await;
        return Err(e);
    }
    Ok((tunnel, net, routes, dns))
}

/// Applies routes and DNS of the running tunnel with new options.
async fn apply_routing(active: &mut Active, opts: RouteOptions) -> Result<()> {
    apply_parts(&mut active.routes, &mut active.dns, &active.net, opts).await
}

/// Applies routes, then DNS (or restores the system DNS in include mode).
async fn apply_parts(
    routes: &mut RouteManager,
    dns: &mut DnsManager,
    net: &TunnelNetConfig,
    opts: RouteOptions,
) -> Result<()> {
    routes.apply(net, opts).await.map_err(err)?;
    // in include mode the system keeps its own DNS: only the chosen apps use
    // the tunnel, the rest of the system must still resolve names
    if opts.split == SplitMode::Include {
        dns.reset().await.map_err(err)
    } else {
        dns.apply(net).await.map_err(err)
    }
}

/// Keeps the encrypted traffic on the current physical network. `refresh` adapts
/// the routes where needed; only on Windows it returns the index of the new
/// physical interface, and then the tunnel socket and split tunneling are moved
/// to it.
async fn follow_network_change(active: &mut Active, split: &mut SplitTunnel) {
    match active.routes.refresh(&active.net).await {
        Ok(Some(index)) => {
            if let Err(e) = active.tunnel.repin(index) {
                tracing::warn!("cannot move the tunnel to the new network: {e}");
            }
            if let Err(e) = split.network_changed().await {
                tracing::warn!("cannot update split tunneling for the new network: {e}");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("network change check failed: {e}"),
    }
}

/// Undoes DNS and routes of a tunnel, then drops it.
async fn teardown(mut active: Active) {
    reset_parts(&mut active.routes, &mut active.dns).await;
    // dropping the tunnel removes the interface and its routes
}

/// Restores DNS and routes; errors are only logged.
async fn reset_parts(routes: &mut RouteManager, dns: &mut DnsManager) {
    if let Err(e) = dns.reset().await {
        tracing::warn!("dns reset failed: {e}");
    }
    if let Err(e) = routes.reset().await {
        tracing::warn!("route reset failed: {e}");
    }
}

/// Converts the tunnel statistics to the IPC form (handshake as Unix seconds).
fn peer_status(stats: Vec<PeerStats>) -> Vec<PeerStatus> {
    stats
        .into_iter()
        .map(|s| PeerStatus {
            public_key: s.public_key.to_string(),
            endpoint: s.endpoint.map(|e| e.to_string()),
            last_handshake: s
                .last_handshake
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
            rx_bytes: s.rx_bytes,
            tx_bytes: s.tx_bytes,
        })
        .collect()
}

/// Converts an error to the message for the client.
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
