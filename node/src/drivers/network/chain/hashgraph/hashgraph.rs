//! Translation of `chain/hashgraph/hashgraph.go` — the Babble distributed
//! consensus algorithm.
//!
//! The `Hashgraph` is accessed exclusively (the Go node serialised every call
//! through `coreLock`), so every method here takes `&mut self`: the six
//! memoization caches mutate on read (LRU recency) and the consensus methods
//! mutate the DAG.

use std::collections::HashMap;

use anyhow::{anyhow, Result};

use super::block::Block;
use super::caches::{Key, PendingRound, PendingRoundsCache, SigPool, TreKey};
use super::event::{
    new_coordinates_map, sort_frame_events, Event, EventBody, EventCoordinates, FrameEvent,
    WireEvent,
};
use super::frame::Frame;
use super::rocks_store::RocksDbStore;
use super::root::Root;
use super::round_info::RoundInfo;
use super::store::Store;
use crate::drivers::network::chain::common::{self, is_store, StoreErrType, LRU};
use crate::drivers::network::chain::peers::PeerSet;
use crate::logrus::Entry;

/// Determines how many `FrameEvent`s are included in a Root. It is deliberately
/// not configurable: peers using different values would produce different
/// Roots, Frames and Blocks.
pub const ROOT_DEPTH: i64 = 10;

/// The frequency of coin rounds.
pub const COIN_ROUND_FREQ: f64 = 4.0;

/// Called by the Hashgraph to commit a Block. A layer of indirection over the
/// proxy commit callback: processing the response is the `Core`'s job.
pub type InternalCommitCallback = Box<dyn Fn(&Block) -> Result<()> + Send>;

/// A no-op commit callback used for testing.
pub fn dummy_internal_commit_callback(_b: &Block) -> Result<()> {
    Ok(())
}

/// A DAG of Events with methods to extract a consensus order and map it onto a
/// blockchain.
pub struct Hashgraph {
    /// Store of Events, Rounds and Blocks.
    pub store: Box<dyn Store>,
    /// FIFO queue of Events whose consensus order is not yet determined.
    pub undetermined_events: Vec<String>,
    /// FIFO queue of Rounds which have not attained consensus yet.
    pub pending_rounds: PendingRoundsCache,
    /// Pool of Block signatures that need to be matched with Blocks.
    pub pending_signatures: SigPool,
    /// Index of the last consensus round.
    pub last_consensus_round: Option<i64>,
    /// Index of the first consensus round (only used in tests).
    pub first_consensus_round: Option<i64>,
    /// Index of the last block with enough signatures.
    pub anchor_block: Option<i64>,
    /// Rounds and events below this lower bound get special treatment.
    round_lower_bound: Option<i64>,
    /// Number of events in the round before `last_consensus_round`.
    pub last_commited_round_events: i64,
    /// Number of consensus transactions.
    pub consensus_transactions: i64,
    /// Number of loaded events that are not yet committed.
    pub pending_loaded_events: i64,
    /// Commit block callback.
    commit_callback: InternalCommitCallback,
    /// Counter used to order events in topological order (node-local).
    topological_index: i64,

    ancestor_cache: LRU<Key, bool>,
    self_ancestor_cache: LRU<Key, bool>,
    strongly_see_cache: LRU<TreKey, bool>,
    round_cache: LRU<String, i64>,
    timestamp_cache: LRU<String, i64>,
    witness_cache: LRU<String, bool>,

    logger: Entry,
}

fn set_vote(votes: &mut HashMap<String, HashMap<String, bool>>, x: &str, y: &str, vote: bool) {
    votes
        .entry(x.to_string())
        .or_default()
        .insert(y.to_string(), vote);
}

fn get_vote(votes: &HashMap<String, HashMap<String, bool>>, x: &str, y: &str) -> bool {
    votes
        .get(x)
        .and_then(|m| m.get(y))
        .copied()
        .unwrap_or(false)
}

impl Hashgraph {
    /// Instantiates a Hashgraph with an underlying data store and a commit
    /// callback.
    pub fn new(
        store: Box<dyn Store>,
        commit_callback: InternalCommitCallback,
        logger: Option<Entry>,
    ) -> Hashgraph {
        let logger = logger.unwrap_or_else(|| {
            let entry = Entry::standalone();
            entry.logger().set_level(crate::logrus::Level::Debug);
            entry
        });

        let cs = store.cache_size().max(0) as usize;
        Hashgraph {
            store,
            undetermined_events: Vec::new(),
            pending_rounds: PendingRoundsCache::new(),
            pending_signatures: SigPool::new(),
            last_consensus_round: None,
            first_consensus_round: None,
            anchor_block: None,
            round_lower_bound: None,
            last_commited_round_events: 0,
            consensus_transactions: 0,
            pending_loaded_events: 0,
            commit_callback,
            topological_index: 0,
            ancestor_cache: LRU::new(cs, None),
            self_ancestor_cache: LRU::new(cs, None),
            strongly_see_cache: LRU::new(cs, None),
            round_cache: LRU::new(cs, None),
            timestamp_cache: LRU::new(cs, None),
            witness_cache: LRU::new(cs, None),
            logger,
        }
    }

    /// Sets the initial PeerSet, which also creates the corresponding Roots and
    /// updates the Repertoire.
    pub fn init(&mut self, peer_set: std::sync::Arc<PeerSet>) -> Result<()> {
        self.store
            .set_peer_set(0, peer_set)
            .map_err(|e| anyhow!("Error setting PeerSet: {}", e))
    }

    // -----------------------------------------------------------------------
    // Private methods
    // -----------------------------------------------------------------------

    /// True if `y` is an ancestor of `x`.
    fn ancestor(&mut self, x: &str, y: &str) -> Result<bool> {
        let k = Key::new(x, y);
        if let Some(c) = self.ancestor_cache.get(&k) {
            return Ok(c);
        }
        let a = self._ancestor(x, y)?;
        self.ancestor_cache.add(k, a);
        Ok(a)
    }

    fn _ancestor(&mut self, x: &str, y: &str) -> Result<bool> {
        if x == y {
            return Ok(true);
        }
        let ex = self.store.get_event(x)?;
        let ey = self.store.get_event(y)?;
        let res = match ex.last_ancestors.get(&ey.creator()) {
            Some(entry) => entry.index >= ey.index(),
            None => false,
        };
        Ok(res)
    }

    /// True if `y` is a self-ancestor of `x`.
    fn self_ancestor(&mut self, x: &str, y: &str) -> Result<bool> {
        let k = Key::new(x, y);
        if let Some(c) = self.self_ancestor_cache.get(&k) {
            return Ok(c);
        }
        let a = self._self_ancestor(x, y)?;
        self.self_ancestor_cache.add(k, a);
        Ok(a)
    }

