use crate::config::{Committee, Parameters};
use crate::core::{ConsensusMessage, Core, ConsensusCommand};
use crate::error::ConsensusResult;
use crate::leader::LeaderElector;
use crate::mempool::{ConsensusMempoolMessage, MempoolDriver};
use crate::messages::Block;
use crate::synchronizer::Synchronizer;
use crypto::{PublicKey, SignatureService};
use log::{info, warn, debug};
use network::{NetMessage}; //{NetReceiver, NetSender, NetMessage};
use store::Store;
use tokio::sync::mpsc::{channel, Receiver, Sender};

use gossip::Gossip;
use ed25519_dalek::{Sha512, Digest as _};
use std::convert::{TryInto};

// #[cfg(test)]
// #[path = "tests/consensus_tests.rs"]
// pub mod consensus_tests;

pub struct Consensus;

impl Consensus {
    // #[allow(clippy::too_many_arguments)]
    // pub async fn run(
    //     name: PublicKey,
    //     committee: Committee,
    //     parameters: Parameters,
    //     store: Store,
    //     signature_service: SignatureService,
    //     tx_core: Sender<ConsensusMessage>,
    //     rx_core: Receiver<ConsensusMessage>,
    //     tx_consensus_mempool: Sender<ConsensusMempoolMessage>,
    //     tx_commit: Sender<Block>,
    // ) -> ConsensusResult<()> {
    //     info!(
    //         "Consensus timeout delay set to {} ms",
    //         parameters.timeout_delay
    //     );
    //     info!(
    //         "Consensus synchronizer retry delay set to {} ms",
    //         parameters.sync_retry_delay
    //     );
    //     info!(
    //         "Consensus max payload size set to {} B",
    //         parameters.max_payload_size
    //     );
    //     info!(
    //         "Consensus min block delay set to {} ms",
    //         parameters.min_block_delay
    //     );

    //     let (tx_network, rx_network) = channel(1000);

    //     // Make the network sender and receiver.
    //     let address = committee.address(&name).map(|mut x| {
    //         x.set_ip("0.0.0.0".parse().unwrap());
    //         x
    //     })?;
    //     let network_receiver = NetReceiver::new(address, tx_core.clone());
    //     tokio::spawn(async move {
    //         network_receiver.run().await;
    //     });

    //     let mut network_sender = NetSender::new(rx_network);
    //     tokio::spawn(async move {
    //         network_sender.run().await;
    //     });

    //     // The leader elector algorithm.
    //     let leader_elector = LeaderElector::new(committee.clone());

    //     // Make the mempool driver which will mediate our requests to the mempool.
    //     let mempool_driver = MempoolDriver::new(tx_consensus_mempool);

    //     // Make the synchronizer. This instance runs in a background thread
    //     // and asks other nodes for any block that we may be missing.
    //     let synchronizer = Synchronizer::new(
    //         name,
    //         committee.clone(),
    //         store.clone(),
    //         /* network_channel */ tx_network.clone(),
    //         /* core_channel */ tx_core,
    //         parameters.sync_retry_delay,
    //     )
    //     .await;

    //     let mut core = Core::new(
    //         name,
    //         committee,
    //         parameters,
    //         signature_service,
    //         store,
    //         leader_elector,
    //         mempool_driver,
    //         synchronizer,
    //         /* core_channel */ rx_core,
    //         /* network_channel */ tx_network,
    //         /* commit_channel */ tx_commit,
    //     );
    //     tokio::spawn(async move {
    //         core.run().await;
    //     });

    //     Ok(())
    // }

