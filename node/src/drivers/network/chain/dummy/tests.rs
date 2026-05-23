//! Translation of `chain/dummy/inmem_dummy_test.go`.
//!
//! `TestInmemDummyAppSide` / `TestInmemDummyServerSide` are reproduced; the
//! socket dummy tests are dropped along with the socket proxies.

use std::thread;
use std::time::Duration;

use crate::drivers::network::chain::crypto;
use crate::drivers::network::chain::hashgraph::{
    Block, InternalTransaction, TransactionType,
};
use crate::drivers::network::chain::node::state::State as NodeState;
use crate::drivers::network::chain::peers::Peer;
use crate::drivers::network::chain::proxy::AppProxy;
use crate::logrus::Entry;

use super::InmemDummyClient;

// Translation of `inmem_dummy_test.go::TestInmemDummyAppSide`.
#[test]
fn inmem_dummy_app_side() {
    let dummy = InmemDummyClient::new(Entry::standalone());
    let submit_rx = dummy.proxy.submit_ch();
    let tx = b"the test transaction".to_vec();
    let tx_clone = tx.clone();

    let handle = thread::spawn(move || {
        let received = submit_rx
            .recv_timeout(Duration::from_millis(200))
            .expect("submit timeout");
        assert_eq!(received, tx_clone);
    });

    dummy.proxy.submit_tx(&tx).unwrap();
    handle.join().unwrap();
}

// Translation of `inmem_dummy_test.go::TestInmemDummyServerSide`.
#[test]
fn inmem_dummy_server_side() {
    let dummy = InmemDummyClient::new(Entry::standalone());

    let blocks: Vec<Block> = (0..5)
        .map(|i| {
            Block::new(
                i,
                i + 1,
                Vec::new(),
                Vec::new(),
                vec![format!("block {} transaction", i).into_bytes()],
                vec![
                    InternalTransaction::new(
                        TransactionType::PeerAdd,
                        Peer::new("node0", "paris", ""),
                    ),
                    InternalTransaction::new(
                        TransactionType::PeerRemove,
                        Peer::new("node1", "london", ""),
                    ),
                ],
                0,
            )
        })
        .collect();

    // Commit the first block and verify the state hash.
    let resp = dummy
        .proxy
        .commit_block(blocks[0].clone())
        .expect("commit block 0");

    let mut expected_state_hash: Vec<u8> = Vec::new();
    for tx in blocks[0].transactions() {
        let tx_hash = crypto::sha256(tx);
        expected_state_hash =
            crypto::simple_hash_from_two_hashes(&expected_state_hash, &tx_hash);
    }
    assert_eq!(resp.state_hash, expected_state_hash);

    let snapshot = dummy
        .proxy
        .get_snapshot(blocks[0].index())
        .expect("snapshot");
    assert_eq!(snapshot, expected_state_hash);

    // Commit the rest, then restore to block 0.
    for b in blocks.iter().skip(1) {
        dummy.proxy.commit_block(b.clone()).expect("commit");
    }

    dummy.proxy.restore(&snapshot).expect("restore");

    // The restored state hash should equal block 0's.
    let resp = dummy
        .proxy
        .commit_block(blocks[0].clone())
        .expect("recommit block 0 after restore");
    // After restore + recommit, the state hash should fold block 0's
    // transactions onto the restored hash.
    let mut hash = expected_state_hash.clone();
    for tx in blocks[0].transactions() {
        hash = crypto::simple_hash_from_two_hashes(&hash, &crypto::sha256(tx));
    }
    assert_eq!(resp.state_hash, hash);

    dummy
        .proxy
        .on_state_changed(NodeState::Babbling)
        .expect("state change");
}