    fn _self_ancestor(&mut self, x: &str, y: &str) -> Result<bool> {
        if x == y {
            return Ok(true);
        }
        let ex = self.store.get_event(x)?;
        let ey = self.store.get_event(y)?;
        Ok(ex.creator() == ey.creator() && ex.index() >= ey.index())
    }

    /// True if `x` sees `y`.
    fn see(&mut self, x: &str, y: &str) -> Result<bool> {
        self.ancestor(x, y)
    }

    /// True if `x` strongly sees `y` based on the given peer-set.
    fn strongly_see(&mut self, x: &str, y: &str, peers: &PeerSet) -> Result<bool> {
        let k = TreKey::new(x, y, &peers.hex());
        if let Some(c) = self.strongly_see_cache.get(&k) {
            return Ok(c);
        }
        let ss = self._strongly_see(x, y, peers)?;
        self.strongly_see_cache.add(k, ss);
        Ok(ss)
    }

    fn _strongly_see(&mut self, x: &str, y: &str, peers: &PeerSet) -> Result<bool> {
        let ex = self.store.get_event(x)?;
        let ey = self.store.get_event(y)?;

        let mut c: i64 = 0;
        for p in peers.by_pub_key.keys() {
            let xla = ex.last_ancestors.get(p);
            let yfd = ey.first_descendants.get(p);
            if let (Some(xla), Some(yfd)) = (xla, yfd) {
                if xla.index >= yfd.index {
                    c += 1;
                }
            }
        }
        Ok(c >= peers.super_majority())
    }

    fn round(&mut self, x: &str) -> Result<i64> {
        if let Some(c) = self.round_cache.get(&x.to_string()) {
            return Ok(c);
        }
        let r = self._round(x)?;
        self.round_cache.add(x.to_string(), r);
        Ok(r)
    }

    fn _round(&mut self, x: &str) -> Result<i64> {
        let ex = self.store.get_event(x)?;

        let mut parent_round: i64 = -1;

        if !ex.self_parent().is_empty() {
            parent_round = self.round(&ex.self_parent())?;
        }

        if !ex.other_parent().is_empty() {
            let op_round = self.round(&ex.other_parent())?;
            if op_round > parent_round {
                parent_round = op_round;
            }
        }

        if parent_round == -1 {
            return Ok(0);
        }

        let mut round = parent_round;

        // Retrieve the parent round's PeerSet and count strongly-seen witnesses.
        let parent_round_obj = self.store.get_round(parent_round)?;
        let parent_round_peer_set = self.store.get_peer_set(parent_round)?;

        let mut c: i64 = 0;
        for w in parent_round_obj.witnesses() {
            if self.strongly_see(x, &w, &parent_round_peer_set)? {
                c += 1;
            }
        }

        if c >= parent_round_peer_set.super_majority() {
            round += 1;
        }

        Ok(round)
    }

    fn witness(&mut self, x: &str) -> Result<bool> {
        if let Some(c) = self.witness_cache.get(&x.to_string()) {
            return Ok(c);
        }
        let r = self._witness(x)?;
        self.witness_cache.add(x.to_string(), r);
        Ok(r)
    }

    /// True if `x` is a witness (first event of a round for its creator).
    fn _witness(&mut self, x: &str) -> Result<bool> {
        let ex = self.store.get_event(x)?;
        let x_round = self.round(x)?;

        // does the creator belong to the PeerSet?
        let peer_set = self.store.get_peer_set(x_round)?;
        if !peer_set.by_pub_key.contains_key(&ex.creator()) {
            return Ok(false);
        }

        let mut sp_round: i64 = -1;
        if !ex.self_parent().is_empty() {
            sp_round = self.round(&ex.self_parent())?;
        }

        Ok(x_round > sp_round)
    }

    fn round_received(&mut self, x: &str) -> Result<i64> {
        let ex = self.store.get_event(x)?;
        Ok(ex.round_received.unwrap_or(-1))
    }

    fn lamport_timestamp(&mut self, x: &str) -> Result<i64> {
        if let Some(c) = self.timestamp_cache.get(&x.to_string()) {
            return Ok(c);
        }
        let r = self._lamport_timestamp(x)?;
        self.timestamp_cache.add(x.to_string(), r);
        Ok(r)
    }

    fn _lamport_timestamp(&mut self, x: &str) -> Result<i64> {
        let mut plt: i64 = -1;
        let ex = self.store.get_event(x)?;

        if !ex.self_parent().is_empty() {
            plt = self.lamport_timestamp(&ex.self_parent())?;
        }

        if !ex.other_parent().is_empty() {
            let mut op_lt = i32::MIN as i64;
            if self.store.get_event(&ex.other_parent()).is_ok() {
                op_lt = self.lamport_timestamp(&ex.other_parent())?;
            }
            if op_lt > plt {
                plt = op_lt;
            }
        }

        Ok(plt + 1)
    }

    /// `round(x) - round(y)`.
    fn round_diff(&mut self, x: &str, y: &str) -> Result<i64> {
        let x_round = self
            .round(x)
            .map_err(|_| anyhow!("event {} has negative round", x))?;
        let y_round = self
            .round(y)
            .map_err(|_| anyhow!("event {} has negative round", y))?;
        Ok(x_round - y_round)
    }

    /// Checks the self-parent is the creator's last known event.
    fn check_self_parent(&self, event: &Event) -> Result<()> {
        let self_parent = event.self_parent();
        let creator = event.creator();

        match self.store.last_event_from(&creator) {
            Ok(creator_last_known) => {
                let self_parent_legit = self_parent == creator_last_known;
                if !self_parent_legit {
                    return Err(super::errors::SelfParentError::new(
                        "Self-parent not last known event by creator",
                        true,
                    )
                    .into());
                }
                Ok(())
            }
            Err(e) => {
                // First event.
                if is_store(&e, StoreErrType::Empty) && self_parent.is_empty() {
                    return Ok(());
                }
                Err(super::errors::SelfParentError::new(&e.to_string(), false).into())
            }
        }
    }

    /// Checks the other-parent is known.
    fn check_other_parent(&self, event: &Event) -> Result<()> {
        let other_parent = event.other_parent();
        if !other_parent.is_empty() && self.store.get_event(&other_parent).is_err() {
            return Err(anyhow!("Other-parent not known"));
        }
        Ok(())
    }