    #[allow(clippy::too_many_arguments)]
    pub async fn myrun(
        name: PublicKey,
        committee: Committee,
        parameters: Parameters,
        store: Store,
        signature_service: SignatureService,
        tx_core: Sender<ConsensusMessage>,
        rx_core: Receiver<ConsensusMessage>,
        tx_consensus_mempool: Sender<ConsensusMempoolMessage>,
        tx_commit: Sender<Block>,
        gossip: Gossip,
        boycott_proposals: bool,
    ) -> ConsensusResult<Sender<ConsensusCommand>> {
        info!(
            "Consensus timeout delay set to {} ms",
            parameters.timeout_delay
        );
        info!(
            "Consensus synchronizer retry delay set to {} ms",
            parameters.sync_retry_delay
        );
        info!(
            "Consensus max payload size set to {} B",
            parameters.max_payload_size
        );
        info!(
            "Consensus min block delay set to {} ms",
            parameters.min_block_delay
        );

        let (tx_network, mut rx_network) = channel::<NetMessage>(1000);


        fn sha512_hash(data: &Vec<u8>) -> Vec<u8> {
            let mut hasher = Sha512::new();
            hasher.update(&data);
            hasher.finalize().as_slice()[..32].try_into().unwrap()
        }

        // Hook up to the Gossip!
        let (mut rx_inbound_msgs, tx_outbound_msgs, _tx_inbound_msgs) = gossip.subscribe("consensus".to_string(), true).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

        // receive msgs from gossip
        {
            let tx_core = tx_core.clone();
            tokio::spawn(async move {
                loop {
                    let (_, msg_raw) = rx_inbound_msgs.recv().await.expect("failed to read inbound msg!");
                    let msg_deserialized = bincode::deserialize::<crate::core::ConsensusMessage>(&msg_raw).expect("failed to deserialize inbound msg!");
                    debug!("Consensus recv (len: {}): {:?}", msg_raw.len(), msg_deserialized);
                    tx_core.send(msg_deserialized).await.expect("failed to forward inbound msg to tx_core!");
                }
            });
        }
        // send msgs via gossip
        tokio::spawn(async move {
            loop {
                let outbound_msg = rx_network.recv().await.expect("failed to read outbound msg!");
                let outbound_msg_deserialized = bincode::deserialize::<crate::core::ConsensusMessage>(&outbound_msg.0.to_vec()).expect("failed to deserialize outbound msg!");
                debug!("Consensus send (len: {}): {:?}", outbound_msg.0.len(), outbound_msg_deserialized);
                match outbound_msg_deserialized {
                    crate::core::ConsensusMessage::LoopBack(_b) => {
                        warn!("*** WILL IGNORE ConsensusMessage::LoopBack FOR THE MOMENT !!! ***");
                    },
                    crate::core::ConsensusMessage::SyncRequest(_d, _key) => {
                        warn!("*** WILL IGNORE ConsensusMessage::SyncRequest FOR THE MOMENT !!! ***");
                    },
                    _ => {
                        tx_outbound_msgs.send((sha512_hash(&outbound_msg.0.to_vec()), outbound_msg.0.to_vec())).await.expect("failed to forward outbound msg to gossip!");
                    },
                }
            }
        });

        // The leader elector algorithm.
        let leader_elector = LeaderElector::new(committee.clone());

        // Make the mempool driver which will mediate our requests to the mempool.
        let mempool_driver = MempoolDriver::new(tx_consensus_mempool);

        // Make the synchronizer. This instance runs in a background thread
        // and asks other nodes for any block that we may be missing.
        let synchronizer = Synchronizer::new(
            name,
            committee.clone(),
            store.clone(),
            /* network_channel */ tx_network.clone(),
            /* core_channel */ tx_core,
            parameters.sync_retry_delay,
        )
        .await;

        let (tx_command, rx_command) = channel(1000);
        let mut core = Core::new(
            name,
            committee,
            parameters,
            signature_service,
            store,
            leader_elector,
            mempool_driver,
            synchronizer,
            /* core_channel */ rx_core,
            /* network_channel */ tx_network,
            /* commit_channel */ tx_commit,
            /* command_channel */ rx_command,
            boycott_proposals,
        );
        tokio::spawn(async move {
            core.run().await;
        });

        Ok(tx_command)
    }
}
