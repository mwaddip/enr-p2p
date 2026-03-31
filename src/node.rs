//! P2P node entry point.
//!
//! The caller provides a config and an optional modifier validator, then calls
//! `P2pNode::start()`. The P2P layer spawns listeners, outbound connections,
//! and the event loop as background tokio tasks. The returned `P2pNode` is a
//! handle for observing state — the caller owns the tokio runtime.

use crate::config::Config;
use crate::protocol::messages::ProtocolMessage;
use crate::protocol::peer::ProtocolEvent;
use crate::routing::router::{Action, Router};
use crate::routing::validator::ModifierValidator;
use crate::routing::latency::LatencyStats;
use crate::transport::connection::Connection;
use crate::transport::frame::Frame;
use crate::transport::handshake::{self, HandshakeConfig};
use crate::types::{Direction, PeerId, ProxyMode, Version};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

type PeerSender = mpsc::Sender<Frame>;

/// Handle to a running P2P node.
///
/// Created by `P2pNode::start()`. Provides read-only observation of the
/// node's state. The P2P layer runs as background tokio tasks — dropping
/// this handle does not stop them. The tasks live until the tokio runtime
/// shuts down.
pub struct P2pNode {
    router: Arc<Mutex<Router>>,
}

impl P2pNode {
    /// Start the P2P layer.
    ///
    /// Loads config, sets up listeners, outbound connections, keepalive, and
    /// the event loop as background tokio tasks. Returns immediately.
    ///
    /// # Contract
    /// - **Precondition**: Called within a tokio runtime.
    /// - **Precondition**: `config` has at least one listener and one seed peer
    ///   (enforced by `Config::load()`).
    /// - **Postcondition**: Background tasks are spawned and running.
    pub async fn start(
        config: Config,
        validator: Option<Box<dyn ModifierValidator>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (ver_major, ver_minor, ver_patch) = config.version_bytes()?;
        let version = Version::new(ver_major, ver_minor, ver_patch);
        let network = config.proxy.network;

        tracing::info!(network = ?network, version = %version, "P2P layer starting");

        let (event_tx, event_rx) = mpsc::channel::<ProtocolEvent>(256);
        let peer_senders: Arc<Mutex<HashMap<PeerId, PeerSender>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let router = Arc::new(Mutex::new(Router::new()));
        let peer_counter = Arc::new(std::sync::atomic::AtomicU64::new(1));

        if let Some(v) = validator {
            router.lock().await.set_validator(v);
        }

        // Start listeners
        if let Some(ref listener_cfg) = config.listen.ipv6 {
            let listener = TcpListener::bind(listener_cfg.address).await?;
            tracing::info!(addr = %listener_cfg.address, mode = ?listener_cfg.mode, "IPv6 listener started");
            let hs_config = make_handshake_config(&config.identity, version, network, listener_cfg.mode);
            tokio::spawn(accept_loop(
                listener, hs_config, listener_cfg.mode, listener_cfg.max_inbound,
                event_tx.clone(), peer_senders.clone(), router.clone(), peer_counter.clone(),
            ));
        }

        if let Some(ref listener_cfg) = config.listen.ipv4 {
            let listener = TcpListener::bind(listener_cfg.address).await?;
            tracing::info!(addr = %listener_cfg.address, mode = ?listener_cfg.mode, "IPv4 listener started");
            let hs_config = make_handshake_config(&config.identity, version, network, listener_cfg.mode);
            tokio::spawn(accept_loop(
                listener, hs_config, listener_cfg.mode, listener_cfg.max_inbound,
                event_tx.clone(), peer_senders.clone(), router.clone(), peer_counter.clone(),
            ));
        }

        // Start outbound connections
        {
            let hs_config = make_handshake_config(&config.identity, version, network, ProxyMode::Full);
            tokio::spawn(outbound_manager(
                config.outbound.seed_peers.clone(), config.outbound.min_peers,
                hs_config, ProxyMode::Full,
                event_tx.clone(), peer_senders.clone(), router.clone(), peer_counter.clone(),
            ));
        }

        // Keepalive: send GetPeers every 2 minutes
        {
            let router = router.clone();
            let peer_senders = peer_senders.clone();
            tokio::spawn(async move {
                let mut ticker = interval(Duration::from_secs(120));
                loop {
                    ticker.tick().await;
                    let outbound = router.lock().await.outbound_peers();
                    let senders = peer_senders.lock().await;
                    let frame = ProtocolMessage::GetPeers.to_frame();
                    for pid in outbound {
                        if let Some(tx) = senders.get(&pid) {
                            let _ = tx.send(frame.clone()).await;
                        }
                    }
                }
            });
        }

        // Event loop: process protocol events through the router
        {
            let router = router.clone();
            let peer_senders = peer_senders.clone();
            tokio::spawn(async move {
                event_loop(event_rx, router, peer_senders).await;
            });
        }

        Ok(P2pNode { router })
    }