    /// Initializes the arrays of last-ancestors and first-descendants.
    fn init_event_coordinates(&mut self, event: &mut Event) -> Result<()> {
        event.last_ancestors = new_coordinates_map();
        event.first_descendants = new_coordinates_map();

        let self_parent = self.store.get_event(&event.self_parent());
        let other_parent = self.store.get_event(&event.other_parent());

        match (&self_parent, &other_parent) {
            (Err(_), Ok(op)) => {
                event.last_ancestors = op.last_ancestors.clone();
            }
            (Ok(sp), Err(_)) => {
                event.last_ancestors = sp.last_ancestors.clone();
            }
            (Ok(sp), Ok(op)) => {
                event.last_ancestors = sp.last_ancestors.clone();
                for (p, ola) in &op.last_ancestors {
                    match event.last_ancestors.get(p) {
                        Some(sla) if sla.index >= ola.index => {}
                        _ => {
                            event.last_ancestors.insert(
                                p.clone(),
                                EventCoordinates {
                                    index: ola.index,
                                    hash: ola.hash.clone(),
                                },
                            );
                        }
                    }
                }
            }
            (Err(_), Err(_)) => {}
        }

        event.first_descendants.insert(
            event.creator(),
            EventCoordinates {
                index: event.index(),
                hash: event.hex(),
            },
        );
        event.last_ancestors.insert(
            event.creator(),
            EventCoordinates {
                index: event.index(),
                hash: event.hex(),
            },
        );

        Ok(())
    }

    /// Updates the first-descendant of each ancestor of `event`.
    fn update_ancestor_first_descendant(&mut self, event: &Event) -> Result<()> {
        let creator = event.creator();
        let index = event.index();
        let hex = event.hex();

        let ancestor_hashes: Vec<String> = event
            .last_ancestors
            .values()
            .map(|c| c.hash.clone())
            .collect();

        for mut ah in ancestor_hashes {
            loop {
                let mut a = match self.store.get_event(&ah) {
                    Ok(a) => a,
                    Err(_) => break,
                };

                if a.first_descendants.contains_key(&creator) {
                    break;
                }

                a.first_descendants.insert(
                    creator.clone(),
                    EventCoordinates {
                        index,
                        hash: hex.clone(),
                    },
                );
                self.store.set_event(&a)?;

                // Stop at the ancestors that are witnesses.
                if let Ok(true) = self.witness(&ah) {
                    break;
                }
                ah = a.self_parent();
            }
        }
        Ok(())
    }

    fn create_frame_event(&mut self, x: &str) -> Result<FrameEvent> {
        let ev = self
            .store
            .get_event(x)
            .map_err(|_| anyhow!("FrameEvent {} not found", x))?;

        let round = self.round(x)?;
        let round_info = self.store.get_round(round)?;

        let te = round_info
            .created_events
            .get(x)
            .copied()
            .ok_or_else(|| anyhow!("round {} CreatedEvents[{}] not found", round, x))?;

        let lt = self.lamport_timestamp(x)?;

        Ok(FrameEvent {
            core: Box::new(ev),
            round,
            lamport_timestamp: lt,
            witness: te.witness,
        })
    }

    fn create_root(&mut self, participant: &str, head: &str) -> Result<Root> {
        let mut root = Root::new();

        if !head.is_empty() {
            let head_event = self.create_frame_event(head)?;
            let mut reverse_root_events: Vec<FrameEvent> = vec![head_event.clone()];

            let mut index = head_event.core.index();
            for _ in 0..ROOT_DEPTH {
                index -= 1;
                if index >= 0 {
                    match self.store.participant_event(participant, index) {
                        Ok(peh) => {
                            let rev = self.create_frame_event(&peh)?;
                            reverse_root_events.push(rev);
                        }
                        Err(_) => break,
                    }
                } else {
                    break;
                }
            }

            for rev in reverse_root_events.into_iter().rev() {
                root.insert(rev);
            }
        }

        Ok(root)
    }

    /// Sets the private wire-info fields on `event`.
    pub fn set_wire_info(&self, event: &mut Event) -> Result<()> {
        let mut self_parent_index: i64 = -1;
        let mut other_parent_creator_id: u32 = 0;
        let mut other_parent_index: i64 = -1;

        let repertoire = self.store.repertoire_by_pub_key();
        let creator = repertoire
            .get(&event.creator())
            .ok_or_else(|| anyhow!("Creator {} not found", event.creator()))?;

        if !event.self_parent().is_empty() {
            let self_parent = self.store.get_event(&event.self_parent())?;
            self_parent_index = self_parent.index();
        }

        if !event.other_parent().is_empty() {
            let other_parent = self.store.get_event(&event.other_parent())?;
            let other_parent_creator = repertoire
                .get(&other_parent.creator())
                .ok_or_else(|| anyhow!("Creator {} not found", other_parent.creator()))?;
            other_parent_creator_id = other_parent_creator.id();
            other_parent_index = other_parent.index();
        }

        event.set_wire_info(
            self_parent_index,
            other_parent_creator_id,
            other_parent_index,
            creator.id(),
        );
        Ok(())
    }

    /// Removes processed signatures from the SigPool.
    fn remove_processed_signatures(&mut self, processed_signatures: &HashMap<String, bool>) {
        for k in processed_signatures.keys() {
            self.pending_signatures.remove(k);
        }
    }

    // -----------------------------------------------------------------------
    // Public consensus methods
    // -----------------------------------------------------------------------

    /// Inserts an Event and runs the consensus methods.
    pub fn insert_event_and_run_consensus(
        &mut self,
        event: &mut Event,
        set_wire_info: bool,
    ) -> Result<()> {
        if let Err(e) = self.insert_event(event, set_wire_info) {
            if !super::errors::is_normal_self_parent_error(&e) {
                self.logger.with_error(&e).error("InsertEvent");
            }
            return Err(e);
        }
        if let Err(e) = self.divide_rounds() {
            self.logger.with_error(&e).error("DivideRounds");
            return Err(e);
        }
        if let Err(e) = self.decide_fame() {
            self.logger.with_error(&e).error("DecideFame");
            return Err(e);
        }
        if let Err(e) = self.decide_round_received() {
            self.logger.with_error(&e).error("DecideRoundReceived");
            return Err(e);
        }
        if let Err(e) = self.process_decided_rounds() {
            self.logger.with_error(&e).error("ProcessDecidedRounds");
            return Err(e);
        }
        Ok(())
    }

    /// Attempts to insert an Event in the DAG. Verifies the signature, checks
    /// the ancestors are known and prevents the introduction of forks.
    pub fn insert_event(&mut self, event: &mut Event, set_wire_info: bool) -> Result<()> {
        // verify signature
        match event.verify() {
            Ok(true) => {}
            Ok(false) => {
                self.logger
                    .with_field("event", event.hex())
                    .with_field("creator", event.creator())
                    .with_field("self_parent", event.self_parent())
                    .error("Invalid Event signature");
                return Err(anyhow!("Invalid Event signature {}", event.hex()));
            }
            Err(e) => return Err(e),
        }

        if let Err(e) = self.check_self_parent(event) {
            let entry = self
                .logger
                .with_field("event", event.hex())
                .with_field("creator", event.creator())
                .with_field("self_parent", event.self_parent())
                .with_error(&e);
            if !super::errors::is_normal_self_parent_error(&e) {
                entry.error("CheckSelfParent");
            } else {
                entry.trace("CheckSelfParent");
            }
            return Err(e);
        }

        if let Err(e) = self.check_other_parent(event) {
            self.logger
                .with_field("event", event.hex())
                .with_field("creator", event.creator())
                .with_field("other_parent", event.other_parent())
                .with_error(&e)
                .error("CheckOtherParent");
            return Err(e);
        }

        event.topological_index = self.topological_index;
        self.topological_index += 1;

        if set_wire_info {
            self.set_wire_info(event)
                .map_err(|e| anyhow!("SetWireInfo: {}", e))?;
        }

        self.init_event_coordinates(event)
            .map_err(|e| anyhow!("InitEventCoordinates: {}", e))?;

        self.store
            .set_event(event)
            .map_err(|e| anyhow!("SetEvent: {}", e))?;

        self.update_ancestor_first_descendant(event)
            .map_err(|e| anyhow!("UpdateAncestorFirstDescendant: {}", e))?;

        self.undetermined_events.push(event.hex());

        if event.is_loaded() {
            self.pending_loaded_events += 1;
        }

        for bs in event.block_signatures() {
            self.logger
                .debug(format!("Inserting pending signature {}", bs.key()));
            self.pending_signatures.add(bs.clone());
        }

        Ok(())
    }

