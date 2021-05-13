use crate::cpevents::{CheckpointEvent};
use crate::config::{CheckpointCommittee, CheckpointParameters};
use crate::lcblocks::{LcBlock, LcBlockTree};

use crypto::{Digest, Hash, PublicKey};
use log::{info, warn};
use tokio::sync::{mpsc::channel as mpsc_channel, mpsc::Receiver as MpscReceiver, mpsc::Sender as MpscSender};
use tokio::sync::{broadcast::Receiver as BroadcastReceiver};
use tokio::sync::{oneshot::channel as oneshot_channel, oneshot::Sender as OneshotSender};
use std::convert::{TryInto};

#[derive(Debug)]
pub enum MyLcBlockTreeManagerCmd {
    GetCurrentProposal(OneshotSender<Digest>),
    ValidateProposal(OneshotSender<bool>, Digest),
    GetHeightOfBlock(OneshotSender<Option<usize>>, Digest),
    GetCurrentTip(OneshotSender<Digest>),
    GetParentOfBlock(OneshotSender<Option<Option<Digest>>>, Digest),
    GetBlock(OneshotSender<Option<LcBlock>>, Digest),
    // GetCurrentLogAva(OneshotSender<Digest>),
}

#[derive(Clone)]
pub struct MyLcBlockTreeManager {
    cmd_queue: MpscSender<MyLcBlockTreeManagerCmd>
}

