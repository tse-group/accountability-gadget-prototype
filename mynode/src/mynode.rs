use crate::config::Export as _;
use crate::config::{Committee, Parameters, Secret};

// use crate::lcblocks::{LcBlock};
use crate::cpextractor::{MyCheckpointExtractor};
use crate::cpproposer::{MyCheckpointProposer};
use crate::miner::{MyMiner};
use crate::lcblocktreemanager::{MyLcBlockTreeManager};
use crate::error::{CheckpointError};

use gossip::{Gossip, GossipError};

use consensus::{Block, Consensus, ConsensusError, ConsensusCommand, QC};
use crypto::{SignatureService, Hash};
use log::{info, error};
use mempool::{Mempool, MempoolError, Payload};
use store::{Store, StoreError};
use thiserror::Error;
use tokio::sync::{mpsc::channel as mpsc_channel, mpsc::Receiver as MpscReceiver, mpsc::Sender as MpscSender};
use tokio::sync::{broadcast::channel as broadcast_channel}; //, broadcast::Receiver as BroadcastReceiver, broadcast::Sender as BroadcastSender};
// use tokio::sync::{watch::channel as watch_channel, watch::Receiver as WatchReceiver, watch::Sender as WatchSender};
use tokio::time::{self, Duration};


#[derive(Error, Debug)]
pub enum NodeError {
    #[error("Failed to read config file '{file}': {message}")]
    ReadError { file: String, message: String },

    #[error("Failed to write config file '{file}': {message}")]
    WriteError { file: String, message: String },

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

    #[error("Serialization error: {0}")]
    SerializationError(#[from] Box<bincode::ErrorKind>),
}

pub struct Node {
    pub commit: MpscReceiver<Block>,
    pub store: Store,
    pub ledger: MpscSender<Vec<u8>>,
}

impl Node {
    pub async fn new(
        committee_file: &str,
        key_file: &str,
        store_path: &str,
        parameters: Option<&str>,
    ) -> Result<Self, NodeError> {

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

        info!("I am honest: {:?}", name);

        // Start gossip
        let gossip = Gossip::run(
            name,
            committee.gossip,
            /* adversarial_privileges */ false,
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
            /* boycott_proposals */ false,
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


        // sign up for lc block gossip
        let (rx_inbound_msgs, tx_outbound_msgs, _) = gossip.subscribe("lcblocks".to_string(), false).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

        // Run LC block tree
        let blocktreemgr = MyLcBlockTreeManager::run(
            name,
            committee.checkpoint.clone(),
            parameters.checkpoint.clone(),
            rx_checkpoint_events_lcblocktree_endpoint,
            rx_inbound_msgs,
            tx_tip_mining,
        );

        // Run LC block miner
        MyMiner::run(
            name,
            committee.checkpoint.clone(),
            parameters.checkpoint.clone(),
            rx_tip_mining,
            tx_outbound_msgs,
            /* produce_adversarial_blocks */ false,
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
            /* boycott_proposals */ false,
            gossip.clone(),
        ).await?;


        // initialization complete, resume consensus
        tx_consensus_trigger_resume.send(()).expect("Failed to ask consensus to resume!");


        info!("Node {} successfully booted (HONEST)", name);
        Ok(Self { commit: rx_commit, store: store, ledger: tx_ledger })
    }

    pub fn print_key_file(filename: &str) -> Result<(), NodeError> {
        Secret::new().write(filename)
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