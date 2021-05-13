use crate::config::{CheckpointCommittee, CheckpointParameters};
use crate::lcblocks::{LcBlock, LcBlockPayload};

use crypto::{Hash, Digest, PublicKey};
use log::{info};
use tokio::time::{Duration, Instant, interval_at};
use tokio::sync::{mpsc::Receiver as MpscReceiver, mpsc::Sender as MpscSender}; // mpsc::channel as mpsc_channel
use std::convert::{TryInto};
use std::time::SystemTime;
use rand::prelude::*;
use rand::rngs::StdRng;

pub struct MyMiner {
    // ...
}

fn get_random_seed(p: PublicKey) -> u64 {
    let mut v: u64 = 0;
    for i in 0..8 {
        v = (v<<8) + (p.0[i] as u64);
    }
    v
}

impl MyMiner {
    pub fn run(
        name: PublicKey,
        committee: CheckpointCommittee,
        parameters: CheckpointParameters,
        mut rx_tip_mining: MpscReceiver<(Digest, u64)>,
        tx_outbound_msgs: MpscSender<(Vec<u8>, Vec<u8>)>,
        produce_adversarial_blocks: bool,
    ) {
        // lay out time schedule for mining
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).expect("failed to retrieve SystemTime!").as_millis() as u64;
        let t_genesis_in = parameters.lcmine_slot - now % parameters.lcmine_slot + parameters.lcmine_slot_offset;
        let mut interval_lc = interval_at(Instant::now() + Duration::from_millis(t_genesis_in), Duration::from_millis(parameters.lcmine_slot));
        let n_machines = committee.authorities.len();

        tokio::spawn(async move {
            info!("Miner started!");

            let mut current_parent: Digest = LcBlock::genesis().digest();
            let mut rnd = StdRng::seed_from_u64(get_random_seed(name));
            let mut accept_blocks_for_slot = 0;
            let mut blocks_from_future_slots = Vec::new();

            loop {
                loop {
                    tokio::select! {
                        _ = interval_lc.tick() => {
                            break;
                        },
                        target_parent = rx_tip_mining.recv() => {
                            match target_parent {
                                Some((target_parent, target_slot)) => {
                                    if target_slot <= accept_blocks_for_slot {
                                        info!("New parent to mine on: {:?} from {:?}", (&target_parent, target_slot), current_parent);
                                        current_parent = target_parent;
                                    } else {
                                        info!("Future parent to mine on, deferring: {:?} from {:?}", (&target_parent, target_slot), current_parent);
                                        blocks_from_future_slots.push((target_parent, target_slot));
                                    }
                                },
                                None => {
                                    panic!("Stream of parents to mine on was closed, terminating ...");
                                },
                            }
                        },
                    }
                }


                let t_slot = (SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).expect("failed to retrieve SystemTime!").as_millis() as u64) / parameters.lcmine_slot;

                if rnd.gen::<f32>() < 1./ (n_machines as f32) {
                    info!("Now is LC slot: {} -- I WON THE LOTTERY!", t_slot);
                    let lcblk_payload_string = if !produce_adversarial_blocks {
                        format!("By {} at {}", name, t_slot)
                    } else {
                        format!("By {} at {} [adv]", name, t_slot)
                    };
                    let lcblk = LcBlock::new(Some(current_parent.clone()), LcBlockPayload::new(lcblk_payload_string), t_slot);
                    let lcblk_binary: Vec<u8> = (&lcblk).try_into().expect("unable to serialize LC block!");
                    tx_outbound_msgs.send((lcblk.digest().to_vec(), lcblk_binary)).await.expect("failed to deliver newly mined block!");
                } else {
                    info!("Now is LC slot: {}", t_slot);
                }

                accept_blocks_for_slot = t_slot;


                let mut blocks_from_future_slots_new = Vec::new();

                for (target_parent, target_slot) in blocks_from_future_slots {
                    if target_slot <= accept_blocks_for_slot {
                        info!("New parent to mine on: {:?} from {:?}", (&target_parent, target_slot), current_parent);
                        current_parent = target_parent;
                    } else {
                        info!("Future parent to mine on, deferring: {:?} from {:?}", (&target_parent, target_slot), current_parent);
                        blocks_from_future_slots_new.push((target_parent, target_slot));
                    }
                }

                blocks_from_future_slots = blocks_from_future_slots_new;
            }
        });
    }
}
