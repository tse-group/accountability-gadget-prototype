use crate::config::Export as _;
use crate::config::{Committee, Parameters, Secret};

// use crate::lcblocks::{LcBlock};
use crate::cpextractor::{MyCheckpointExtractor};
use crate::cpproposer::{MyCheckpointProposer};
use crate::miner::{MyMiner};
use crate::lcblocktreemanager::{MyLcBlockTreeManager};
use crate::error::{CheckpointError};
use crate::mynode::{NodeError};

use gossip::{Gossip, GossipError};

use consensus::{Block, Consensus, ConsensusError, ConsensusCommand, QC};
use crypto::SignatureService;
use log::{info, error};
use mempool::{Mempool, MempoolError, Payload};
use store::{Store, StoreError};
use thiserror::Error;
use tokio::sync::{mpsc::channel as mpsc_channel, mpsc::Receiver as MpscReceiver, mpsc::Sender as MpscSender};
use tokio::sync::{broadcast::channel as broadcast_channel}; //, broadcast::Receiver as BroadcastReceiver, broadcast::Sender as BroadcastSender};
// use tokio::sync::{watch::channel as watch_channel, watch::Receiver as WatchReceiver, watch::Sender as WatchSender};
use tokio::time::{self, Duration};


// define adversarial communication messages

use serde::{Deserialize, Serialize};
use crypto::{Digest, Hash, PublicKey};
use ed25519_dalek::{Sha512, Digest as _};
use std::convert::{TryFrom, TryInto};
// use std::collections::{HashMap, HashSet};
// use std::fmt;
use crate::lcblocks::LcBlock;

#[derive(Debug, Serialize, Deserialize)]
pub enum AdversarialGossipMsg {
    Hello(PublicKey),
    HiddenLcBlock(LcBlock),
}

impl TryFrom<Vec<u8>> for AdversarialGossipMsg {
    type Error = Box<bincode::ErrorKind>;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        bincode::deserialize::<AdversarialGossipMsg>(&value)
    }
}

impl TryFrom<&AdversarialGossipMsg> for Vec<u8> {
    type Error = Box<bincode::ErrorKind>;

    fn try_from(value: &AdversarialGossipMsg) -> Result<Self, Self::Error> {
        bincode::serialize(&value)
    }
}