    /// Number of connected peers (inbound + outbound).
    pub async fn peer_count(&self) -> usize {
        self.router.lock().await.peer_count()
    }

    /// Currently connected outbound peer IDs.
    pub async fn outbound_peers(&self) -> Vec<PeerId> {
        self.router.lock().await.outbound_peers()
    }

    /// Currently connected inbound peer IDs.
    pub async fn inbound_peers(&self) -> Vec<PeerId> {
        self.router.lock().await.inbound_peers()
    }

    /// Aggregate latency statistics across all tracked peers.
    pub async fn latency_stats(&self) -> Option<LatencyStats> {
        self.router.lock().await.latency_stats()
    }
}

async fn event_loop(
    mut event_rx: mpsc::Receiver<ProtocolEvent>,
    router: Arc<Mutex<Router>>,
    peer_senders: Arc<Mutex<HashMap<PeerId, PeerSender>>>,
) {
    loop {
        match event_rx.recv().await {
            Some(event) => {
                let actions = router.lock().await.handle_event(event);
                let senders = peer_senders.lock().await;
                for action in actions {
                    match action {
                        Action::Send { target, message } => {
                            if let Some(tx) = senders.get(&target) {
                                let frame = message.to_frame();
                                if tx.send(frame).await.is_err() {
                                    tracing::warn!(peer = %target, "Failed to send to peer");
                                }
                            }
                        }
                    }
                }
            }
            None => {
                tracing::info!("All event senders dropped, event loop exiting");
                break;
            }
        }
    }
}

async fn run_peer(
    peer_id: PeerId,
    conn: Connection,
    direction: Direction,
    mode: ProxyMode,
    event_tx: mpsc::Sender<ProtocolEvent>,
    peer_senders: Arc<Mutex<HashMap<PeerId, PeerSender>>>,
    router: Arc<Mutex<Router>>,
) {
    let spec = conn.peer_spec().clone();
    tracing::info!(
        peer = %peer_id,
        name = %spec.name,
        agent = %spec.agent,
        version = %spec.version,
        direction = ?direction,
        "Peer active"
    );

    // Register peer in router
    router.lock().await.register_peer(peer_id, direction, mode);

    // Send PeerConnected event
    let _ = event_tx.send(ProtocolEvent::PeerConnected {
        peer_id,
        spec: spec.clone(),
        direction,
    }).await;

    // Split connection for concurrent read/write
    let (mut reader, mut writer, magic, _) = conn.split();

    // Create write channel
    let (write_tx, mut write_rx) = mpsc::channel::<Frame>(64);
    peer_senders.lock().await.insert(peer_id, write_tx);

    // Writer task
    let write_handle = tokio::spawn(async move {
        while let Some(frame) = write_rx.recv().await {
            if let Err(e) = crate::transport::frame::write_frame(&mut writer, &magic, &frame).await {
                tracing::warn!(peer = %peer_id, error = %e, "Write failed");
                break;
            }
        }
    });

    // Reader loop
    loop {
        match crate::transport::frame::read_frame(&mut reader, &magic).await {
            Ok(frame) => {
                match ProtocolMessage::from_frame(&frame) {
                    Ok(msg) => {
                        let event = ProtocolEvent::Message { peer_id, message: msg };
                        if event_tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(peer = %peer_id, error = %e, "Message parse failed");
                    }
                }
            }
            Err(e) => {
                tracing::info!(peer = %peer_id, error = %e, "Connection lost");
                break;
            }
        }
    }

    // Cleanup
    peer_senders.lock().await.remove(&peer_id);
    write_handle.abort();

    let _ = event_tx.send(ProtocolEvent::PeerDisconnected {
        peer_id,
        reason: "connection closed".into(),
    }).await;

    tracing::info!(peer = %peer_id, "Peer removed");
}