    /// Inserts the FrameEvent's core Event without checking parents or
    /// signature, and without adding it to `undetermined_events`.
    pub fn insert_frame_event(&mut self, frame_event: &FrameEvent) -> Result<()> {
        let mut event = (*frame_event.core).clone();

        // Set caches so round, witness and timestamp won't be recalculated.
        self.round_cache.add(event.hex(), frame_event.round);
        self.witness_cache.add(event.hex(), frame_event.witness);
        self.timestamp_cache
            .add(event.hex(), frame_event.lamport_timestamp);

        // Set the event's private fields for later use.
        event.set_round(frame_event.round);
        event.set_lamport_timestamp(frame_event.lamport_timestamp);

        // Create/update the RoundInfo object in the store.
        let mut round_info = match self.store.get_round(frame_event.round) {
            Ok(ri) => ri,
            Err(e) => {
                if !is_store(&e, StoreErrType::KeyNotFound) {
                    return Err(e);
                }
                RoundInfo::new()
            }
        };
        round_info.add_created_event(&event.hex(), frame_event.witness);
        self.store.set_round(frame_event.round, &round_info)?;

        self.init_event_coordinates(&mut event)
            .map_err(|e| anyhow!("InitEventCoordinates: {}", e))?;

        self.store
            .set_event(&event)
            .map_err(|e| anyhow!("SetEvent: {}", e))?;

        self.update_ancestor_first_descendant(&event)
            .map_err(|e| anyhow!("UpdateAncestorFirstDescendant: {}", e))?;

        self.store
            .add_consensus_event(&event)
            .map_err(|_| anyhow!("AddConsensusEvent"))?;

        Ok(())
    }

    /// Assigns a Round and LamportTimestamp to Events, flags witnesses, and
    /// pushes Rounds into the PendingRounds queue if necessary.
    pub fn divide_rounds(&mut self) -> Result<()> {
        let undetermined = self.undetermined_events.clone();
        for hash in undetermined {
            let mut ev = self.store.get_event(&hash)?;
            let mut update_event = false;

            if ev.round.is_none() {
                let round_number = self.round(&hash)?;
                ev.set_round(round_number);
                update_event = true;

                let mut round_info = match self.store.get_round(round_number) {
                    Ok(ri) => ri,
                    Err(e) => {
                        if !is_store(&e, StoreErrType::KeyNotFound) {
                            return Err(e);
                        }
                        RoundInfo::new()
                    }
                };

                if !self.pending_rounds.queued(round_number)
                    && !round_info.decided
                    && (self.round_lower_bound.is_none()
                        || round_number > self.round_lower_bound.unwrap())
                {
                    self.pending_rounds.set(PendingRound {
                        index: round_number,
                        decided: false,
                    });
                }

                let witness = self.witness(&hash)?;
                round_info.add_created_event(&hash, witness);
                self.store.set_round(round_number, &round_info)?;
            }

            if ev.lamport_timestamp.is_none() {
                let lamport_timestamp = self.lamport_timestamp(&hash)?;
                ev.set_lamport_timestamp(lamport_timestamp);
                update_event = true;
            }

            if update_event {
                let _ = self.store.set_event(&ev);
            }
        }
        Ok(())
    }

    /// Decides if witnesses are famous.
    pub fn decide_fame(&mut self) -> Result<()> {
        let mut votes: HashMap<String, HashMap<String, bool>> = HashMap::new();
        let mut decided_rounds: Vec<i64> = Vec::new();

        for r in self.pending_rounds.get_ordered_pending_rounds() {
            let round_index = r.index;

            let mut r_round_info = self.store.get_round(round_index)?;
            let r_peer_set = self.store.get_peer_set(round_index)?;

            let r_witnesses = r_round_info.witnesses();
            for x in &r_witnesses {
                if r_round_info.is_decided(x) {
                    continue;
                }
                let last_round = self.store.last_round();
                'vote_loop: for j in (round_index + 1)..=last_round {
                    let j_round_info = self.store.get_round(j)?;
                    let j_peer_set = self.store.get_peer_set(j)?;

                    for y in j_round_info.witnesses() {
                        let diff = j - round_index;
                        if diff == 1 {
                            let ycx = self.see(&y, x)?;
                            set_vote(&mut votes, &y, x, ycx);
                        } else {
                            let j_prev_round_info = self.store.get_round(j - 1)?;
                            let j_prev_peer_set = self.store.get_peer_set(j - 1)?;

                            let mut ss_witnesses: Vec<String> = Vec::new();
                            for w in j_prev_round_info.witnesses() {
                                if self.strongly_see(&y, &w, &j_prev_peer_set)? {
                                    ss_witnesses.push(w);
                                }
                            }

                            let mut yays = 0i64;
                            let mut nays = 0i64;
                            for w in &ss_witnesses {
                                if get_vote(&votes, w, x) {
                                    yays += 1;
                                } else {
                                    nays += 1;
                                }
                            }
                            let mut v = false;
                            let mut t = nays;
                            if yays >= nays {
                                v = true;
                                t = yays;
                            }

                            if (diff as f64) % COIN_ROUND_FREQ > 0.0 {
                                // normal round
                                if t >= j_peer_set.super_majority() {
                                    r_round_info.set_fame(x, v);
                                    set_vote(&mut votes, &y, x, v);
                                    break 'vote_loop;
                                } else {
                                    set_vote(&mut votes, &y, x, v);
                                }
                            } else {
                                // coin round
                                if t >= j_peer_set.super_majority() {
                                    set_vote(&mut votes, &y, x, v);
                                } else {
                                    set_vote(&mut votes, &y, x, middle_bit(&y));
                                }
                            }
                        }
                    }
                }
            }

            if r_round_info.witnesses_decided(&r_peer_set) {
                decided_rounds.push(round_index);
            }
            self.store.set_round(round_index, &r_round_info)?;
        }

