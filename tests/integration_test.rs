use ergo_proxy_node::protocol::messages::ProtocolMessage;
use ergo_proxy_node::protocol::peer::ProtocolEvent;
use ergo_proxy_node::routing::router::{Action, Router};
use ergo_proxy_node::types::{Direction, PeerId, ProxyMode};

#[test]
fn full_tx_relay_scenario() {
    let mut router = Router::new();
    let outbound = PeerId(1);
    let inbound = PeerId(2);
    router.register_peer(outbound, Direction::Outbound, ProxyMode::Full);
    router.register_peer(inbound, Direction::Inbound, ProxyMode::Full);

    let tx_id = [0x42; 32];

    // Outbound announces tx
    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: outbound,
        message: ProtocolMessage::Inv { modifier_type: 2, ids: vec![tx_id] },
    });
    assert_eq!(actions.len(), 1);
    assert!(matches!(&actions[0], Action::Send { target, .. } if *target == inbound));

    // Inbound requests it
    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: inbound,
        message: ProtocolMessage::ModifierRequest { modifier_type: 2, ids: vec![tx_id] },
    });
    assert_eq!(actions.len(), 1);
    assert!(matches!(&actions[0], Action::Send { target, .. } if *target == outbound));

    // Outbound delivers
    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: outbound,
        message: ProtocolMessage::ModifierResponse {
            modifier_type: 2,
            modifiers: vec![(tx_id, vec![0xde, 0xad, 0xbe, 0xef])],
        },
    });
    assert_eq!(actions.len(), 1);
    assert!(matches!(&actions[0], Action::Send { target, .. } if *target == inbound));
}

#[test]
fn disconnect_cleanup_scenario() {
    let mut router = Router::new();
    let out1 = PeerId(1);
    let out2 = PeerId(2);
    let inb = PeerId(3);
    router.register_peer(out1, Direction::Outbound, ProxyMode::Full);
    router.register_peer(out2, Direction::Outbound, ProxyMode::Full);
    router.register_peer(inb, Direction::Inbound, ProxyMode::Full);

    let tx_id = [0x55; 32];

    router.handle_event(ProtocolEvent::Message {
        peer_id: out1,
        message: ProtocolMessage::Inv { modifier_type: 2, ids: vec![tx_id] },
    });

    router.handle_event(ProtocolEvent::PeerDisconnected {
        peer_id: out1,
        reason: "gone".into(),
    });

    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: inb,
        message: ProtocolMessage::ModifierRequest { modifier_type: 2, ids: vec![tx_id] },
    });
    assert!(actions.is_empty());
}

#[test]
fn inv_fanout_scenario() {
    let mut router = Router::new();
    let outbound = PeerId(1);
    router.register_peer(outbound, Direction::Outbound, ProxyMode::Full);

    for i in 2..=5 {
        router.register_peer(PeerId(i), Direction::Inbound, ProxyMode::Full);
    }

    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: outbound,
        message: ProtocolMessage::Inv { modifier_type: 2, ids: vec![[0xaa; 32]] },
    });

    assert_eq!(actions.len(), 4);
}

#[test]
fn light_mode_blocks_sync() {
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
fn full_mode_sync_flow() {
    let mut router = Router::new();
    router.register_peer(PeerId(1), Direction::Outbound, ProxyMode::Full);
    router.register_peer(PeerId(2), Direction::Inbound, ProxyMode::Full);

    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(2),
        message: ProtocolMessage::SyncInfo { body: vec![1, 2, 3] },
    });
    assert_eq!(actions.len(), 1);
    assert!(matches!(&actions[0], Action::Send { target, .. } if *target == PeerId(1)));

    let actions = router.handle_event(ProtocolEvent::Message {
        peer_id: PeerId(1),
        message: ProtocolMessage::SyncInfo { body: vec![4, 5, 6] },
    });
    assert_eq!(actions.len(), 1);
    assert!(matches!(&actions[0], Action::Send { target, .. } if *target == PeerId(2)));
}