impl MyLcBlockTreeManager {
    pub fn run(
        _name: PublicKey,
        _committee: CheckpointCommittee,
        parameters: CheckpointParameters,
        mut rx_checkpoint_events_lcblocktree_endpoint: BroadcastReceiver<CheckpointEvent>,
        mut rx_inbound_msgs: MpscReceiver<(Vec<u8>, Vec<u8>)>,
        tx_tip_mining: MpscSender<(Digest, u64)>,
    ) -> MyLcBlockTreeManager {
        let (tx_blocktreemgr_cmds, mut rx_blocktreemgr_cmds) = mpsc_channel(1000);

        tokio::spawn(async move {
            info!("LcBlockTreeManager spawned!");

            let mut lcblocktree = LcBlockTree::new();
            let mut checkpoints = Vec::new();
            let mut checkpoints_iterations = Vec::new();

            loop {
                tokio::select! {
                    new_msg = rx_inbound_msgs.recv() => match new_msg {
                        Some((_k, v)) => {
                            let blk: LcBlock = v.try_into().expect("failed to deserialize what is expected to be LcBlock!");
                            info!("Adding new LcBlock to LcBlockTree: {:?}", &blk);
                            match lcblocktree.add(blk) {
                                Ok(()) => {
                                },
                                Err(e) => {
                                    panic!("Error when adding block to tree: {:?}", e);
                                },
                            };

                            // tx_tip_mining.send(
                            //     lcblocktree.leading_tip_extending(
                            //         checkpoints.last().unwrap_or(&LcBlock::genesis().digest())
                            //     ).clone()
                            // ).await.expect("failed to update tip to mine on!");
                            let tip_blk_digest = lcblocktree.leading_tip_extending(
                                    checkpoints.last().unwrap_or(&LcBlock::genesis().digest())
                                ).clone();
                            let tip_blk_slot = lcblocktree.block(&tip_blk_digest).unwrap().slot;
                            tx_tip_mining.send((tip_blk_digest, tip_blk_slot))
                                .await.expect("failed to update tip to mine on!");


                            let genesis_digest = LcBlock::genesis().digest();
                            let log_acc = checkpoints.last().unwrap_or(&genesis_digest);
                            let log_ava = lcblocktree.higher_of(
                                &log_acc,
                                lcblocktree.k_deep(
                                    parameters.lctree_logava_k_deep,
                                    lcblocktree.leading_tip_extending(&log_acc)
                                ),
                            ).expect("failed to compare blocks for unexpected reason!");
                            info!("{}", lcblocktree.dump_dot(&checkpoints, &checkpoints_iterations));
                            info!("Current LOGacc: {} {} {:?}",
                                lcblocktree.height_honest(log_acc).expect("failed to obtain height_honest of LOGacc"),
                                lcblocktree.height(log_acc).expect("failed to obtain height of LOGacc"),
                                log_acc);
                            info!("Current LOGava: {} {} {:?}",
                                lcblocktree.height_honest(log_ava).expect("failed to obtain height_honest of LOGava"),
                                lcblocktree.height(log_ava).expect("failed to obtain height of LOGava"),
                                log_ava);
                        },
                        None => {
                            panic!("Stream of incoming network messages closed, terminating!");
                        }
                    },
                    new_cp = rx_checkpoint_events_lcblocktree_endpoint.recv() => match new_cp {
                        Ok(CheckpointEvent::NewCheckpoint(i, cp)) => {
                            info!("New checkpoint decision: {} {:?}", i, cp);
                            match cp {
                                Some(cp) => {
                                    checkpoints.push(cp);
                                    checkpoints_iterations.push(i);

                                    // tx_tip_mining.send(
                                    //     lcblocktree.leading_tip_extending(
                                    //         checkpoints.last().unwrap_or(&LcBlock::genesis().digest())
                                    //     ).clone()
                                    // ).await.expect("failed to update tip to mine on!");
                                    let tip_blk_digest = lcblocktree.leading_tip_extending(
                                            checkpoints.last().unwrap_or(&LcBlock::genesis().digest())
                                        ).clone();
                                    let tip_blk_slot = lcblocktree.block(&tip_blk_digest).unwrap().slot;
                                    tx_tip_mining.send((tip_blk_digest, tip_blk_slot))
                                        .await.expect("failed to update tip to mine on!");


                                    let genesis_digest = LcBlock::genesis().digest();
                                    let log_acc = checkpoints.last().unwrap_or(&genesis_digest);
                                    let log_ava = lcblocktree.higher_of(
                                        &log_acc,
                                        lcblocktree.k_deep(
                                            parameters.lctree_logava_k_deep,
                                            lcblocktree.leading_tip_extending(&log_acc)
                                        ),
                                    ).expect("failed to compare blocks for unexpected reason!");
                                    info!("{}", lcblocktree.dump_dot(&checkpoints, &checkpoints_iterations));
                                    info!("Current LOGacc: {} {} {:?}",
                                        lcblocktree.height_honest(log_acc).expect("failed to obtain height_honest of LOGacc"),
                                        lcblocktree.height(log_acc).expect("failed to obtain height of LOGacc"),
                                        log_acc);
                                    info!("Current LOGava: {} {} {:?}",
                                        lcblocktree.height_honest(log_ava).expect("failed to obtain height_honest of LOGava"),
                                        lcblocktree.height(log_ava).expect("failed to obtain height of LOGava"),
                                        log_ava);
                                },
                                None => {

                                }
                            }
                        },
                        Ok(CheckpointEvent::NewProposal(i, cp)) => {
                            info!("New checkpoint proposal: {} {:?}", i, cp);
                        },
                        Err(e) => {
                            panic!("Terminating due to failing to recv checkpoint event: {:?}", e);
                        },
                    },
                    cmd = rx_blocktreemgr_cmds.recv() => {
                        info!("Received MyLcBlockTreeManagerCmd: {:?}", cmd);
                        match cmd {
                            Some(MyLcBlockTreeManagerCmd::GetCurrentProposal(ch)) => {
                                let genesis_digest = LcBlock::genesis().digest();
                                let latest_checkpoint = checkpoints.last().unwrap_or(&genesis_digest);
                                ch.send(
                                    lcblocktree.higher_of(
                                        &latest_checkpoint,
                                        lcblocktree.k_deep(
                                            parameters.propose_k_deep,
                                            lcblocktree.leading_tip_extending(&latest_checkpoint)
                                        ),
                                    ).expect("failed to compare blocks for unexpected reason!").clone()
                                ).expect("failed to send result of GetCurrentProposal");
                            },
                            Some(MyLcBlockTreeManagerCmd::ValidateProposal(ch, digest)) => {
                                let genesis_digest = LcBlock::genesis().digest();
                                let latest_checkpoint = checkpoints.last().unwrap_or(&genesis_digest);
                                let condition_1 = lcblocktree.is_prefix_of(&latest_checkpoint, &digest);
                                let condition_2 = lcblocktree.is_prefix_of(&digest, &lcblocktree.leading_tip_extending(&latest_checkpoint));
                                if condition_1.is_err() || condition_2.is_err() {
                                    warn!("Validating proposal failed, this should not happen among honest nodes! {:?} {:?}", condition_1, condition_2);
                                    ch.send(false).expect("failed to send result of ValidateProposal");
                                } else {
                                    ch.send(condition_1.unwrap() && condition_2.unwrap()).expect("failed to send result of ValidateProposal");
                                }
                            },
                            Some(MyLcBlockTreeManagerCmd::GetHeightOfBlock(ch, digest)) => {
                                ch.send(lcblocktree.height(&digest).ok()).expect("failed to send result of GetHeightOfBlock");
                            },
                            Some(MyLcBlockTreeManagerCmd::GetCurrentTip(ch)) => {
                                ch.send(lcblocktree.leading_tip_extending(
                                    checkpoints.last().unwrap_or(&LcBlock::genesis().digest())
                                ).clone()).expect("failed to send result of GetCurrentTip");
                            },
                            Some(MyLcBlockTreeManagerCmd::GetParentOfBlock(ch, digest)) => {
                                ch.send(lcblocktree.parent(&digest).ok()).expect("failed to send result of GetParentOfBlock");
                            },
                            Some(MyLcBlockTreeManagerCmd::GetBlock(ch, digest)) => {
                                ch.send(lcblocktree.block(&digest).ok()).expect("failed to send result of GetBlock");
                            },
                            // Some(MyLcBlockTreeManagerCmd::GetCurrentLogAva(ch)) => {
                            //     let genesis_digest = LcBlock::genesis().digest();
                            //     let latest_checkpoint = checkpoints.last().unwrap_or(&genesis_digest);
                            //     ch.send(
                            //         lcblocktree.higher_of(
                            //             &latest_checkpoint,
                            //             lcblocktree.k_deep(
                            //                 parameters.lctree_logava_k_deep,
                            //                 lcblocktree.leading_tip_extending(&latest_checkpoint)
                            //             ),
                            //         ).expect("failed to compare blocks for unexpected reason!").clone()
                            //     ).expect("failed to send result of GetCurrentLogAva");
                            // },
                            None => {
                                panic!("Stream of incoming MyLcBlockTreeManagerCmd closed, terminating!");
                            }
                        }
                    },
                }
            }
        });

        Self { cmd_queue: tx_blocktreemgr_cmds }
    }