impl Hash for AdversarialGossipMsg {
    fn digest(&self) -> Digest {
        let self_binary: Vec<u8> = self.try_into().unwrap();
        let mut hasher = Sha512::new();
        hasher.update(self_binary);
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}


// adversarial node error

#[derive(Error, Debug)]
pub enum AdversarialNodeError {
    #[error("Store error: {0}")]
    StoreError(#[from] StoreError),

    #[error(transparent)]
    ConsensusError(#[from] ConsensusError),

    #[error(transparent)]
    MempoolError(#[from] MempoolError),

    #[error(transparent)]
    CheckpointError(#[from] CheckpointError),

    #[error(transparent)]
    GossipError(#[from] GossipError),

    #[error(transparent)]
    NodeError(#[from] NodeError),
}

pub struct AdversarialNode {
    pub commit: MpscReceiver<Block>,
    pub store: Store,
    pub ledger: MpscSender<Vec<u8>>,
}

impl AdversarialNode {
    pub async fn new(
        committee_file: &str,
        key_file: &str,
        store_path: &str,
        parameters: Option<&str>,
    ) -> Result<Self, AdversarialNodeError> {

        // prep for usual Node setup
        let (tx_commit, rx_commit) = mpsc_channel(1000);
        let (tx_consensus, rx_consensus) = mpsc_channel(1000);
        let (tx_consensus_mempool, rx_consensus_mempool) = mpsc_channel(1000);

        // Read the committee and secret key from file.
        let committee = Committee::read(committee_file)?;
        let secret = Secret::read(key_file)?;
        let name = secret.name;
        let secret_key = secret.secret;

        // Load default parameters if none are specified.
        let parameters = match parameters {
            Some(filename) => Parameters::read(filename)?,
            None => Parameters::default(),
        };

        info!("I am adversarial: {:?}", name);

        // Start gossip
        let gossip = Gossip::run(
            name,
            committee.gossip,
            /* adversarial_privileges */ true,
        ).await?;
        time::sleep(Duration::from_millis(3000)).await;

        // Make the data store.
        let store = Store::new(store_path)?;

        // Run the signature service.
        let signature_service = SignatureService::new(secret_key);

        // Make a new mempool.
        let tx_transactions = Mempool::myrun(
            name,
            committee.mempool,
            parameters.mempool,
            store.clone(),
            signature_service.clone(),
            tx_consensus.clone(),
            rx_consensus_mempool,
            gossip.clone(),
        ).await?;

        // Run the consensus core.
        let tx_consensus_cmds = Consensus::myrun(
            name,
            committee.consensus,
            parameters.consensus,
            store.clone(),
            signature_service.clone(),
            tx_consensus,
            rx_consensus,
            tx_consensus_mempool,
            tx_commit,
            gossip.clone(),
            /* boycott_proposals */ true,
        ).await?;

        // pause consensus for the duration of the initialization (there is some second delay after joining each of gossip network topics)
        let (tx_consensus_trigger_resume, rx_consensus_trigger_resume) = tokio::sync::oneshot::channel();
        tx_consensus_cmds.send(ConsensusCommand::Pause(rx_consensus_trigger_resume)).await.expect("Failed to ask consensus to pause!");


        // Channels for communication between components
        let (tx_ledger, rx_ledger) = mpsc_channel(1000);   // HotStuff -> checkpoint extractor
        let (tx_checkpoint_events, _rx_checkpoint_events) = broadcast_channel(1000);   // checkpoint extractor -> (checkpoint proposer, lc block tree)
        let rx_checkpoint_events_cpproposer_endpoint = tx_checkpoint_events.subscribe();
        let rx_checkpoint_events_lcblocktree_endpoint = tx_checkpoint_events.subscribe();
        let (tx_tip_mining, rx_tip_mining) = mpsc_channel(1000);


        // sign up for adversarial communication
        let (mut rx_adv_msgs_inbound_msgs, tx_adv_msgs_outbound_msgs, _) = gossip.subscribe("adversary".to_string(), false).await;


        // sign up for lc block gossip
        let (mut rx_inbound_msgs, tx_outbound_msgs, _) = gossip.subscribe("lcblocks".to_string(), false).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;


        // say hi to other adversaries
        let hello_msg = AdversarialGossipMsg::Hello(name);
        let hello_msg_binary: Vec<u8> = (&hello_msg).try_into().expect("failed to serialize adversarial gossip msg!");
        tx_adv_msgs_outbound_msgs.send((hello_msg.digest().to_vec(), hello_msg_binary)).await.expect("failed to introduce myself to other adversaries!");


        // blocks from miner, intercepted by adversarial logic
        let (tx_new_locally_mined_blocks, mut rx_new_locally_mined_blocks) = mpsc_channel::<(Vec<u8>, Vec<u8>)>(1000);
        let (tx_new_mined_blocks, rx_new_mined_blocks) = mpsc_channel(1000);


        // Run LC block tree
        let blocktreemgr = MyLcBlockTreeManager::run(
            name,
            committee.checkpoint.clone(),
            parameters.checkpoint.clone(),
            rx_checkpoint_events_lcblocktree_endpoint,
            rx_new_mined_blocks, // rx_inbound_msgs,
            tx_tip_mining,
        );


        // adversarial logic
        {
            let blocktreemgr = blocktreemgr.clone();
            tokio::spawn(async move {
                info!("Adversarial main logic spawned!");

                let mut adversary_leader = name;
                let mut highest_honest_block: usize = 0;
                // let mut hidden_blocks = Vec::new();
                // let mut matched_parents = HashSet::<Digest>::new();

                loop {
                    tokio::select! {
                        // read freshly locally mined blocks from the miner from rx_new_locally_mined_blocks,
                        // broadcast these (hidden!) blocks to other adversaries
                        msg = rx_new_locally_mined_blocks.recv() => match msg {
                            Some((_k, v)) => {
                                let blk: LcBlock = v.to_vec().try_into().expect("failed to deserialize inbound LcBlock!");
                                info!("Freshly locally mined block from the miner: {:?}", blk);
                                let msg = AdversarialGossipMsg::HiddenLcBlock(blk);
                                let msg_binary: Vec<u8> = (&msg).try_into().expect("failed to serialize outbound AdversarialGossipMsg!");
                                tx_adv_msgs_outbound_msgs.send((msg.digest().to_vec(), msg_binary)).await.expect("failed to send AdversarialGossipMsg!");
                            },
                            None => {
                                panic!("Stream rx_new_locally_mined_blocks closed unexpectedly, terminating!");
                            },
                        },
                        // receive broadcasts on the adversary network (= rx_adv_msgs_inbound_msgs)
                        //  -> determine leader as the adversary with "smallest" public key from "hello" messages
                        //  -> keep track of hidden blocks and forward them to the local blocktreemanager (via tx_new_mined_blocks)
                        msg = rx_adv_msgs_inbound_msgs.recv() => match msg {
                            Some((_k, v)) => {
                                let msg: AdversarialGossipMsg = v.to_vec().try_into().expect("failed to deserialize inbound AdversarialGossipMsg!");
                                match msg {
                                    AdversarialGossipMsg::Hello(pk) => {
                                        if pk < adversary_leader {
                                            adversary_leader = pk;
                                        }
                                        info!("Adversary leader: {:?} Me? {}", adversary_leader, adversary_leader == name);
                                    },
                                    AdversarialGossipMsg::HiddenLcBlock(blk) => {
                                        info!("AdversarialGossipMsg::HiddenLcBlock: {:?}", blk);
                                        let lcblk = blk;
                                        let lcblk_binary: Vec<u8> = (&lcblk).try_into().expect("unable to serialize LC block!");
                                        tx_new_mined_blocks.send((lcblk.digest().to_vec(), lcblk_binary.clone())).await.expect("failed to deliver newly mined block!");

                                        // if I am the leader, then I should reveal this block right away if it is lower or equal
                                        // in height to the highest honest block I have seen ...
                                        // actually: this is something that every adversarial node should do -- worst case we get duplicate
                                        // gossip msg propagation, but that is OK
                                        // if adversary_leader == name {
                                        tokio::task::yield_now().await;
                                        // let blk_height = blocktreemgr.get_height_of_block(lcblk.digest()).await.expect("failed to determine block height!?");

                                        let mut blk_height = None;
                                        while blk_height == None {
                                            blk_height = blocktreemgr.get_height_of_block(lcblk.digest()).await;
                                        }
                                        let blk_height = blk_height.unwrap();

                                        info!("Publish hidden block immediately?: {:?} {} {}", lcblk, blk_height, highest_honest_block);

                                        if blk_height <= highest_honest_block {
                                            tx_outbound_msgs.send((lcblk.digest().to_vec(), lcblk_binary.clone())).await.expect("failed to broadcast LC block!");
                                        }
                                        // }
                                    },
                                }
                            },
                            None => {
                                panic!("Stream rx_adv_msgs_inbound_msgs closed unexpectedly, terminating!");
                            },
                        },
                        // receive blocks from the public broadcast network (= rx_inbound_msgs)
                        //  * forward them to the local blocktreemanager (via tx_new_mined_blocks)
                        //  * if the blocks are non-adversarial, and I am the leader,
                        //    then I need to respond with a hidden block (if possible) and
                        //    broadcast it to the full network via tx_outbound_msgs
                        msg = rx_inbound_msgs.recv() => match msg {
                            Some((k, v)) => {
                                let blk: LcBlock = v.to_vec().try_into().expect("failed to deserialize inbound LcBlock!");
                                info!("New block from the public network: {:?}", blk);
                                tx_new_mined_blocks.send((k, v)).await.expect("failed to deliver block from the network!");
                                tokio::task::yield_now().await;

                                let mut blk_height = None;
                                while blk_height == None {
                                    blk_height = blocktreemgr.get_height_of_block(blk.digest()).await;
                                }
                                let blk_height = blk_height.unwrap();

                                info!("New public block, might have to respond?!: {:?} {} {}", blk, blk_height, highest_honest_block);

                                // again, actually every adversary can do this, which improves speed of message propagation
                                // if adversary_leader == name && !blk.is_adversarial() && blk_height > highest_honest_block {
                                if !blk.is_adversarial() && blk_height > highest_honest_block {
                                    info!("I am the chief adversary here, I need to do something against this new block! {:?}", blk);
                                    let mut blk_tip = blocktreemgr.get_current_tip().await;
                                    let mut blk_tip_height = blocktreemgr.get_height_of_block(blk_tip.clone()).await.expect("failed to determine block height!?");
                                    info!("Option: {:?} Height: {}", blk_tip.clone(), blk_tip_height);

                                    if blk_tip_height >= blk_height {
                                        while blk_tip_height > blk_height {
                                            blk_tip = blocktreemgr.get_parent_of_block(blk_tip.clone()).await.expect("failed to determine block parent!?").unwrap();
                                            blk_tip_height -= 1;
                                        }

                                        let blk_adv = blocktreemgr.get_block(blk_tip).await.expect("failed to obtain block!?");
                                        if blk_adv.is_adversarial() {
                                            info!("Responding with: {:?}", blk_adv);
                                            let blk_adv_binary: Vec<u8> = (&blk_adv).try_into().expect("unable to serialize LC block!");
                                            tx_outbound_msgs.send((blk_adv.digest().to_vec(), blk_adv_binary)).await.expect("failed to broadcast LC block!");
                                        } else {
                                            info!("Responding with {:?} makes no sense as it is not adversarial ...", blk_adv);
                                        }
                                    }

                                    highest_honest_block = blk_height;
                                }
                            },
                            None => {
                                panic!("Stream rx_inbound_msgs closed unexpectedly, terminating!");
                            },
                        },
                    }
                }
            });
        }


        // Run LC block miner
        MyMiner::run(
            name,
            committee.checkpoint.clone(),
            parameters.checkpoint.clone(),
            rx_tip_mining,
            tx_new_locally_mined_blocks, // tx_outbound_msgs,
            /* produce_adversarial_blocks */ true,
        );

        // Run checkpoint extractor
        MyCheckpointExtractor::run(
            name,
            committee.checkpoint.clone(),
            parameters.checkpoint.clone(),
            rx_ledger,
            tx_checkpoint_events,
        )?;

        // Run checkpoint proposer
        MyCheckpointProposer::run(
            name,
            committee.checkpoint.clone(),
            parameters.checkpoint.clone(),
            signature_service.clone(),
            tx_transactions.clone(),
            rx_checkpoint_events_cpproposer_endpoint,
            blocktreemgr,
            tx_consensus_cmds,
            /* boycott_proposals */ true,
            gossip.clone(),
        ).await?;


        // initialization complete, resume consensus
        tx_consensus_trigger_resume.send(()).expect("Failed to ask consensus to resume!");


        info!("Node {} successfully booted (ADVERSARIAL)", name);
        Ok(Self { commit: rx_commit, store: store, ledger: tx_ledger })
    }

    pub async fn analyze_block(&mut self) {
        let mut smallest_unprocessed_round: usize = 0;
        while let Some(block) = self.commit.recv().await {
            let mut blks = Vec::new();
            let mut blk = block.clone();

            loop {
                blks.push(blk.clone());
                match self.get_parent_block(&blk).await.expect("Failed to get parent block!") {
                    None => break,
                    Some(blk_parent) => {
                        blk = blk_parent;
                    }
                }
            }
            // info!("Committed block: {:?} / Prefix: {:?}", block, blks);

            for block in blks.iter().rev() {
                if block.round as usize >= smallest_unprocessed_round {
                    smallest_unprocessed_round = (block.round as usize) + 1;
                    info!("PROCESSING BLOCK: {:?}", block);
                    // info!("Payload: {:?}", block.payload);
                    for payload_digest in block.payload.iter() {
                        let payload = self.store.read(payload_digest.to_vec()).await.expect("Failed to retrieve payload (1)").expect("Failed to retrieve payload (2)");
                        let payload = bincode::deserialize::<Payload>(&payload).expect("Failed to deserialize payload");
                        for tx in payload.transactions.iter() {
                            self.ledger.send(tx.clone()).await.expect("Error sending ledger output!");
                        }
                    }
                }
            }
        }
    }

    async fn get_parent_block(&mut self, block: &Block) -> Result<Option<Block>, NodeError> {
        if block.digest() == Block::genesis().digest() {
            return Ok(None);
        }
        if block.qc == QC::genesis() {
            return Ok(Some(Block::genesis()));
        }
        let parent = block.parent();
        match self.store.read(parent.to_vec()).await? {
            Some(bytes) => Ok(Some(bincode::deserialize(&bytes)?)),
            None => { panic!("Could not load parent block!?"); },
        }
    }
}
