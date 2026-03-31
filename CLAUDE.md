# Ergo Proxy Node

Lightweight Ergo P2P relay — forwards messages between peers without maintaining blockchain state. First IPv6-enabled Ergo network endpoint.

## What This Is

A message-forwarding proxy for the Ergo P2P network. It connects to Ergo nodes as a peer, accepts inbound connections from other peers, and relays P2P messages between them. No blockchain state, no UTXO validation, no mining — pure message routing.

## Why

- **IPv6 bridge**: Current Ergo network is IPv4-only. This proxy listens on IPv6, enabling BlockHost VMs with IPv6-only connectivity to reach the Ergo network.
- **Connection stability**: Ergo testnet has very few peers that aggressively rate-limit connections. A persistent relay reduces connection churn.
- **BlockHost integration**: BlockHost instances connect to one relay instead of each fighting for seed peer connections. The relay runs on a broker-allocated IPv6 address, dogfooding the full stack.

## Base Package

Built on top of `ergo-relay` at `~/projects/blockhost-ergo/ergo-relay/`.

The existing codebase (1130 lines of Rust) provides:
- **`src/p2p.rs`** (597 lines) — Complete Ergo P2P protocol: handshake (v6), message framing, GetPeers/Peers exchange, Inv/ModifierRequest/ModifierResponse for tx broadcast, `.testing-mode` auto-detection
- **`src/main.rs`** (341 lines) — HTTP signing service (`/wallet/transaction/sign`) and P2P broadcast endpoint (`/transactions`)
- **`src/peers.rs`** (192 lines) — Peer discovery with persistent peer list, exponential backoff, automatic retry

Key capabilities already implemented:
- Ergo P2P handshake (protocol v6.0.1, VLQ timestamp, session feature)
- Message framing (magic bytes, checksums, VLQ lengths)
- Network auto-detection from `/etc/blockhost/.testing-mode`
- Peer discovery and persistent peer list (`/var/lib/blockhost/ergo-peers.json`)
- Transaction broadcast via Inv → ModifierRequest → ModifierResponse

## What Needs Building

### 1. Inbound listener
Accept TCP connections from other Ergo nodes/clients. Listen on configurable address (including `[::]:port` for IPv6). Perform handshake as responder (currently only initiator logic exists).

### 2. Persistent outbound connections
Maintain long-lived connections to known peers instead of connect-per-operation. Reconnect on drop. Track connection state per peer.

### 3. Message forwarding
Route messages between connected peers:
- Inv received from peer A → forward to all other connected peers
- ModifierRequest from peer B → forward to whichever peer has the data
- ModifierResponse → forward to the requester
- GetPeers/Peers → respond with known peer list

### 4. Dual-stack listening
Listen on both IPv4 and IPv6. This is the core value proposition — bridge IPv6 clients to the IPv4 Ergo network.

## Architecture

```
                    ┌─────────────────────┐
                    │   Ergo Proxy Node   │
                    │                     │
  IPv6 clients ───▶│  Inbound listener   │
  (BlockHost VMs)  │  ([::]:{port})      │
                    │                     │
                    │  Message router     │
                    │                     │
  IPv4 Ergo ◀──────│  Outbound conns     │
  network          │  (persistent)       │
                    └─────────────────────┘
```

## Language & Dependencies

**Rust** — same as ergo-relay. Share the `p2p.rs` module directly (copy or workspace member).

Key crates:
- **`tokio`** — async runtime for managing multiple concurrent connections (ergo-relay uses sync/threads, but a relay with many connections needs async)
- **`ergo-lib`**, **`ergotree-ir`**, **`ergo-chain-types`** — only if the proxy needs to parse tx content (probably not — it can forward opaque bytes)
- **`serde`**, **`serde_json`** — config, peer list
- **`blake2`** — message checksums (already in p2p.rs)

Alternatively, if staying sync (simpler, fewer deps): use `std::net::TcpListener` + `std::thread::spawn` per connection. This works fine for <50 concurrent connections, which is realistic for a proxy.

## Config

```toml
# ergo-proxy.toml
[listen]
address = "[::]:9030"       # dual-stack IPv6+IPv4
max_inbound = 20

[network]
# Auto-detected from /etc/blockhost/.testing-mode, or set explicitly
# type = "testnet"

[outbound]
min_peers = 3               # maintain at least this many outbound connections
max_peers = 10
```

