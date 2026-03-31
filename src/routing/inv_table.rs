//! Inv table: maps modifier IDs to the peer that announced them.
//!
//! # Contract
//! - `record(id, peer)`: associates a modifier ID with a peer.
//!   Postcondition: `lookup(id) == Some(peer)`.
//! - `lookup(id)`: returns the peer that announced the modifier, or None.
//! - `purge_peer(peer)`: removes all entries for a peer.
//!   Postcondition: no entry maps to the purged peer.
//! - Invariant: the table never contains entries for disconnected peers
//!   (enforced by caller invoking `purge_peer` on disconnect).

use crate::types::{ModifierId, PeerId};
use std::collections::HashMap;

pub struct InvTable {
    entries: HashMap<ModifierId, PeerId>,
}

impl InvTable {
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    pub fn record(&mut self, modifier_id: ModifierId, peer: PeerId) {
        self.entries.insert(modifier_id, peer);
    }

    pub fn lookup(&self, modifier_id: &ModifierId) -> Option<PeerId> {
        self.entries.get(modifier_id).copied()
    }

    pub fn purge_peer(&mut self, peer: PeerId) {
        self.entries.retain(|_, p| *p != peer);

        #[cfg(debug_assertions)]
        self.check_invariant_no_peer(peer);
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(debug_assertions)]
    fn check_invariant_no_peer(&self, peer: PeerId) {
        debug_assert!(
            !self.entries.values().any(|p| *p == peer),
            "Invariant violated: Inv table still contains entries for purged peer {}",
            peer
        );
    }
}
