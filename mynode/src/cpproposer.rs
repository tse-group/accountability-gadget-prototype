use crate::error::{CheckpointResult};
use crate::cpactions::{CheckpointTransaction, CheckpointAction};
use crate::cpevents::{CheckpointEvent};
use crate::cpleader::LeaderElector;
use crate::config::{CheckpointCommittee, CheckpointParameters};
use crate::lcblocktreemanager::{MyLcBlockTreeManager};

use consensus::{ConsensusCommand};
use gossip::{Gossip};

use log::{info, warn};
use tokio::sync::{mpsc::channel as mpsc_channel, mpsc::Sender as MpscSender}; //, mpsc::Receiver as MpscReceiver};
use tokio::sync::{broadcast::Receiver as BroadcastReceiver};
use tokio::time::{self, Duration};
use std::convert::{TryInto};
use crypto::{Digest, PublicKey, SignatureService, Hash};
use std::assert;
use std::collections::{HashMap};


pub struct MyCheckpointProposer {
    // ...
}

impl MyCheckpointProposer {
    pub async fn run(
        name: PublicKey,
        committee: CheckpointCommittee,
        parameters: CheckpointParameters,
        signature_service: SignatureService,
        tx_transactions: MpscSender<Vec<u8>>,
        mut rx_checkpoint_events: BroadcastReceiver<CheckpointEvent>,
        lcblocktreemgr: MyLcBlockTreeManager,
        tx_consensus_cmds: MpscSender<ConsensusCommand>,
        boycott_proposals: bool,
        gossip: Gossip,
    ) -> CheckpointResult<()> {
        // Hook up to the Gossip!
        let (mut rx_inbound_proposals_raw, tx_outbound_proposals_raw, _) = gossip.subscribe("proposals".to_string(), false).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;


        // channel where proposals (from broadcast) and events (from extractor) come together
        let (tx_checkpoint_events_fusioned, rx_checkpoint_events_fusioned) = mpsc_channel(1000);

        {
            let committee = committee.clone();
            tokio::spawn(async move {
                let leader_election = LeaderElector::new(committee.clone());

                loop {
                    tokio::select! {
                        ev = rx_checkpoint_events.recv() => {
                            if let Ok(ev) = ev {
                                tx_checkpoint_events_fusioned.send(ev).await.expect("failed to forward checkpoint event");
                            } else { panic!("Stream of incoming checkpoint events closed unexpectedly!"); }
                        },
                        val = rx_inbound_proposals_raw.recv() => {
                            if let Some((_k, v)) = val {
                                let p: CheckpointTransaction = v.try_into().expect("failed to deserialize CheckpointTransaction!");

                                if p.verify_signature(&committee).is_err() {
                                    continue;
                                }

                                if let CheckpointAction::Propose(i, b) = p.action {
                                    if !(p.author == leader_election.get_leader(i)) {
                                        continue;
                                    }

                                    // proposal has valid signature from authorized leader, so it is a verified proposal
                                    tx_checkpoint_events_fusioned.send(CheckpointEvent::NewProposal(i, b)).await.expect("failed to forward proposal");
                                }
                            } else { panic!("Stream of incoming checkpoint proposals closed unexpectedly!");}
                        }
                    }
                }
            });
        }

        let mut rx_checkpoint_events = rx_checkpoint_events_fusioned;


        tokio::spawn(async move {
            info!("Checkpoint proposer started!");


            let leader_election = LeaderElector::new(committee.clone());

            let mut last_checkpoint = None;
            let mut proposals = HashMap::<usize, Digest>::new();

            'iteration_loop: for current_iteration in 0.. {
                info!("Iteration: {}", current_iteration);


                // cleanup old proposals
                if current_iteration > 0 {
                    proposals.remove(&(current_iteration-1));
                }


                // pause consensus temporarily
                // will be resumed on the first event of:
                // - iteration is left with a decision (because then all copies of tx_trigger_resume will go out of scope)
                // - a proposal is received
                // - iteration is being abandoned
                let (tx_trigger_resume_single, rx_trigger_resume_single) = tokio::sync::oneshot::channel();
                tx_consensus_cmds.send(ConsensusCommand::Pause(rx_trigger_resume_single)).await.expect("Failed to ask consensus to pause!");
                let (tx_trigger_resume, mut rx_trigger_resume) = mpsc_channel::<()>(1);
                tokio::spawn(async move {
                    match rx_trigger_resume.recv().await {
                        Some(()) => { info!("Explicitely asked to resume consensus!"); },
                        None => { info!("Resuming consensus because of end of iteration!"); },
                    }
                    tx_trigger_resume_single.send(()).expect("Failed to ask consensus to resume!");
                });


                if last_checkpoint != None {
                    let sleep_inter_checkpoint_delay = time::sleep(Duration::from_millis(parameters.propose_inter_checkpoint_delay));
                    tokio::pin!(sleep_inter_checkpoint_delay);

                    while !sleep_inter_checkpoint_delay.is_elapsed() {
                        tokio::select! {
                            _ = &mut sleep_inter_checkpoint_delay => {
                                info!("Inter-checkpoint delay up of iteration {}!", current_iteration);
                            },
                            ev = rx_checkpoint_events.recv() => {
                                info!("CheckpointEvent received during inter-checkpoint delay of iteration {}: {:?}", current_iteration, ev);
                                if let Some(val) = ev {
                                    match val {
                                        CheckpointEvent::NewCheckpoint(i, b) => {
                                            assert!(i == current_iteration);

                                            // tx_trigger_resume.send(()).await.ok(); //.expect("Failed to ask consensus to resume!");   // resume consensus
                                            last_checkpoint = b;
                                            continue 'iteration_loop;
                                        },
                                        CheckpointEvent::NewProposal(i, b) => {
                                            // TODO: probably want to reject proposals too far in the future
                                            if i >= current_iteration && proposals.get(&i).is_none() {
                                                proposals.insert(i, b);
                                            }
                                        },
                                    }
                                } else { panic!("Error receiving checkpoint event!"); }
                            },
                        }
                    }
                }


                info!("Checkpoint leader: {:?} Myself?: {:?} Iteration: {}", leader_election.get_leader(current_iteration), leader_election.get_leader(current_iteration) == name, current_iteration);
                if leader_election.get_leader(current_iteration) == name && !boycott_proposals {
                    Self::broadcast_proposal(&tx_outbound_proposals_raw, CheckpointTransaction::new(
                        name.clone(),
                        CheckpointAction::Propose(current_iteration, lcblocktreemgr.get_current_proposal().await),
                        signature_service.clone()
                    ).await).await;
                }


                let (tx_proposal_to_act_on, mut rx_proposal_to_act_on) = mpsc_channel::<Digest>(1);
                if let Some(p) = proposals.get(&current_iteration) {
                    tx_proposal_to_act_on.send(p.clone()).await.expect("failed to inject proposal to act on!");
                }

                let sleep_timeout_delay = time::sleep(Duration::from_millis(parameters.propose_timeout));
                tokio::pin!(sleep_timeout_delay);

                while !sleep_timeout_delay.is_elapsed() {
                    tokio::select! {
                        _ = &mut sleep_timeout_delay => {
                            info!("Checkpoint timeout up of iteration {}!", current_iteration);
                        },
                        proposal_to_act_on = rx_proposal_to_act_on.recv() => {
                            if proposal_to_act_on.is_none() {
                                panic!("Error while waiting for proposal to act on: {:?}", proposal_to_act_on);
                            }
                            let proposal_to_act_on = proposal_to_act_on.unwrap();

                            tx_trigger_resume.send(()).await.ok(); //.expect("Failed to ask consensus to resume!");   // resume consensus
                            
                            if lcblocktreemgr.validate_proposal(proposal_to_act_on.clone()).await {
                                info!("Checkpoint proposal: {:?} Valid?: {:?} Iteration: {}", proposal_to_act_on, true, current_iteration);
                                Self::submit_vote(&tx_transactions, CheckpointTransaction::new(
                                    name.clone(),
                                    CheckpointAction::Accept(current_iteration, proposal_to_act_on.clone()),
                                    signature_service.clone()
                                ).await).await;
                            } else {
                                info!("Checkpoint proposal: {:?} Valid?: {:?} Iteration: {}", proposal_to_act_on, false, current_iteration);
                                Self::submit_vote(&tx_transactions, CheckpointTransaction::new(
                                    name.clone(),
                                    CheckpointAction::Reject(current_iteration),
                                    signature_service.clone()
                                ).await).await;
                            }
                        },
                        ev = rx_checkpoint_events.recv() => {
                            info!("CheckpointEvent received while waiting for checkpoint timeout of iteration {}: {:?}", current_iteration, ev);
                            if let Some(val) = ev {
                                match val {
                                    CheckpointEvent::NewCheckpoint(i, b) => {
                                        assert!(i == current_iteration);

                                        // tx_trigger_resume.send(()).await.ok(); //.expect("Failed to ask consensus to resume!");   // resume consensus
                                        last_checkpoint = b;
                                        continue 'iteration_loop;
                                    },
                                    CheckpointEvent::NewProposal(i, b) => {
                                        // TODO: probably want to reject proposals too far in the future
                                        if i >= current_iteration && proposals.get(&i).is_none() {
                                            proposals.insert(i, b.clone());

                                            if i == current_iteration {
                                                tx_proposal_to_act_on.send(b).await.expect("failed to inject proposal to act on!");
                                            }
                                        }
                                    },
                                }
                            } else { panic!("Error receiving checkpoint event!"); }
                        },
                    }
                }

                tx_trigger_resume.send(()).await.ok(); //.expect("Failed to ask consensus to resume!");   // resume consensus

                // it seems the HotStuff implementation might sometimes drop an
                // unconfirmed transaction, in particular linked to faulty leaders;
                // thus, re-send Reject repeatedly until checkpoint iteration ends
                loop {
                    warn!("Checkpoint timeout, rejecting current iteration {}!", current_iteration);
                    Self::submit_vote(&tx_transactions, CheckpointTransaction::new(
                        name.clone(),
                        CheckpointAction::Reject(current_iteration),
                        signature_service.clone()
                    ).await).await;

                    let sleep_reject_repeat_interval = time::sleep(Duration::from_millis(30_000));
                    tokio::pin!(sleep_reject_repeat_interval);

                    while !sleep_reject_repeat_interval.is_elapsed() {
                        tokio::select! {
                            _ = &mut sleep_reject_repeat_interval => {},
                            ev = rx_checkpoint_events.recv() => {
                                info!("CheckpointEvent received while waiting for end of checkpoint iteration {}: {:?}", current_iteration, ev);
                                if let Some(val) = ev {
                                    match val {
                                        CheckpointEvent::NewCheckpoint(i, b) => {
                                            assert!(i == current_iteration);

                                            // tx_trigger_resume.send(()).await.ok(); //.expect("Failed to ask consensus to resume!");   // resume consensus
                                            last_checkpoint = b;
                                            continue 'iteration_loop;
                                        },
                                        CheckpointEvent::NewProposal(i, b) => {
                                            // TODO: probably want to reject proposals too far in the future
                                            if i >= current_iteration && proposals.get(&i).is_none() {
                                                proposals.insert(i, b);
                                            }
                                        },
                                    }
                                } else { panic!("Error receiving checkpoint event!"); }
                            },
                        }
                    }
                }
            }
        });

        Ok(())
    }

    async fn submit_vote(tx_transactions: &MpscSender<Vec<u8>>, checkpoint_tx: CheckpointTransaction) {
        tx_transactions.send(checkpoint_tx.try_into().unwrap()).await.expect("Error sending checkpointing transaction!")
    }

    async fn broadcast_proposal(tx_transactions: &MpscSender<(Vec<u8>, Vec<u8>)>, checkpoint_tx: CheckpointTransaction) {
        let checkpoint_tx_binary: Vec<u8> = checkpoint_tx.clone().try_into().unwrap();
        tx_transactions.send((checkpoint_tx.digest().to_vec(), checkpoint_tx_binary)).await.expect("Error sending checkpointing transaction!")
    }
}