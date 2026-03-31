//! Request tracker and SyncInfo tracker.
//!
//! # RequestTracker Contract
//! - `record(id, peer)`: records that `peer` requested modifier `id`.
//! - `fulfill(id)`: returns and removes the requester for `id`.
//!   Postcondition: entry is removed after fulfill.
//! - `purge_peer(peer)`: removes all entries for a peer.
//!
//! # SyncTracker Contract
//! - `pair(inbound, outbound)`: records that `inbound`'s sync is handled by `outbound`.
//! - `outbound_for(inbound)`: returns the outbound peer handling this inbound peer's sync.
//! - `inbound_for(outbound)`: returns the inbound peer whose sync is handled by this outbound peer.
//! - `purge_peer(peer)`: removes any pairing involving this peer.
//! - Invariant: pairings are bidirectionally consistent — if A→B exists, B→A exists.

use crate::types::{ModifierId, PeerId};
use std::collections::HashMap;

pub struct RequestTracker {
    pending: HashMap<ModifierId, PeerId>,
}

impl RequestTracker {
    pub fn new() -> Self {
        Self { pending: HashMap::new() }
    }

    pub fn record(&mut self, modifier_id: ModifierId, requester: PeerId) {
        self.pending.insert(modifier_id, requester);
    }

    pub fn lookup(&self, modifier_id: &ModifierId) -> Option<PeerId> {
        self.pending.get(modifier_id).copied()
    }

    pub fn fulfill(&mut self, modifier_id: &ModifierId) -> Option<PeerId> {
        self.pending.remove(modifier_id)
    }

    pub fn purge_peer(&mut self, peer: PeerId) {
        self.pending.retain(|_, p| *p != peer);
    }
}

pub struct SyncTracker {
    inbound_to_outbound: HashMap<PeerId, PeerId>,
    outbound_to_inbound: HashMap<PeerId, PeerId>,
}

impl SyncTracker {
    pub fn new() -> Self {
        Self {
            inbound_to_outbound: HashMap::new(),
            outbound_to_inbound: HashMap::new(),
        }
    }

    pub fn pair(&mut self, inbound: PeerId, outbound: PeerId) {
        self.inbound_to_outbound.insert(inbound, outbound);
        self.outbound_to_inbound.insert(outbound, inbound);

        #[cfg(debug_assertions)]
        self.check_invariant();
    }

    pub fn outbound_for(&self, inbound: &PeerId) -> Option<PeerId> {
        self.inbound_to_outbound.get(inbound).copied()
    }

    pub fn inbound_for(&self, outbound: &PeerId) -> Option<PeerId> {
        self.outbound_to_inbound.get(outbound).copied()
    }

    pub fn purge_peer(&mut self, peer: PeerId) {
        if let Some(outbound) = self.inbound_to_outbound.remove(&peer) {
            self.outbound_to_inbound.remove(&outbound);
        }
        if let Some(inbound) = self.outbound_to_inbound.remove(&peer) {
            self.inbound_to_outbound.remove(&inbound);
        }

        #[cfg(debug_assertions)]
        self.check_invariant();
    }

    #[cfg(debug_assertions)]
    fn check_invariant(&self) {
        for (&inb, &outb) in &self.inbound_to_outbound {
            debug_assert_eq!(
                self.outbound_to_inbound.get(&outb),
                Some(&inb),
                "SyncTracker invariant violated: inbound {} → outbound {} but reverse missing",
                inb, outb
            );
        }
        for (&outb, &inb) in &self.outbound_to_inbound {
            debug_assert_eq!(
                self.inbound_to_outbound.get(&inb),
                Some(&outb),
                "SyncTracker invariant violated: outbound {} → inbound {} but reverse missing",
                outb, inb
            );
        }
    }
}
