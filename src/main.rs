mod config;
mod types;
mod transport;
mod protocol;
mod routing;

use config::Config;
use protocol::messages::ProtocolMessage;
use protocol::peer::ProtocolEvent;
use routing::router::{Action, Router};
use transport::connection::Connection;
use transport::frame::Frame;
use transport::handshake::{self, HandshakeConfig};
use types::{Direction, Network, PeerId, ProxyMode, Version};

use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration};

type PeerSender = mpsc::Sender<Frame>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ergo-proxy.toml".to_string());

    let config = Config::load(&config_path)?;
    let (ver_major, ver_minor, ver_patch) = config.version_bytes()?;
    let version = Version::new(ver_major, ver_minor, ver_patch);
    let network = config.proxy.network;

    tracing::info!(network = ?network, version = %version, "Ergo proxy node starting");

    let (event_tx, mut event_rx) = mpsc::channel::<ProtocolEvent>(256);
    let peer_senders: Arc<Mutex<HashMap<PeerId, PeerSender>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let router = Arc::new(Mutex::new(Router::new()));
    let peer_counter = Arc::new(std::sync::atomic::AtomicU64::new(1));

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

    // RRD latency stats: update every 30 seconds
    {
        let router = router.clone();
        tokio::spawn(async move {
            // Create RRD if rrdtool is available
            let rrd_path = "/var/lib/ergo-proxy/latency.rrd";
            let rrd_available = tokio::process::Command::new("rrdtool")
                .arg("info")
                .arg(rrd_path)
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false);

            if !rrd_available {
                // Try to create the RRD file
                let create_result = tokio::process::Command::new("rrdtool")
                    .args([
                        "create", rrd_path,
                        "--step", "30",
                        "DS:min:GAUGE:60:0:U",
                        "DS:avg:GAUGE:60:0:U",
                        "DS:max:GAUGE:60:0:U",
                        "DS:peers:GAUGE:60:0:U",
                        "RRA:AVERAGE:0.5:1:2880",    // 30s resolution, 24 hours
                        "RRA:AVERAGE:0.5:10:8640",    // 5min resolution, 30 days
                        "RRA:AVERAGE:0.5:120:8760",   // 1hr resolution, 1 year
                        "RRA:MAX:0.5:1:2880",
                        "RRA:MAX:0.5:10:8640",
                        "RRA:LAST:0.5:1:2880",        // last value, for peer count
                        "RRA:LAST:0.5:10:8640",
                    ])
                    .output()
                    .await;

                match create_result {
                    Ok(o) if o.status.success() => {
                        tracing::info!("Created RRD at {}", rrd_path);
                    }
                    Ok(o) => {
                        tracing::warn!("rrdtool create failed: {}", String::from_utf8_lossy(&o.stderr));
                        return;
                    }
                    Err(_) => {
                        tracing::info!("rrdtool not available, latency RRD disabled");
                        return;
                    }
                }
            }

            let mut ticker = interval(Duration::from_secs(30));
            loop {
                ticker.tick().await;
                let r = router.lock().await;
                let peer_count = r.outbound_peers().len();
                let update = match r.latency_stats() {
                    Some(stats) => format!(
                        "N:{:.1}:{:.1}:{:.1}:{}",
                        stats.min_ms, stats.avg_ms, stats.max_ms, stats.peer_count
                    ),
                    None => format!("N:U:U:U:{}", peer_count),
                };
                drop(r);
                let _ = tokio::process::Command::new("rrdtool")
                    .args(["update", rrd_path, &update])
                    .output()
                    .await;
            }
        });
    }

    // Main event loop
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
                tracing::info!("All event senders dropped, shutting down");
                break;
            }
        }
    }

    Ok(())
}

fn make_handshake_config(
    identity: &config::IdentityConfig,
    version: Version,
    network: Network,
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
            if let Err(e) = transport::frame::write_frame(&mut writer, &magic, &frame).await {
                tracing::warn!(peer = %peer_id, error = %e, "Write failed");
                break;
            }
        }
    });

    // Reader loop
    loop {
        match transport::frame::read_frame(&mut reader, &magic).await {
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
