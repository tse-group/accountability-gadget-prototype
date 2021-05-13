use crate::error::{CheckpointResult};
use crate::cpactions::{CheckpointTransaction, CheckpointAction};
use crate::cpevents::{CheckpointEvent};
// use crate::cpleader::{LeaderElector};
use crate::config::{CheckpointCommittee, CheckpointParameters};

use crypto::{PublicKey, Digest};
use log::{info, warn};
use tokio::sync::{mpsc::Receiver as MpscReceiver};
use tokio::sync::{broadcast::Sender as BroadcastSender};
use std::convert::{TryInto};
// use std::collections::HashSet;
use std::collections::{HashMap};
use itertools::Itertools;


#[derive(Eq, PartialEq, Clone, Debug)]
enum Vote {
    Reject,
    Accept(Digest),
}


pub struct MyCheckpointExtractor {
    // ...
}

impl MyCheckpointExtractor {
    pub fn run(
        _name: PublicKey,
        committee: CheckpointCommittee,
        _parameters: CheckpointParameters,
        mut rx_ledger: MpscReceiver<Vec<u8>>,
        tx_checkpoint_events: BroadcastSender<CheckpointEvent>,
    ) -> CheckpointResult<()> {
        tokio::spawn(async move {
            info!("Checkpoint extractor started!");

            '_iteration_loop: for current_iteration in 0.. {
                info!("Iteration: {}", current_iteration);
                let mut current_votes = HashMap::<PublicKey, Vote>::new();

                'vote_loop: while let Some(tx) = rx_ledger.recv().await {
                    let vote: Result<CheckpointTransaction, _> = tx.try_into();
                    if vote.is_err() {
                        warn!("Deserialization of vote failed, this should not happen among honest nodes!");
                        continue 'vote_loop;
                    }
                    let vote = vote.unwrap();
                    if vote.verify_signature(&committee).is_err() {
                        warn!("Invalid signature on vote: {:?} / Valid? {:?}", vote, vote.verify_signature(&committee));
                        continue 'vote_loop;
                    }

                    info!("Well-formed vote with valid signature: {:?} by {:?}", vote.action, vote.author);
                    match vote.action {
                        CheckpointAction::Accept(i, b)
                            if i == current_iteration
                        => {
                            current_votes.insert(vote.author, Vote::Accept(b));
                        },
                        CheckpointAction::Reject(i)
                            if i == current_iteration
                        => {
                            current_votes.insert(vote.author, Vote::Reject);
                        },
                        _ => { /* info!("Invalid vote: {:?}", vote); */ /* note: most of "invalid" votes are actually repeated votes, which are being repeated because the HotStuff implementation currently does not manage mempool 100% correct in presence of adversaries */ },
                    }

                    // is this checkpointing iteration over?
                    // TODO: this does not take into account stake yet!
                    let bs: Vec<Digest> = current_votes.values().filter_map(|v| {
                        match v {
                            Vote::Reject => None,
                            Vote::Accept(b) => Some(b),
                        }
                    }).unique().cloned().collect();

                    for b in bs {
                        if current_votes.values().filter(|v| { (**v) == Vote::Accept(b.clone()) }).count() as f32 > 2.0/3.0 * (committee.size() as f32) {
                            info!("2n/3 votes for: {:?}", b);
                            tx_checkpoint_events.send(CheckpointEvent::NewCheckpoint(current_iteration, Some(b.clone()))).expect("failed to send checkpoint event!");
                            break 'vote_loop
                        }
                    }
                    if current_votes.values().filter(|v| { (**v) == Vote::Reject }).count() as f32 >= 1.0/3.0 * (committee.size() as f32) {
                        info!("n/3 votes for: Reject");
                        tx_checkpoint_events.send(CheckpointEvent::NewCheckpoint(current_iteration, None)).expect("failed to send checkpoint event!");
                        break 'vote_loop
                    }
                }
            }
        });

        Ok(())
    }
}