    pub async fn get_current_proposal(&self) -> Digest {
        let (tx, rx) = oneshot_channel();
        self.cmd_queue.send(MyLcBlockTreeManagerCmd::GetCurrentProposal(tx)).await.expect("failed to send command!");
        rx.await.expect("failed to recv command response!")
    }

    pub async fn validate_proposal(&self, prop: Digest) -> bool {
        let (tx, rx) = oneshot_channel();
        self.cmd_queue.send(MyLcBlockTreeManagerCmd::ValidateProposal(tx, prop)).await.expect("failed to send command!");
        rx.await.expect("failed to recv command response!")
    }

    pub async fn get_height_of_block(&self, d: Digest) -> Option<usize> {
        let (tx, rx) = oneshot_channel();
        self.cmd_queue.send(MyLcBlockTreeManagerCmd::GetHeightOfBlock(tx, d)).await.expect("failed to send command!");
        rx.await.expect("failed to recv command response!")
    }

    pub async fn get_current_tip(&self) -> Digest {
        let (tx, rx) = oneshot_channel();
        self.cmd_queue.send(MyLcBlockTreeManagerCmd::GetCurrentTip(tx)).await.expect("failed to send command!");
        rx.await.expect("failed to recv command response!")
    }

    pub async fn get_parent_of_block(&self, d: Digest) -> Option<Option<Digest>> {
        let (tx, rx) = oneshot_channel();
        self.cmd_queue.send(MyLcBlockTreeManagerCmd::GetParentOfBlock(tx, d)).await.expect("failed to send command!");
        rx.await.expect("failed to recv command response!")
    }

    pub async fn get_block(&self, d: Digest) -> Option<LcBlock> {
        let (tx, rx) = oneshot_channel();
        self.cmd_queue.send(MyLcBlockTreeManagerCmd::GetBlock(tx, d)).await.expect("failed to send command!");
        rx.await.expect("failed to recv command response!")
    }

    // pub async fn get_current_log_ava(&self) -> Digest {
    //     let (tx, rx) = oneshot_channel();
    //     self.cmd_queue.send(MyLcBlockTreeManagerCmd::GetCurrentLogAva(tx)).await.expect("failed to send command!");
    //     rx.await.expect("failed to recv command response!")
    // }
}