        self.pending_rounds.update(&decided_rounds);
        Ok(())
    }

    /// Assigns a RoundReceived to undetermined events when they reach
    /// consensus.
    pub fn decide_round_received(&mut self) -> Result<()> {
        let mut new_undetermined_events: Vec<String> = Vec::new();
        let undetermined = self.undetermined_events.clone();

        for x in undetermined {
            let mut received = false;
            let r = self.round(&x)?;
            let last_round = self.store.last_round();

            for i in (r + 1)..=last_round {
                let mut tr = match self.store.get_round(i) {
                    Ok(tr) => tr,
                    Err(_) => break,
                };
                let t_peers = self.store.get_peer_set(i)?;

                if !tr.witnesses_decided(&t_peers) {
                    if self.round_lower_bound.is_none() || self.round_lower_bound.unwrap() < i {
                        break;
                    } else {
                        continue;
                    }
                }

                let fws = tr.famous_witnesses();
                let mut s: Vec<String> = Vec::new();
                for w in &fws {
                    if self.see(w, &x)? {
                        s.push(w.clone());
                    }
                }

                if s.len() == fws.len() && s.len() as i64 >= t_peers.super_majority() {
                    received = true;

                    let mut ex = self.store.get_event(&x)?;
                    ex.set_round_received(i);
                    self.store.set_event(&ex)?;

                    tr.add_received_event(&x);
                    self.store.set_round(i, &tr)?;
                    break;
                }
            }

            if !received {
                new_undetermined_events.push(x);
            }
        }

        self.undetermined_events = new_undetermined_events;
        Ok(())
    }

    /// Takes Rounds whose witnesses are decided, computes Frames, maps them
    /// into Blocks and commits the Blocks via the commit callback.
    pub fn process_decided_rounds(&mut self) -> Result<()> {
        let mut processed_rounds: Vec<i64> = Vec::new();
        let result = self.process_decided_rounds_inner(&mut processed_rounds);
        self.pending_rounds.clean(&processed_rounds);
        result
    }

    fn process_decided_rounds_inner(&mut self, processed_rounds: &mut Vec<i64>) -> Result<()> {
        for r in self.pending_rounds.get_ordered_pending_rounds() {
            // Never process a decided round before all earlier rounds.
            if !r.decided {
                break;
            }

            let round = self.store.get_round(r.index)?;
            let frame = self
                .get_frame(r.index)
                .map_err(|e| anyhow!("Getting Frame {}: {}", r.index, e))?;

            self.logger
                .with_field("round_received", r.index)
                .with_field("witnesses", round.famous_witnesses().len())
                .with_field("created_events", round.created_events.len())
                .with_field("events", frame.events.len())
                .with_field("peers", frame.peers.len())
                .debug("Processing Decided Round");

            if !frame.events.is_empty() {
                for e in &frame.events {
                    self.store.add_consensus_event(&e.core)?;
                    self.consensus_transactions += e.core.transactions().len() as i64;
                    if e.core.is_loaded() {
                        self.pending_loaded_events -= 1;
                    }
                }

                let last_block_index = self.store.last_block_index();
                let block = Block::new_from_frame(last_block_index + 1, &frame)?;

                if !block.transactions().is_empty()
                    || !block.internal_transactions().is_empty()
                {
                    self.store.set_block(&block)?;
                    if (self.commit_callback)(&block).is_err() {
                        self.logger
                            .warn(format!("Failed to commit block {}", block.index()));
                    }
                }

                self.last_commited_round_events = frame.events.len() as i64;
            } else {
                self.logger
                    .debug(format!("No Events to commit for ConsensusRound {}", r.index));
            }

            processed_rounds.push(r.index);

            if self.last_consensus_round.is_none()
                || r.index > self.last_consensus_round.unwrap()
            {
                self.set_last_consensus_round(r.index);
            }
        }
        Ok(())
    }

    /// Computes the Frame corresponding to a round-received.
    pub fn get_frame(&mut self, round_received: i64) -> Result<Frame> {
        // Try the store first.
        match self.store.get_frame(round_received) {
            Ok(frame) => return Ok(frame),
            Err(e) => {
                if !is_store(&e, StoreErrType::KeyNotFound) {
                    return Err(e);
                }
            }
        }

        let round = self.store.get_round(round_received)?;
        let peer_set = self.store.get_peer_set(round_received)?;

        let mut events: Vec<FrameEvent> = Vec::new();
        for eh in &round.received_events {
            events.push(self.create_frame_event(eh)?);
        }
        sort_frame_events(&mut events);

        // Get/create Roots. Events are in topological order, so the first
        // event of a participant triggers Root creation.
        let mut roots: std::collections::BTreeMap<String, Root> =
            std::collections::BTreeMap::new();
        for ev in &events {
            let p = ev.core.creator();
            if !roots.contains_key(&p) {
                let sp = ev.core.self_parent();
                let r = self.create_root(&p, &sp)?;
                roots.insert(p, r);
            }
        }

        // Every participant known before round_received needs a Root.
        let repertoire = self.store.repertoire_by_pub_key();
        for (p, peer) in &repertoire {
            let (first_round, ok) = self.store.first_round(peer.id());
            if !ok || first_round > round_received {
                continue;
            }
            if !roots.contains_key(p) {
                let last_consensus_event_hash = self.store.last_consensus_event_from(p)?;
                let root = self.create_root(p, &last_consensus_event_hash)?;
                roots.insert(p.clone(), root);
            }
        }

        let all_peer_sets = self.store.get_all_peer_sets()?;
        let peer_sets: std::collections::BTreeMap<i64, Vec<_>> =
            all_peer_sets.into_iter().collect();

        // Compute the BFT timestamp.
        let mut timestamps: Vec<i64> = Vec::new();
        for fw in round.famous_witnesses() {
            let ev = self.store.get_event(&fw)?;
            timestamps.push(ev.timestamp());
        }
        let frame_timestamp = common::median(&timestamps);

        let res = Frame {
            round: round_received,
            peers: peer_set.peers.clone(),
            roots,
            events,
            peer_sets,
            timestamp: frame_timestamp,
        };

        self.store.set_frame(&res)?;
        Ok(res)
    }

    /// Maps pending signatures to known Blocks; valid signatures are appended
    /// and the AnchorBlock is updated if necessary.
    pub fn process_sig_pool(&mut self) -> Result<()> {
        self.logger
            .with_field("pending_signatures", self.pending_signatures.len())
            .debug("ProcessSigPool()");

        for (_k, bs) in self.pending_signatures.items() {
            let mut block = match self.store.get_block(bs.index) {
                Ok(b) => b,
                Err(e) => {
                    self.logger
                        .with_field("index", bs.index)
                        .with_field("msg", e)
                        .warn("Verifying Block signature. Could not fetch Block");
                    continue;
                }
            };

            let peer_set = match self.store.get_peer_set(block.round_received()) {
                Ok(ps) => ps,
                Err(e) => {
                    self.logger
                        .with_field("index", bs.index)
                        .with_field("round", block.round_received())
                        .with_field("err", e)
                        .warn("Verifying Block signature. No PeerSet for Block's Round");
                    continue;
                }
            };

            if !peer_set.by_pub_key.contains_key(&bs.validator_hex()) {
                self.logger
                    .with_field("index", bs.index)
                    .with_field("round", block.round_received())
                    .with_field("validator", bs.validator_hex())
                    .warn("Verifying Block signature. Validator does not belong to Block's PeerSet");
                continue;
            }

            let valid = block.verify(bs.clone())?;
            if !valid {
                self.logger
                    .with_field("index", bs.index)
                    .with_field("validator", bs.validator_hex())
                    .warn("Verifying Block signature. Invalid signature");
                continue;
            }

            block.set_signature(bs.clone())?;

            if let Err(e) = self.store.set_block(&block) {
                self.logger
                    .with_field("index", bs.index)
                    .with_field("msg", e)
                    .warn("Saving Block");
            }

            self.set_anchor_block(&block)?;

            self.logger.debug(format!("processed sig {}", bs.key()));
            self.pending_signatures.remove(&bs.key());
        }

        Ok(())
    }

    /// Sets the AnchorBlock index if `block` has collected enough signatures
    /// (+1/3) and is above the current AnchorBlock.
    pub fn set_anchor_block(&mut self, block: &Block) -> Result<()> {
        let peer_set = match self.store.get_peer_set(block.round_received()) {
            Ok(ps) => ps,
            Err(e) => {
                self.logger
                    .with_error(&e)
                    .error("No PeerSet for Block's Round");
                return Err(e);
            }
        };

        if (block.signatures.len() as i64) > peer_set.trust_count()
            && (self.anchor_block.is_none() || block.index() > self.anchor_block.unwrap())
        {
            self.set_anchor_block_index(block.index());
            self.logger
                .with_field("block_index", block.index())
                .with_field("signatures", block.signatures.len())
                .with_field("trustCount", peer_set.trust_count())
                .debug("Setting AnchorBlock");
        } else {
            let msg = match self.anchor_block {
                Some(ab) => ab.to_string(),
                None => "Anchor Block not set".to_string(),
            };
            self.logger
                .with_field("index", block.index())
                .with_field("sigs", block.signatures.len())
                .with_field("trust_count", peer_set.trust_count())
                .with_field("anchor_block", msg)
                .debug("Block is not a suitable Anchor");
        }

        Ok(())
    }

    /// Returns the AnchorBlock and its corresponding Frame.
    pub fn get_anchor_block_with_frame(&mut self) -> Result<(Block, Frame)> {
        let anchor = self
            .anchor_block
            .ok_or_else(|| anyhow!("No Anchor Block"))?;
        let block = self.store.get_block(anchor)?;
        let frame = self.get_frame(block.round_received())?;
        Ok((block, frame))
    }

    /// Clears the Hashgraph and resets it from a new base.
    pub fn reset(&mut self, block: &Block, frame: &Frame) -> Result<()> {
        self.last_consensus_round = None;
        self.first_consensus_round = None;
        self.anchor_block = None;

        self.undetermined_events = Vec::new();
        self.pending_rounds = PendingRoundsCache::new();
        self.pending_loaded_events = 0;
        self.topological_index = 0;

        let cs = self.store.cache_size().max(0) as usize;
        self.ancestor_cache = LRU::new(cs, None);
        self.self_ancestor_cache = LRU::new(cs, None);
        self.strongly_see_cache = LRU::new(cs, None);
        self.round_cache = LRU::new(cs, None);
        self.witness_cache = LRU::new(cs, None);

        self.store.reset(frame)?;

        let sorted_frame_events = frame.sorted_frame_events();
        for rev in &sorted_frame_events {
            self.insert_frame_event(rev)?;
        }

        self.store.set_block(block)?;
        self.set_last_consensus_round(block.round_received());
        self.set_round_lower_bound(block.round_received());

        Ok(())
    }

    /// Loads all Events from the Store's DB and feeds them to the consensus
    /// methods in topological order. WE CAN ONLY BOOTSTRAP FROM 0.
    pub fn bootstrap(&mut self) -> Result<()> {
        // Phase 1: read everything from the persistent DB.
        let mut batches: Vec<Vec<Event>> = Vec::new();
        let mut restore_maintenance = false;
        let mut has_rocks = false;

        {
            if let Some(rocks) = self.store.as_any().downcast_ref::<RocksDbStore>() {
                has_rocks = true;
                if !rocks.get_maintenance_mode() {
                    restore_maintenance = true;
                }
                rocks.set_maintenance_mode(true);

                match rocks.db_get_peer_set(0) {
                    Err(_) => {
                        self.logger.debug("No Genesis PeerSet, skip bootstrap");
                        if restore_maintenance {
                            rocks.set_maintenance_mode(false);
                        }
                        return Ok(());
                    }
                    Ok(peer_set) => {
                        // Initialize the InmemStore with the Genesis PeerSet.
                        rocks.set_peer_set(0, std::sync::Arc::new(peer_set))?;

                        let batch_size = 100i64;
                        let mut index = 0i64;
                        loop {
                            let batch =
                                rocks.db_topological_events(index * batch_size, batch_size)?;
                            let n = batch.len() as i64;
                            batches.push(batch);
                            if n < batch_size {
                                break;
                            }
                            index += 1;
                        }
                    }
                }
            }
        }

        if !has_rocks {
            return Ok(());
        }

        // Phase 2: insert the Events into the Hashgraph.
        for batch in batches {
            for mut e in batch {
                self.insert_event_and_run_consensus(&mut e, true)?;
            }
            self.process_sig_pool()?;
        }

        if restore_maintenance {
            if let Some(rocks) = self.store.as_any().downcast_ref::<RocksDbStore>() {
                rocks.set_maintenance_mode(false);
            }
        }

        Ok(())
    }

    /// Converts a `WireEvent` to an `Event` by replacing int IDs with the
    /// corresponding public keys.
    pub fn read_wire_info(&self, wevent: &WireEvent) -> Result<Event> {
        let mut self_parent = String::new();
        let mut other_parent = String::new();

        let repertoire_by_id = self.store.repertoire_by_id();
        let creator = repertoire_by_id
            .get(&wevent.body.creator_id)
            .ok_or_else(|| anyhow!("Creator {} not found", wevent.body.creator_id))?;

        let creator_bytes = common::decode_from_string(&creator.pub_key_string())?;

        if wevent.body.self_parent_index >= 0 {
            self_parent = self
                .store
                .participant_event(&creator.pub_key_string(), wevent.body.self_parent_index)?;
        }

        if wevent.body.other_parent_index >= 0 {
            let other_parent_creator = repertoire_by_id
                .get(&wevent.body.other_parent_creator_id)
                .ok_or_else(|| {
                    anyhow!("Participant {} not found", wevent.body.other_parent_creator_id)
                })?;
            other_parent = self
                .store
                .participant_event(
                    &other_parent_creator.pub_key_string(),
                    wevent.body.other_parent_index,
                )
                .map_err(|_| {
                    anyhow!(
                        "OtherParent (creator: {}, index: {}) not found",
                        wevent.body.other_parent_creator_id,
                        wevent.body.other_parent_index
                    )
                })?;
        }

        let body = EventBody {
            transactions: wevent.body.transactions.clone(),
            internal_transactions: wevent.body.internal_transactions.clone(),
            block_signatures: wevent.block_signatures(&creator_bytes),
            parents: vec![self_parent, other_parent],
            creator: creator_bytes,
            index: wevent.body.index,
            timestamp: wevent.body.timestamp,
            self_parent_index: wevent.body.self_parent_index,
            other_parent_creator_id: wevent.body.other_parent_creator_id,
            other_parent_index: wevent.body.other_parent_index,
            creator_id: wevent.body.creator_id,
        };

        Ok(Event {
            body,
            signature: wevent.signature.clone(),
            ..Event::default()
        })
    }

    /// Returns an error if the Block does not contain valid signatures from
    /// MORE than 1/3 of participants.
    pub fn check_block(&self, block: &Block, peer_set: &PeerSet) -> Result<()> {
        let psh = peer_set.hash();
        if psh != block.peers_hash() {
            return Err(anyhow!("Wrong PeerSet"));
        }

        let mut valid_signatures = 0i64;
        for s in block.get_signatures() {
            let validator_hex = s.validator_hex();
            if !peer_set.by_pub_key.contains_key(&validator_hex) {
                self.logger
                    .with_field("validator", validator_hex)
                    .warn("Verifying Block signature. Unknown validator");
                continue;
            }
            if block.verify(s).unwrap_or(false) {
                valid_signatures += 1;
            }
        }

        if valid_signatures <= peer_set.trust_count() {
            return Err(anyhow!(
                "Not enough valid signatures: got {}, need {}",
                valid_signatures,
                peer_set.trust_count()
            ));
        }

        self.logger
            .with_field("valid_signatures", valid_signatures)
            .debug("CheckBlock");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Setters
    // -----------------------------------------------------------------------

    fn set_last_consensus_round(&mut self, i: i64) {
        self.last_consensus_round = Some(i);
        if self.first_consensus_round.is_none() {
            self.first_consensus_round = Some(i);
        }
    }

    fn set_round_lower_bound(&mut self, i: i64) {
        self.round_lower_bound = Some(i);
    }

    fn set_anchor_block_index(&mut self, i: i64) {
        self.anchor_block = Some(i);
    }
}