fn make_handshake_config(
    identity: &crate::config::IdentityConfig,
    version: Version,
    network: crate::types::Network,
    mode: ProxyMode,
) -> HandshakeConfig {
    HandshakeConfig {
        agent_name: identity.agent_name.clone(),
        peer_name: identity.peer_name.clone(),
        version,
        network,
        mode,
        declared_address: None,
    }
}

async fn accept_loop(
    listener: TcpListener,
    hs_config: HandshakeConfig,
    mode: ProxyMode,
    max_inbound: usize,
    event_tx: mpsc::Sender<ProtocolEvent>,
    peer_senders: Arc<Mutex<HashMap<PeerId, PeerSender>>>,
    router: Arc<Mutex<Router>>,
    peer_counter: Arc<std::sync::atomic::AtomicU64>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let remote_ip = addr.ip();
                let inbound_count = router.lock().await.inbound_peers().len();
                if inbound_count >= max_inbound {
                    tracing::warn!(ip = %remote_ip, "F2B_REJECT connection limit exceeded");
                    continue;
                }

                let peer_id = PeerId(peer_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
                tracing::info!(peer = %peer_id, ip = %remote_ip, "Inbound connection");

                let hs = HandshakeConfig {
                    agent_name: hs_config.agent_name.clone(),
                    peer_name: hs_config.peer_name.clone(),
                    version: hs_config.version,
                    network: hs_config.network,
                    mode: hs_config.mode,
                    declared_address: hs_config.declared_address,
                };
                let event_tx = event_tx.clone();
                let peer_senders = peer_senders.clone();
                let router = router.clone();

                tokio::spawn(async move {
                    match Connection::inbound(stream, &hs).await {
                        Ok(conn) => {
                            run_peer(peer_id, conn, Direction::Inbound, mode, event_tx, peer_senders, router).await;
                        }
                        Err(e) => {
                            tracing::warn!(ip = %remote_ip, error = %e, "F2B_HANDSHAKE_FAIL bad handshake");
                        }
                    }
                });
            }
            Err(e) => {
                tracing::error!(error = %e, "Accept failed");
            }
        }
    }
}

async fn outbound_manager(
    seeds: Vec<std::net::SocketAddr>,
    min_peers: usize,
    hs_config: HandshakeConfig,
    mode: ProxyMode,
    event_tx: mpsc::Sender<ProtocolEvent>,
    peer_senders: Arc<Mutex<HashMap<PeerId, PeerSender>>>,
    router: Arc<Mutex<Router>>,
    peer_counter: Arc<std::sync::atomic::AtomicU64>,
) {
    let mut backoff = Duration::from_secs(5);

    loop {
        let current_outbound = router.lock().await.outbound_peers().len();
        if current_outbound < min_peers {
            for addr in &seeds {
                let current = router.lock().await.outbound_peers().len();
                if current >= min_peers {
                    break;
                }

                let addr = *addr;
                tracing::info!(addr = %addr, "Connecting to outbound peer");
                match tokio::time::timeout(
                    Duration::from_secs(10),
                    tokio::net::TcpStream::connect(addr),
                ).await {
                    Ok(Ok(stream)) => {
                        let peer_id = PeerId(peer_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
                        let hs = HandshakeConfig {
                            agent_name: hs_config.agent_name.clone(),
                            peer_name: hs_config.peer_name.clone(),
                            version: hs_config.version,
                            network: hs_config.network,
                            mode: hs_config.mode,
                            declared_address: hs_config.declared_address,
                        };
                        let event_tx = event_tx.clone();
                        let peer_senders = peer_senders.clone();
                        let router = router.clone();

                        tokio::spawn(async move {
                            match Connection::outbound(stream, &hs).await {
                                Ok(conn) => {
                                    if handshake::is_proxy(conn.peer_spec()) {
                                        tracing::info!(peer = %peer_id, addr = %addr, "Outbound peer is a proxy, skipping");
                                        return;
                                    }
                                    tracing::info!(peer = %peer_id, "Outbound handshake OK");
                                    run_peer(peer_id, conn, Direction::Outbound, mode, event_tx, peer_senders, router).await;
                                }
                                Err(e) => {
                                    tracing::warn!(peer = %peer_id, addr = %addr, error = %e, "Outbound handshake failed");
                                }
                            }
                        });

                        backoff = Duration::from_secs(5);
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(addr = %addr, error = %e, "Connect failed");
                    }
                    Err(_) => {
                        tracing::warn!(addr = %addr, "Connect timeout");
                    }
                }
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(300));
    }
}
