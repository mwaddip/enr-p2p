# ergo-proxy-node

A lightweight Ergo P2P relay. Connects to the Ergo network as a peer, accepts inbound connections, and forwards messages between them. No blockchain state, no validation, no mining.

## What it does

- Speaks the Ergo P2P protocol (handshake, message framing, peer exchange)
- Maintains persistent outbound connections to Ergo nodes
- Accepts inbound connections from other peers
- Routes messages between them using tracked Inv-based forwarding
- Listens on IPv6, bridging IPv6-only hosts to the IPv4 Ergo network

## What it doesn't do

- Store blocks, headers, or UTXO state
- Validate transactions or consensus rules
- Mine or participate in block production
- Replace a full node — it's a relay, not a node

If a peer asks for data the proxy doesn't have, the request is forwarded to an outbound peer that announced it. The proxy never generates or validates content — it passes bytes.

## Why

The Ergo network is IPv4-only. This proxy enables IPv6 hosts to participate by relaying traffic to IPv4 peers. It also reduces connection churn on testnets with few peers and aggressive rate-limiting.

## Architecture

Three layers, each with a documented [Design by Contract](contracts/) boundary:

```
Routing    — Inv table, request tracking, mode filtering, forwarding decisions
Protocol   — Message parsing, peer lifecycle state machine
Transport  — TCP streams, frame encoding, handshake, checksum verification
```

Messages flow up from transport (raw frames) through protocol (typed messages) to routing (forwarding decisions), and back down as outgoing frames.

### Proxy modes

Each listener can operate in one of two modes:

- **Full** — forwards everything: Inv, modifiers, SyncInfo, peer exchange. Advertises as a full archival node. An inbound peer can fully sync through the proxy.
- **Light** — gossip only: Inv relay, transaction broadcast, peer exchange. Advertises as a NiPoPoW-bootstrapped node. Peers won't ask for blocks or headers.

The intended setup: IPv6 listener in full mode (so IPv6 peers get a complete endpoint), IPv4 listener in light mode (IPv4 peers can already reach real nodes directly).

## Building

```
cargo build --release
```

### Debian package

```
./build-deb
sudo dpkg -i target/ergo-proxy-node_*.deb
```

The .deb:
- Runs as an unprivileged `ergo-proxy` system user
- Installs a hardened systemd service
- Auto-detects fail2ban and enables the rate-limiting jail
- If fail2ban is installed later, `dpkg-reconfigure ergo-proxy-node` picks it up

## Configuration

```toml
[proxy]
network = "testnet"    # or "mainnet"

# Dual-stack: [::] accepts both IPv6 and IPv4
[listen.ipv6]
address = "[::]:9030"
mode = "full"
max_inbound = 20

[outbound]
min_peers = 3
max_peers = 10
seed_peers = ["213.239.193.208:9023"]

[identity]
agent_name = "ergo-proxy"
peer_name = "ergo-proxy-node"
protocol_version = "6.0.3"
```

## Design philosophy

**Forward, don't interpret.** The proxy treats message bodies as opaque bytes wherever possible. SyncInfo is forwarded without parsing. Unknown message codes are relayed, not dropped. This makes the proxy resilient to protocol changes without code updates.

**Design by Contract.** Every layer boundary has explicit preconditions, postconditions, and invariants documented in [`contracts/`](contracts/) and enforced via `debug_assert!`. The Inv table never references disconnected peers. The request tracker is always consistent. If an invariant breaks, it breaks loudly in development.

**The wire is the spec.** The Ergo P2P protocol has no formal specification — the JVM reference node is the de facto standard. The protocol implementation was verified against pcap captures of actual JVM node handshakes, not documentation. If the docs and the bytes disagree, follow the bytes.

**Minimal dependencies.** The proxy needs tokio, blake2, serde, and toml. No Ergo-specific crates — it doesn't parse transactions, evaluate scripts, or touch the blockchain. If it can be forwarded as raw bytes, it is.

**Foundation code.** The transport and protocol layers are designed to be reusable in a future non-JVM Ergo node implementation. Correctness and clean boundaries matter more than shipping fast.

## Protocol notes

- Ergo P2P uses the [Scorex](https://github.com/ScorexFoundation/scorex-util) serialization framework. All integer fields (feature body lengths, address ports, signed values) are VLQ-encoded, not fixed-width. This is not documented anywhere — it was discovered by comparing our output against pcap captures.
- The handshake includes a Mode feature (id=16) advertising node capabilities, and a Session feature (id=3) with network magic bytes and a session ID.
- Peers below protocol version 4.0.100 (EIP-37) are rejected. This is enforced by the network.

## Rate limiting

Inbound rate limiting is handled by fail2ban, not in-process. The proxy logs tagged events (`F2B_REJECT`, `F2B_HANDSHAKE_FAIL`) with the offender's IP. fail2ban watches the log and bans repeat offenders at the firewall level. The shipped jail config: 5 violations in 10 minutes = 1 hour ban.

## Contributing

Contributions are welcome. Some areas that would benefit from work:

- **Peer discovery persistence** — save discovered peers to disk for faster restarts
- **GetPeers response** — currently returns an empty peer list; should serialize the known peer registry
- **Metrics** — peer count, messages forwarded, bytes relayed
- **Connection deduplication** — detect and reject duplicate connections from the same peer
- **Live testing on mainnet** — the proxy has been tested on testnet; mainnet testing would be valuable

If you're interested in Ergo protocol internals or building non-JVM Ergo infrastructure, this is a good place to start. The codebase is small (~1500 lines), the layers are well-separated, and the contracts tell you exactly what each component promises.

## License

MIT