/// The middle bit of an event's hash.
fn middle_bit(ehex: &str) -> bool {
    let hash = match common::decode_from_string(ehex) {
        Ok(h) => h,
        Err(e) => {
            println!("ERROR decoding hex string: {}", e);
            Vec::new()
        }
    };
    !(!hash.is_empty() && hash[hash.len() / 2] == 0)
}

#[cfg(test)]
mod tests {
    //! Translation of `chain/hashgraph/hashgraph_test.go`.
    //!
    //! The test module lives inside `hashgraph.rs` so it can exercise the
    //! private predicate methods (`ancestor`, `see`, `lamport_timestamp`, ...).

    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use k256::ecdsa::SigningKey;

    use super::super::block::BlockSignature;
    use super::super::event::Event;
    use super::super::inmem_store::InmemStore;
    use super::super::rocks_store::RocksDbStore;
    use crate::drivers::network::chain::common;
    use crate::drivers::network::chain::crypto::keys;
    use crate::drivers::network::chain::peers::{Peer, PeerSet};

    const CACHE_SIZE: i64 = 100;
    const N: usize = 3;

    /// Translation of the Go `TestNode` struct.
    struct TestNode {
        pub_bytes: Vec<u8>,
        key: SigningKey,
        #[allow(dead_code)]
        events: Vec<Event>,
    }

