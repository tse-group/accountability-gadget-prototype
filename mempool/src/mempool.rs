use crate::config::{Committee, Parameters};
use crate::core::Core;
use crate::error::MempoolResult;
use crate::front::Front;
use crate::payload::PayloadMaker;
use crate::synchronizer::Synchronizer;
use consensus::{ConsensusMempoolMessage, ConsensusMessage};
use crypto::{PublicKey, SignatureService, Hash};
use log::{info, warn, debug};
use network::{NetReceiver, NetSender, NetMessage};
use store::Store;
use tokio::sync::mpsc::{channel, Receiver, Sender};

use gossip::Gossip;


#[cfg(test)]
#[path = "tests/mempool_tests.rs"]
pub mod mempool_tests;

pub struct Mempool;

impl Mempool {
    pub fn run(
        name: PublicKey,
        committee: Committee,
        parameters: Parameters,
        store: Store,
        signature_service: SignatureService,
        consensus_channel: Sender<ConsensusMessage>,
        consensus_mempool_channel: Receiver<ConsensusMempoolMessage>,
    ) -> MempoolResult<()> {
        info!(
            "Mempool queue capacity set to {} payloads",
            parameters.queue_capacity
        );
        info!(
            "Mempool max payload size set to {} B",
            parameters.max_payload_size
        );
        info!(
            "Mempool min block delay set to {} ms",
            parameters.min_block_delay
        );

        let (tx_network, rx_network) = channel(1000);
        let (tx_core, rx_core) = channel(1000);
        let (tx_client, rx_client) = channel(1000);

        // Run the front end that receives client transactions.
        let address = committee.front_address(&name).map(|mut x| {
            x.set_ip("0.0.0.0".parse().unwrap());
            x
        })?;

        let front = Front::new(address, tx_client);
        tokio::spawn(async move {
            front.run().await;
        });

        // Run the mempool network sender and receiver.
        let address = committee.mempool_address(&name).map(|mut x| {
            x.set_ip("0.0.0.0".parse().unwrap());
            x
        })?;
        let network_receiver = NetReceiver::new(address, tx_core.clone());
        tokio::spawn(async move {
            network_receiver.run().await;
        });

        let mut network_sender = NetSender::new(rx_network);
        tokio::spawn(async move {
            network_sender.run().await;
        });

        // Build and run the synchronizer.
        let synchronizer = Synchronizer::new(
            consensus_channel,
            store.clone(),
            name,
            committee.clone(),
            tx_network.clone(),
            parameters.sync_retry_delay,
        );

        // Build and run the payload maker.
        let payload_maker = PayloadMaker::new(
            name,
            signature_service,
            parameters.max_payload_size,
            parameters.min_block_delay,
            rx_client,
            tx_core,
        );

        // Run the core.
        let mut core = Core::new(
            name,
            committee,
            parameters,
            store,
            synchronizer,
            payload_maker,
            /* core_channel */ rx_core,
            consensus_mempool_channel,
            /* network_channel */ tx_network,
        );
        tokio::spawn(async move {
            core.run().await;
        });

        Ok(())
    }

    pub async fn myrun(
        name: PublicKey,
        committee: Committee,
        parameters: Parameters,
        store: Store,
        signature_service: SignatureService,
        consensus_channel: Sender<ConsensusMessage>,
        consensus_mempool_channel: Receiver<ConsensusMempoolMessage>,
        gossip: Gossip,
    ) -> MempoolResult<Sender<Vec<u8>>> {
        info!(
            "Mempool queue capacity set to {} payloads",
            parameters.queue_capacity
        );
        info!(
            "Mempool max payload size set to {} B",
            parameters.max_payload_size
        );
        info!(
            "Mempool min block delay set to {} ms",
            parameters.min_block_delay
        );

        let (tx_network, mut rx_network) = channel::<NetMessage>(1000);
        let (tx_core, rx_core) = channel(1000);
        let (tx_client, rx_client) = channel(1000);

        // Hook up to the Gossip!
        let (mut rx_inbound_msgs, tx_outbound_msgs, _tx_inbound_msgs) = gossip.subscribe("mempool".to_string(), false).await;
        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

        // receive msgs from gossip
        {
            let tx_core = tx_core.clone();
            tokio::spawn(async move {
                loop {
                    let msg_raw = rx_inbound_msgs.recv().await.expect("failed to read inbound msg!").1;
                    let inbound_payload = bincode::deserialize::<crate::messages::Payload>(&msg_raw).expect("failed to deserialize payload!");
                    debug!("Mempool recv (len: {}): {:?}", msg_raw.len(), inbound_payload);
                    tx_core.send(crate::core::MempoolMessage::Payload(inbound_payload)).await.expect("failed to forward inbound msg to tx_core!");
                }
            });
        }
        // send msgs via gossip
        tokio::spawn(async move {
            loop {
                let outbound_msg = rx_network.recv().await.expect("failed to read outbound msg!");
                let outbound_msg_deserialized = bincode::deserialize::<crate::core::MempoolMessage>(&outbound_msg.0.to_vec()).expect("failed to deserialize outbound msg!");
                debug!("Mempool send (len: {}): {:?}", outbound_msg.0.len(), outbound_msg_deserialized);
                match outbound_msg_deserialized {
                    crate::core::MempoolMessage::Payload(p) => {
                        let payload_serialized = bincode::serialize(&p).expect("failed to serialize payload!");
                        tx_outbound_msgs.send((p.digest().to_vec(), payload_serialized)).await.expect("failed to forward outbound msg to gossip!");
                    },
                    crate::core::MempoolMessage::PayloadRequest(_ds, _key) => {
                        warn!("*** WILL IGNORE MempoolMessage::PayloadRequest FOR THE MOMENT !!! ***");
                    },
                    crate::core::MempoolMessage::OwnPayload(p) => {
                        panic!("Unexpected mempool message: crate::core::MempoolMessage::OwnPayload(p) {:?}", p);
                    },
                }
            }
        });

        // Build and run the synchronizer.
        let synchronizer = Synchronizer::new(
            consensus_channel,
            store.clone(),
            name,
            committee.clone(),
            tx_network.clone(),
            parameters.sync_retry_delay,
        );

        // Build and run the payload maker.
        let payload_maker = PayloadMaker::new(
            name,
            signature_service,
            parameters.max_payload_size,
            parameters.min_block_delay,
            rx_client,
            tx_core,
        );

        // Run the core.
        let mut core = Core::new(
            name,
            committee,
            parameters,
            store,
            synchronizer,
            payload_maker,
            /* core_channel */ rx_core,
            consensus_mempool_channel,
            /* network_channel */ tx_network,
        );
        tokio::spawn(async move {
            core.run().await;
        });

        Ok(tx_client.clone())
    }
}
