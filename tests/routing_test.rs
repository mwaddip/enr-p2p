use enr_p2p::routing::inv_table::InvTable;
use enr_p2p::routing::tracker::{RequestTracker, SyncTracker};
use enr_p2p::types::PeerId;

// --- Inv table tests ---

#[test]
fn inv_table_record_and_lookup() {
    let mut table = InvTable::new();
    let id = [0xaa; 32];
    table.record(id, PeerId(1));
    assert_eq!(table.lookup(&id), Some(PeerId(1)));
}

#[test]
fn inv_table_lookup_missing() {
    let table = InvTable::new();
    assert_eq!(table.lookup(&[0xbb; 32]), None);
}

#[test]
fn inv_table_latest_announcer_wins() {
    let mut table = InvTable::new();
    let id = [0xaa; 32];
    table.record(id, PeerId(1));
    table.record(id, PeerId(2));
    assert_eq!(table.lookup(&id), Some(PeerId(2)));
}

#[test]
fn inv_table_purge_peer() {
    let mut table = InvTable::new();
    table.record([0xaa; 32], PeerId(1));
    table.record([0xbb; 32], PeerId(1));
    table.record([0xcc; 32], PeerId(2));

    table.purge_peer(PeerId(1));

    assert_eq!(table.lookup(&[0xaa; 32]), None);
    assert_eq!(table.lookup(&[0xbb; 32]), None);
    assert_eq!(table.lookup(&[0xcc; 32]), Some(PeerId(2)));
}

#[test]
fn inv_table_invariant_no_disconnected_peers() {
    let mut table = InvTable::new();
    for i in 0..100 {
        let mut id = [0u8; 32];
        id[0] = i;
        table.record(id, PeerId(1));
    }
    table.purge_peer(PeerId(1));
    assert!(table.is_empty());
}

// --- Request tracker tests ---

#[test]
fn request_tracker_record_and_lookup() {
    let mut tracker = RequestTracker::new();
    let id = [0xaa; 32];
    tracker.record(id, PeerId(5));
    assert_eq!(tracker.lookup(&id), Some(PeerId(5)));
}

#[test]
fn request_tracker_fulfill_removes_entry() {
    let mut tracker = RequestTracker::new();
    let id = [0xaa; 32];
    tracker.record(id, PeerId(5));
    let requester = tracker.fulfill(&id);
    assert_eq!(requester, Some(PeerId(5)));
    assert_eq!(tracker.lookup(&id), None);
}

#[test]
fn request_tracker_fulfill_missing() {
    let mut tracker = RequestTracker::new();
    assert_eq!(tracker.fulfill(&[0xbb; 32]), None);
}

#[test]
fn request_tracker_purge_peer() {
    let mut tracker = RequestTracker::new();
    tracker.record([0xaa; 32], PeerId(1));
    tracker.record([0xbb; 32], PeerId(2));
    tracker.record([0xcc; 32], PeerId(1));

    tracker.purge_peer(PeerId(1));

    assert_eq!(tracker.lookup(&[0xaa; 32]), None);
    assert_eq!(tracker.lookup(&[0xbb; 32]), Some(PeerId(2)));
    assert_eq!(tracker.lookup(&[0xcc; 32]), None);
}

// --- Sync tracker tests ---

#[test]
fn sync_tracker_pair_and_lookup() {
    let mut tracker = SyncTracker::new();
    tracker.pair(PeerId(1), PeerId(10));
    assert_eq!(tracker.outbound_for(&PeerId(1)), Some(PeerId(10)));
    assert_eq!(tracker.inbound_for(&PeerId(10)), Some(PeerId(1)));
}

#[test]
fn sync_tracker_purge_inbound() {
    let mut tracker = SyncTracker::new();
    tracker.pair(PeerId(1), PeerId(10));
    tracker.purge_peer(PeerId(1));
    assert_eq!(tracker.outbound_for(&PeerId(1)), None);
    assert_eq!(tracker.inbound_for(&PeerId(10)), None);
}

#[test]
fn sync_tracker_purge_outbound() {
    let mut tracker = SyncTracker::new();
    tracker.pair(PeerId(1), PeerId(10));
    tracker.purge_peer(PeerId(10));
    assert_eq!(tracker.outbound_for(&PeerId(1)), None);
    assert_eq!(tracker.inbound_for(&PeerId(10)), None);
}

// --- Router tests ---

use enr_p2p::routing::router::{Router, Action};
use enr_p2p::protocol::messages::ProtocolMessage;
use enr_p2p::protocol::peer::ProtocolEvent;
use enr_p2p::types::{Direction, ProxyMode};

#[test]
fn router_inv_from_outbound_forwards_to_inbound() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Full);
    router.register_peer(PeerId(3), Direction::Inbound, ProxyMode::Full);

    let event = ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::Inv { modifier_type: 2, ids: vec![[0xaa; 32]] },
    };

    let actions = router.handle_event(event);
    let targets: Vec<PeerId> = actions.iter().filter_map(|a| match a {
        Action::Send { target, .. } => Some(*target),
        _ => None,
    }).collect();
    assert!(targets.contains(&PeerId(2)));
    assert!(targets.contains(&PeerId(3)));
    assert!(!targets.contains(&PeerId(1)));
}

#[test]
fn router_modifier_request_routes_via_inv_table() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Full);

    router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::Inv { modifier_type: 2, ids: vec![[0xaa; 32]] },
    });

    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(2),
        message: ProtocolMessage::ModifierRequest { modifier_type: 2, ids: vec![[0xaa; 32]] },
    });

    let targets: Vec<PeerId> = actions.iter().filter_map(|a| match a {
        Action::Send { target, .. } => Some(*target),
        _ => None,
    }).collect();
    assert_eq!(targets, vec![PeerId(1)]);
}