    impl TestNode {
        fn new(key: SigningKey) -> TestNode {
            let pub_bytes = keys::from_public_key(key.verifying_key());
            TestNode {
                pub_bytes,
                key,
                events: Vec::new(),
            }
        }

        fn sign_and_add_event(
            &mut self,
            mut event: Event,
            name: &str,
            index: &mut HashMap<String, String>,
            ordered_events: &mut Vec<Event>,
        ) {
            event.sign(&self.key).unwrap();
            index.insert(name.to_string(), event.hex());
            self.events.push(event.clone());
            ordered_events.push(event);
        }
    }

    /// Translation of the Go `play` struct.
    struct Play {
        to: usize,
        index: i64,
        self_parent: String,
        other_parent: String,
        name: String,
        tx_payload: Vec<Vec<u8>>,
        sig_payload: Vec<BlockSignature>,
    }

    /// Convenience constructor for a `Play` with no transaction/signature
    /// payload (the common case in the Go tests).
    fn p(to: usize, index: i64, self_parent: &str, other_parent: &str, name: &str) -> Play {
        Play {
            to,
            index,
            self_parent: self_parent.to_string(),
            other_parent: other_parent.to_string(),
            name: name.to_string(),
            tx_payload: Vec::new(),
            sig_payload: Vec::new(),
        }
    }

    fn idx(index: &HashMap<String, String>, name: &str) -> String {
        index.get(name).cloned().unwrap_or_default()
    }

    fn test_logger() -> Entry {
        common::new_test_entry(common::TEST_LOG_LEVEL)
    }