## Key Design Decisions

- **No blockchain state** — the proxy never validates transactions or blocks. It forwards what it receives. Invalid messages are the sender's problem.
- **No signing** — that stays in ergo-relay. This is purely a network relay.
- **Stateless forwarding** — no mempool, no block index. Just route messages by type.
- **Presence as state** — if the proxy is running and has peers, it's working. No config beyond the listen address.

## Relationship to ergo-relay

ergo-relay stays as-is — it's the signing + broadcast service for BlockHost engines. The proxy node is a separate binary that provides network-level relay. They can run on the same machine or different machines.

On a BlockHost system:
- `ergo-relay` runs locally for signing
- `ergo-proxy-node` runs on a broker-allocated IPv6 address as a shared relay
- ergo-relay's peer list points to the proxy node

## Testing

The testnet is the test environment. Deploy the proxy, point a BlockHost VM's ergo-relay at it, submit a transaction, verify it reaches the network.

Success criteria: a transaction signed by ergo-relay, broadcast through the proxy to an IPv4 testnet peer, appears in the explorer.

## Existing Non-JVM Components (Reuse Before Building)

The Rust/non-JVM ecosystem has substantial coverage of Ergo protocol components. Check these before writing anything from scratch.

### Available in Rust

| Component | Package | Crate/Repo |
|-----------|---------|------------|
| ErgoTree interpreter | sigma-rust | `ergotree-interpreter` |
| Transaction signing | sigma-rust | `ergo-lib` (multi-secret, coin selection, tx building) |
| Box/UTXO primitives | sigma-rust | `ergo-lib` (Box, Register, Token, JSON node API compat) |
| AVL+ authenticated tree | ergo_avltree_rust | `ergo_avltree_rust` (prover + verifier, matches JVM reference) |
| P2P protocol | ergo-relay | `~/projects/blockhost-ergo/ergo-relay/src/p2p.rs` (handshake v6, messages, peer discovery) |
| Tx/block serialization | sigma-rust | `ergo-chain-types` |
| Encoding/hashing utils | ergo-utilities-rust | `ergoplatform/ergo-utilities-rust` |
| Node REST API client | ergo-node-interface-rust | `ergoplatform/ergo-node-interface-rust` |
| NiPoPoW proofs | sigma-rust | Referenced, light client bootstrapping |
| WASM bindings | sigma-rust | `ergo-lib-wasm` (browser + Node.js) |

### Not Yet Available in Rust (JVM Only)

| Component | JVM Location | Notes |
|-----------|-------------|-------|
| Block validation | `ergo-core` sub-module | Consensus rules, header chain validation |
| UTXO set state management | `ergo` node | Apply block → update AVL tree, rollback support |
| Mempool | `ergo` node | Tx ordering, eviction, double-spend detection |
| Difficulty adjustment (Autolykos2) | `ergo` node | Algorithm documented, needs Rust impl for validation |
| Storage rent enforcement | `ergo` node | Collection logic (min box value exists in sigma-rust) |
| Chain sync state machine | `ergo` node | Full/digest/UTXO-snapshot modes |
| PoW verification | `ergo-core` / mining software | Autolykos2 verify may exist in mining pools |

### Key Insight

For the proxy node (message forwarding only), NONE of the "missing" components are needed. The proxy doesn't validate — it forwards. All the Rust pieces for P2P messaging are already in ergo-relay.

For a future lightweight validating node, the gap is block validation + UTXO state management. The AVL tree is ported, sigma-rust can evaluate scripts, but the orchestration layer (apply block, check consensus, manage state) would need to be built. The JVM `ergo-core` sub-module is the reference for this — it's designed to be consumed independently of the full node.

### JVM ergo-core Sub-Module

The JVM `ergo` repo (`ergoplatform/ergo`) has a modular structure:
- `avldb` — authenticated AVL+ tree persistence
- `ergo-core` — SPV-level primitives: P2P messages, PoW, NiPoPoW, header validation
- `ergo-wallet` — transaction signing and verification

`ergo-core` is the most relevant for porting — it contains the consensus rules without the full node runtime.