#[test]
fn router_modifier_response_routes_to_requester() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Full);

    router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::Inv { modifier_type: 2, ids: vec![[0xaa; 32]] },
    });
    router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(2),
        message: ProtocolMessage::ModifierRequest { modifier_type: 2, ids: vec![[0xaa; 32]] },
    });

    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::ModifierResponse {
            modifier_type: 2,
            modifiers: vec![([0xaa; 32], vec![1, 2, 3])],
        },
    });

    let targets: Vec<PeerId> = actions.iter().filter_map(|a| match a {
        Action::Send { target, .. } => Some(*target),
        _ => None,
    }).collect();
    assert_eq!(targets, vec![PeerId(2)]);
}

#[test]
fn router_get_peers_handled_directly() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);

    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::GetPeers,
    });

    assert!(actions.iter().any(|a| matches!(a, Action::Send { target, message }
        if *target == PeerId(1) && matches!(message, ProtocolMessage::Peers { .. })
    )));
}

#[test]
fn router_light_mode_drops_sync_info() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Light);

    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(2),
        message: ProtocolMessage::SyncInfo { body: vec![1, 2, 3] },
    });
    assert!(actions.is_empty());
}

#[test]
fn router_full_mode_forwards_sync_info() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Full);

    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(2),
        message: ProtocolMessage::SyncInfo { body: vec![1, 2, 3] },
    });
    assert!(!actions.is_empty());
}

#[test]
fn router_peer_disconnect_purges_state() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Full);

    router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::Inv { modifier_type: 2, ids: vec![[0xaa; 32]] },
    });

    router.handle_event(ProtocolEvent::PeerDisconnected {
        peer_id: PeerId(1),
        reason: "gone".into(),
    });

    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(2),
        message: ProtocolMessage::ModifierRequest { modifier_type: 2, ids: vec![[0xaa; 32]] },
    });
    assert!(actions.is_empty());
}

// --- Action::Validate tests ---

#[test]
fn modifier_response_emits_validate_action() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Full);

    // Peer 2 requests via inv route
    router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::Inv { modifier_type: 1, ids: vec![[0xaa; 32]] },
    });
    router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(2),
        message: ProtocolMessage::ModifierRequest { modifier_type: 1, ids: vec![[0xaa; 32]] },
    });

    // Response arrives
    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::ModifierResponse {
            modifier_type: 1,
            modifiers: vec![([0xaa; 32], vec![1, 2, 3])],
        },
    });

    // Should have both Validate and Send
    let has_validate = actions.iter().any(|a| matches!(a, Action::Validate { .. }));
    let has_send = actions.iter().any(|a| matches!(a, Action::Send { .. }));
    assert!(has_validate, "should emit Action::Validate");
    assert!(has_send, "should emit Action::Send to requester");
}

// --- ModifierRequest fallback routing tests ---

#[test]
fn router_modifier_request_no_inv_falls_back_to_outbound() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Full);

    // No Inv — peer 2 requests a modifier the router has never seen
    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(2),
        message: ProtocolMessage::ModifierRequest { modifier_type: 1, ids: vec![[0xaa; 32]] },
    });

    // Should fall back to outbound peer 1
    let targets: Vec<PeerId> = actions.iter().filter_map(|a| match a {
        Action::Send { target, .. } => Some(*target),
        _ => None,
    }).collect();
    assert_eq!(targets, vec![PeerId(1)]);
}

#[test]
fn router_modifier_request_no_inv_no_outbound_drops() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Inbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Full);

    // No outbound peers at all, no inv entry
    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::ModifierRequest { modifier_type: 1, ids: vec![[0xaa; 32]] },
    });

    assert!(actions.is_empty(), "no outbound peers means no fallback target");
}

#[test]
fn router_modifier_request_with_inv_ignores_fallback() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(3), Direction::Inbound, ProxyMode::Full);

    // Peer 2 announces, so inv table maps to peer 2
    router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(2),
        message: ProtocolMessage::Inv { modifier_type: 1, ids: vec![[0xaa; 32]] },
    });

    // Peer 3 requests — should route to peer 2 (inv hit), not peer 1 (fallback)
    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(3),
        message: ProtocolMessage::ModifierRequest { modifier_type: 1, ids: vec![[0xaa; 32]] },
    });

    let targets: Vec<PeerId> = actions.iter().filter_map(|a| match a {
        Action::Send { target, .. } => Some(*target),
        _ => None,
    }).collect();
    assert_eq!(targets, vec![PeerId(2)], "inv table target should be used, not fallback");
}

#[test]
fn router_fallback_request_response_reaches_requester() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Full);

    // No inv — fallback routes request to peer 1
    router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(2),
        message: ProtocolMessage::ModifierRequest { modifier_type: 1, ids: vec![[0xaa; 32]] },
    });

    // Peer 1 responds — should route back to peer 2 (the original requester)
    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::ModifierResponse {
            modifier_type: 1,
            modifiers: vec![([0xaa; 32], vec![1, 2, 3])],
        },
    });

    let targets: Vec<PeerId> = actions.iter().filter_map(|a| match a {
        Action::Send { target, .. } => Some(*target),
        _ => None,
    }).collect();
    assert_eq!(targets, vec![PeerId(2)], "response should route back to original requester");
}