    fn temp_badger_dir() -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir()
            .join(format!("babble-hg-{}-{}", std::process::id(), nanos))
            .to_string_lossy()
            .into_owned()
    }

    fn init_hashgraph_nodes(
        n: usize,
    ) -> (Vec<TestNode>, HashMap<String, String>, Vec<Event>, PeerSet) {
        let mut nodes = Vec::new();
        let mut pirs = Vec::new();
        for _ in 0..n {
            let key = keys::generate_ecdsa_key().unwrap();
            let pub_hex = keys::public_key_hex(key.verifying_key());
            pirs.push(Peer::new(&pub_hex, "", ""));
            nodes.push(TestNode::new(key));
        }
        let peer_set = PeerSet::new(pirs);
        (nodes, HashMap::new(), Vec::new(), peer_set)
    }

    fn play_events(
        plays: &[Play],
        nodes: &mut [TestNode],
        index: &mut HashMap<String, String>,
        ordered_events: &mut Vec<Event>,
    ) {
        for play in plays {
            let sp = idx(index, &play.self_parent);
            let op = idx(index, &play.other_parent);
            let e = Event::new(
                play.tx_payload.clone(),
                vec![],
                play.sig_payload.clone(),
                vec![sp, op],
                nodes[play.to].pub_bytes.clone(),
                play.index,
            );
            nodes[play.to].sign_and_add_event(e, &play.name, index, ordered_events);
        }
    }

    fn create_hashgraph(
        db: bool,
        ordered_events: &mut [Event],
        peer_set: PeerSet,
    ) -> Hashgraph {
        let store: Box<dyn Store> = if db {
            Box::new(RocksDbStore::new(CACHE_SIZE, &temp_badger_dir(), false).unwrap())
        } else {
            Box::new(InmemStore::new(CACHE_SIZE))
        };

        let mut hashgraph = Hashgraph::new(
            store,
            Box::new(dummy_internal_commit_callback),
            Some(test_logger()),
        );

        hashgraph
            .init(Arc::new(peer_set))
            .expect("ERROR initializing Hashgraph");

        for (i, ev) in ordered_events.iter_mut().enumerate() {
            hashgraph
                .insert_event(ev, true)
                .unwrap_or_else(|e| panic!("ERROR inserting event {}: {}", i, e));
        }

        hashgraph
    }

    fn init_hashgraph_full(
        plays: Vec<Play>,
        db: bool,
        n: usize,
    ) -> (Hashgraph, HashMap<String, String>, Vec<Event>) {
        let (mut nodes, mut index, mut ordered_events, peer_set) = init_hashgraph_nodes(n);
        play_events(&plays, &mut nodes, &mut index, &mut ordered_events);
        let hashgraph = create_hashgraph(db, &mut ordered_events, peer_set);
        (hashgraph, index, ordered_events)
    }

    /*
    |  e12  |
    |   | \ |
    |  s10 e20
    |   | / |
    |   /   |
    | / |   |
    s00 |  s20
    |   |   |
    e01 |   |
    | \ |   |
    e0  e1  e2
    0   1   2
    */
    fn init_hashgraph() -> (Hashgraph, HashMap<String, String>) {
        let plays = vec![
            p(0, 0, "", "", "e0"),
            p(1, 0, "", "", "e1"),
            p(2, 0, "", "", "e2"),
            p(0, 1, "e0", "e1", "e01"),
            p(2, 1, "e2", "", "s20"),
            p(1, 1, "e1", "", "s10"),
            p(0, 2, "e01", "", "s00"),
            p(2, 2, "s20", "s00", "e20"),
            p(1, 2, "s10", "e20", "e12"),
        ];
        let (h, index, _) = init_hashgraph_full(plays, false, N);
        (h, index)
    }

    /// `(descendant, ancestor, expected_value, expected_error)`.
    type AncestryItem = (&'static str, &'static str, bool, bool);

    // Translation of hashgraph_test.go::TestAncestor.
    #[test]
    fn test_ancestor() {
        let (mut h, index) = init_hashgraph();
        let expected: Vec<AncestryItem> = vec![
            // first generation
            ("e01", "e0", true, false),
            ("e01", "e1", true, false),
            ("s00", "e01", true, false),
            ("s20", "e2", true, false),
            ("e20", "s00", true, false),
            ("e20", "s20", true, false),
            ("e12", "e20", true, false),
            ("e12", "s10", true, false),
            // second generation
            ("s00", "e0", true, false),
            ("s00", "e1", true, false),
            ("e20", "e01", true, false),
            ("e20", "e2", true, false),
            ("e12", "e1", true, false),
            ("e12", "s20", true, false),
            // third generation
            ("e20", "e0", true, false),
            ("e20", "e1", true, false),
            ("e20", "e2", true, false),
            ("e12", "e01", true, false),
            ("e12", "e0", true, false),
            ("e12", "e1", true, false),
            ("e12", "e2", true, false),
            // false positive
            ("e01", "e2", false, false),
            ("s00", "e2", false, false),
            ("e0", "", false, true),
            ("s00", "", false, true),
            ("e12", "", false, true),
        ];

        for (descendant, ancestor, val, exp_err) in expected {
            let (a, is_err) = match h.ancestor(&idx(&index, descendant), &idx(&index, ancestor)) {
                Ok(v) => (v, false),
                Err(_) => (false, true),
            };
            assert!(
                !(is_err && !exp_err),
                "Error computing ancestor({}, {})",
                descendant,
                ancestor
            );
            assert_eq!(a, val, "ancestor({}, {}) mismatch", descendant, ancestor);
        }
    }

    // Translation of hashgraph_test.go::TestSelfAncestor.
    #[test]
    fn test_self_ancestor() {
        let (mut h, index) = init_hashgraph();
        let expected: Vec<AncestryItem> = vec![
            ("e01", "e0", true, false),
            ("s00", "e01", true, false),
            ("e01", "e1", false, false),
            ("e12", "e20", false, false),
            ("s20", "e1", false, false),
            ("s20", "", false, true),
            ("e20", "e2", true, false),
            ("e12", "e1", true, false),
            ("e20", "e0", false, false),
            ("e12", "e2", false, false),
            ("e20", "e01", false, false),
        ];

        for (descendant, ancestor, val, exp_err) in expected {
            let (a, is_err) =
                match h.self_ancestor(&idx(&index, descendant), &idx(&index, ancestor)) {
                    Ok(v) => (v, false),
                    Err(_) => (false, true),
                };
            assert!(
                !(is_err && !exp_err),
                "Error computing self_ancestor({}, {})",
                descendant,
                ancestor
            );
            assert_eq!(a, val, "self_ancestor({}, {}) mismatch", descendant, ancestor);
        }
    }

    // Translation of hashgraph_test.go::TestSee.
    #[test]
    fn test_see() {
        let (mut h, index) = init_hashgraph();
        let expected: Vec<AncestryItem> = vec![
            ("e01", "e0", true, false),
            ("e01", "e1", true, false),
            ("e20", "e0", true, false),
            ("e20", "e01", true, false),
            ("e12", "e01", true, false),
            ("e12", "e0", true, false),
            ("e12", "e1", true, false),
            ("e12", "s20", true, false),
        ];

        for (descendant, ancestor, val, _exp_err) in expected {
            let a = h
                .see(&idx(&index, descendant), &idx(&index, ancestor))
                .unwrap_or(false);
            assert_eq!(a, val, "see({}, {}) mismatch", descendant, ancestor);
        }
    }

    // Translation of hashgraph_test.go::TestLamportTimestamp.
    #[test]
    fn test_lamport_timestamp() {
        let (mut h, index) = init_hashgraph();
        let expected: Vec<(&str, i64)> = vec![
            ("e0", 0),
            ("e1", 0),
            ("e2", 0),
            ("e01", 1),
            ("s10", 1),
            ("s20", 1),
            ("s00", 2),
            ("e20", 3),
            ("e12", 4),
        ];

        for (e, ets) in expected {
            let ts = h
                .lamport_timestamp(&idx(&index, e))
                .unwrap_or_else(|err| panic!("Error computing lamport_timestamp({}): {}", e, err));
            assert_eq!(ts, ets, "{} LamportTimestamp mismatch", e);
        }
    }
}
