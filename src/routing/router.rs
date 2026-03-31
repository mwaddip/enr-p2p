//! Message routing: forwarding decisions, mode filtering, peer registry.
//!
//! # Contract
//! - `handle_event`: given a `ProtocolEvent`, returns a list of `Action`s.
//!   Precondition: peer IDs in events are registered (or being disconnected).
//!   Postcondition: actions target only registered, non-disconnected peers.
//! - `register_peer` / peer removal on disconnect: manage the peer registry.
//! - Invariant: Inv table, request tracker, and sync tracker are consistent with
//!   the peer registry — no references to unregistered peers.

use crate::protocol::messages::ProtocolMessage;
use crate::protocol::peer::ProtocolEvent;
use crate::routing::inv_table::InvTable;
use crate::routing::latency::{LatencyTracker, LatencyStats};
use crate::routing::tracker::{RequestTracker, SyncTracker};
use crate::types::{Direction, PeerId, ProxyMode};
use std::collections::HashMap;

/// A routing directive: send a message to a specific peer.
#[derive(Debug)]
pub enum Action {
    Send {
        target: PeerId,
        message: ProtocolMessage,
    },
}

struct PeerEntry {
    direction: Direction,
    mode: ProxyMode,
}

pub struct Router {
    peers: HashMap<PeerId, PeerEntry>,
    inv_table: InvTable,
    request_tracker: RequestTracker,
    sync_tracker: SyncTracker,
    latency_tracker: LatencyTracker,
}

impl Router {
    pub fn new() -> Self {
        Self {
            peers: HashMap::new(),
            inv_table: InvTable::new(),
            request_tracker: RequestTracker::new(),
            sync_tracker: SyncTracker::new(),
            latency_tracker: LatencyTracker::new(),
        }
    }

    pub fn register_peer(&mut self, peer_id: PeerId, direction: Direction, mode: ProxyMode) {
        self.peers.insert(peer_id, PeerEntry { direction, mode });
    }

    pub fn handle_event(&mut self, event: ProtocolEvent) -> Vec<Action> {
        match event {
            ProtocolEvent::PeerConnected { .. } => {
                vec![]
            }

            ProtocolEvent::PeerDisconnected { peer_id, .. } => {
                self.inv_table.purge_peer(peer_id);
                self.request_tracker.purge_peer(peer_id);
                self.sync_tracker.purge_peer(peer_id);
                self.latency_tracker.purge_peer(peer_id);
                self.peers.remove(&peer_id);
                vec![]
            }

            ProtocolEvent::Message { peer_id, message } => {
                self.route_message(peer_id, message)
            }
        }
    }

    fn route_message(&mut self, source: PeerId, message: ProtocolMessage) -> Vec<Action> {
        let source_entry = match self.peers.get(&source) {
            Some(e) => e,
            None => return vec![],
        };
        let source_direction = source_entry.direction;
        let source_mode = source_entry.mode;

        match message {
            ProtocolMessage::Inv { modifier_type, ids } => {
                for id in &ids {
                    self.inv_table.record(*id, source);
                }

                let target_direction = match source_direction {
                    Direction::Outbound => Direction::Inbound,
                    Direction::Inbound => Direction::Outbound,
                };

                self.peers.iter()
                    .filter(|(pid, entry)| **pid != source && entry.direction == target_direction)
                    .map(|(pid, _)| Action::Send {
                        target: *pid,
                        message: ProtocolMessage::Inv {
                            modifier_type,
                            ids: ids.clone(),
                        },
                    })
                    .collect()
            }

            ProtocolMessage::ModifierRequest { modifier_type, ids } => {
                let mut actions = Vec::new();
                for id in &ids {
                    if let Some(target) = self.inv_table.lookup(id) {
                        if target != source {
                            self.request_tracker.record(*id, source);
                            self.latency_tracker.record_request(*id, target);
                            actions.push(Action::Send {
                                target,
                                message: ProtocolMessage::ModifierRequest {
                                    modifier_type,
                                    ids: vec![*id],
                                },
                            });
                        }
                    }
                }
                actions
            }

            ProtocolMessage::ModifierResponse { modifier_type, modifiers } => {
                let mut actions = Vec::new();
                for (id, data) in &modifiers {
                    self.latency_tracker.record_response(id);
                    if let Some(requester) = self.request_tracker.fulfill(id) {
                        actions.push(Action::Send {
                            target: requester,
                            message: ProtocolMessage::ModifierResponse {
                                modifier_type,
                                modifiers: vec![(*id, data.clone())],
                            },
                        });
                    }
                }
                actions
            }

            ProtocolMessage::SyncInfo { body } => {
                if source_mode == ProxyMode::Light {
                    return vec![];
                }

                match source_direction {
                    Direction::Inbound => {
                        if let Some((&outbound_id, _)) = self.peers.iter()
                            .find(|(pid, entry)| {
                                entry.direction == Direction::Outbound
                                    && self.sync_tracker.inbound_for(pid).is_none()
                            })
                        {
                            self.sync_tracker.pair(source, outbound_id);
                            vec![Action::Send {
                                target: outbound_id,
                                message: ProtocolMessage::SyncInfo { body },
                            }]
                        } else {
                            vec![]
                        }
                    }
                    Direction::Outbound => {
                        if let Some(inbound) = self.sync_tracker.inbound_for(&source) {
                            vec![Action::Send {
                                target: inbound,
                                message: ProtocolMessage::SyncInfo { body },
                            }]
                        } else {
                            vec![]
                        }
                    }
                }
            }

            ProtocolMessage::GetPeers => {
                vec![Action::Send {
                    target: source,
                    message: ProtocolMessage::Peers { body: vec![] },
                }]
            }

            ProtocolMessage::Peers { .. } => {
                vec![]
            }

            ProtocolMessage::Unknown { code, body } => {
                let target_direction = match source_direction {
                    Direction::Outbound => Direction::Inbound,
                    Direction::Inbound => Direction::Outbound,
                };

                self.peers.iter()
                    .filter(|(pid, entry)| **pid != source && entry.direction == target_direction)
                    .map(|(pid, _)| Action::Send {
                        target: *pid,
                        message: ProtocolMessage::Unknown { code, body: body.clone() },
                    })
                    .collect()
            }
        }
    }

    pub fn outbound_peers(&self) -> Vec<PeerId> {
        self.peers.iter()
            .filter(|(_, e)| e.direction == Direction::Outbound)
            .map(|(pid, _)| *pid)
            .collect()
    }

    pub fn inbound_peers(&self) -> Vec<PeerId> {
        self.peers.iter()
            .filter(|(_, e)| e.direction == Direction::Inbound)
            .map(|(pid, _)| *pid)
            .collect()
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    pub fn latency_stats(&self) -> Option<LatencyStats> {
        self.latency_tracker.stats()
    }
}
