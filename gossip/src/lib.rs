mod config;

pub use crate::config::GossipSwarm;

use crypto::PublicKey;

// use libp2p::mdns::{Mdns, MdnsEvent};
use libp2p::ping::{Ping, PingEvent, PingConfig};
use libp2p::gossipsub::{MessageId, GossipsubEvent, GossipsubMessage, IdentTopic as Topic, MessageAuthenticity, ValidationMode};
use libp2p::kad::{record::store::MemoryStore, Kademlia, KademliaEvent, Quorum, Record, record::Key, QueryResult, PeerRecord};
use libp2p::swarm::{SwarmBuilder};
use libp2p::{gossipsub, identity, PeerId, NetworkBehaviour};
use libp2p::core::multiaddr::{Protocol};
use libp2p::Transport;

use tokio::time::{Duration};
use tokio::sync::{mpsc::channel as mpsc_channel, mpsc::Receiver as MpscReceiver, mpsc::Sender as MpscSender};

use ed25519_dalek::{Sha512, Digest as _};

use std::convert::{TryInto, From};
use std::collections::{HashMap};
use std::net::Ipv4Addr;
use log::{info, warn};
use thiserror::Error;

use bincode;


fn sha512_hash(data: &Vec<u8>) -> Vec<u8> {
    let mut hasher = Sha512::new();
    hasher.update(&data);
    // hasher.finalize().as_slice()[..32].try_into().unwrap()
    hasher.finalize().as_slice()[..16].try_into().unwrap()
}

fn kv_serialize(k: &Vec<u8>, v: &Vec<u8>) -> Vec<u8> {
    bincode::serialize(&(k, v)).expect("failed to serialize (k, v)!")
}

fn kv_deserialize(d: &Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    bincode::deserialize::<(Vec<u8>, Vec<u8>)>(d).expect("failed to deserialize (k, v)!")
}


pub async fn my_development_transport(local_key: libp2p::identity::Keypair)
    -> std::io::Result<libp2p::core::transport::Boxed<(libp2p::PeerId, libp2p::core::muxing::StreamMuxerBox)>>
{
    let transport = {
        let tcp = libp2p::tcp::TcpConfig::new().nodelay(true);
        // let dns_tcp = dns::DnsConfig::system(tcp).await?;
        // let ws_dns_tcp = websocket::WsConfig::new(dns_tcp.clone());
        // dns_tcp.or_transport(ws_dns_tcp)
        tcp
    };

    // let noise_keys = noise::Keypair::<noise::X25519Spec>::new()
    //     .into_authentic(&keypair)
    //     .expect("Signing libp2p-noise static DH keypair failed.");

    // use plaintext::PlainText2Config;
    // use std::convert::TryInto;
    // use std::str::FromStr;

    // let local_key = identity::Keypair::generate_ed25519();
    // let local_public_key = local_key.public();
    // let local_peer_id = local_public_key.clone().into_peer_id();

    let local_public_key = local_key.public();
    let plain = libp2p::plaintext::PlainText2Config {
        local_public_key: local_public_key,
    };

    Ok(transport
        .upgrade(libp2p::core::upgrade::Version::V1)
        // .upgrade(::Version::V1)
        // .and_then(|conn, endpoint| {
        //     libp2p::core::upgrade::apply(conn, libp2p::deflate::DeflateConfig::default(), endpoint, libp2p::core::upgrade::Version::V1)
        // })
        // .authenticate(noise::NoiseConfig::xx(noise_keys).into_authenticated())
        .authenticate(plain)
        // .apply(libp2p::deflate::DeflateConfig::default())
        // .apply(libp2p::deflate::DeflateConfig {
        //     compression: flate2::Compression::best(),
        // })
        .multiplex(libp2p::yamux::YamuxConfig::default())
        .timeout(std::time::Duration::from_secs(20))
        .boxed())
}


pub type GossipResult<T> = Result<T, GossipError>;

#[derive(Error, Debug)]
pub enum GossipError {
    #[error("IO error: {0}")]
    IOError(#[from] std::io::Error),

    #[error("Libp2p transport error")]
    TransportError(#[from] libp2p::TransportError<std::io::Error>),

    #[error("Libp2p gossipsub subscription error")]
    SubscriptionError(),
}

impl From<libp2p::gossipsub::error::SubscriptionError> for GossipError {
    fn from(_value: libp2p::gossipsub::error::SubscriptionError) -> GossipError {
        GossipError::SubscriptionError {}
    }
}


#[derive(Debug)]
enum MyOutEvent {
    // Mdns { e: MdnsEvent },
    Ping { e: PingEvent },
    Gossipsub { e: GossipsubEvent },
    Kademlia { e: KademliaEvent },
}

// impl From<MdnsEvent> for MyOutEvent {
//     fn from(value: MdnsEvent) -> MyOutEvent {
//         MyOutEvent::Mdns { e: value }
//     }
// }

impl From<PingEvent> for MyOutEvent {
    fn from(value: PingEvent) -> MyOutEvent {
        MyOutEvent::Ping { e: value }
    }
}

impl From<GossipsubEvent> for MyOutEvent {
    fn from(value: GossipsubEvent) -> MyOutEvent {
        MyOutEvent::Gossipsub { e: value }
    }
}

impl From<KademliaEvent> for MyOutEvent {
    fn from(value: KademliaEvent) -> MyOutEvent {
        MyOutEvent::Kademlia { e: value }
    }
}


// Custom network behavior
#[derive(NetworkBehaviour)]
#[behaviour(out_event = "MyOutEvent", event_process = false)]
struct MyBehaviour {
    // mdns: Mdns,
    ping: Ping,
    gossipsub: gossipsub::Gossipsub,
    kademlia: Kademlia<MemoryStore>,
}


#[derive(Debug, Clone)]
pub enum GossipCmd {
    Subscribe(String, MpscSender<(Vec<u8>, Vec<u8>)>),
    Publish(String, Vec<u8>, Vec<u8>, bool),
    Retrieve(Vec<u8>, MpscSender<(Vec<u8>, Vec<u8>)>),
}


#[derive(Debug, Clone)]
pub struct Gossip(MpscSender<GossipCmd>);

impl Gossip {
    pub async fn run(
            name: PublicKey,
            swarm_config: GossipSwarm,
            adversarial_privileges: bool,
        ) -> Result<Gossip, GossipError> {

        // BASED ON rust-libp2p EXAMPLES: https://github.com/libp2p/rust-libp2p/tree/v0.36.0/examples


        // Create a random PeerId
        let local_key = identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());
        info!("Local gossip peer id: {:?}", local_peer_id);

        // Set up an encrypted TCP Transport over the Mplex and Yamux protocols
        let transport = my_development_transport(local_key).await?;
        // let transport = libp2p::development_transport(local_key.clone()).await?;
        // let transport = libp2p::tokio_development_transport(local_key.clone())?;   // we have the wrong feature flags for this


        // Create a Swarm to manage peers and events
        let mut swarm = {
            // let mdns = Mdns::new(Default::default()).await?;
            let ping = Ping::new(
                PingConfig::new()
                .with_timeout(Duration::from_secs(5))
                .with_interval(Duration::from_secs(5))
                .with_max_failures(2u32.try_into().unwrap())
                .with_keep_alive(true)
            );

            let gossipsub_config = if adversarial_privileges {
                // Set a custom gossipsub
                gossipsub::GossipsubConfigBuilder::default()
                    // .heartbeat_interval(Duration::from_secs(2))   // This is set to aid debugging by not cluttering the log space
                    // .validation_mode(ValidationMode::Strict)   // This sets the kind of message validation. The default is Strict (enforce message signing)
                    .validation_mode(ValidationMode::Anonymous)   // This sets the kind of message validation. Enforce anonymity.
                    .message_id_fn(|message: &GossipsubMessage| {   // To content-address message, we can take the hash of message and use it as an ID.
                        MessageId(sha512_hash(&message.data))
                    })   // content-address messages. No two messages of the same content will be propagated.
                    .max_transmit_size(4*16*65536)
                    // .flood_publish(false)
                    // .mesh_outbound_min(1)
                    .mesh_n(4)
                    .mesh_n_low(3)
                    // .history_gossip(2)
                    // .gossip_lazy(3)
                    // .max_ihave_length(100)
                    .build()
                    .expect("Valid config")
            } else {
                // Set a custom gossipsub
                gossipsub::GossipsubConfigBuilder::default()
                    // .heartbeat_interval(Duration::from_secs(2))   // This is set to aid debugging by not cluttering the log space
                    // .validation_mode(ValidationMode::Strict)   // This sets the kind of message validation. The default is Strict (enforce message signing)
                    .validation_mode(ValidationMode::Anonymous)   // This sets the kind of message validation. Enforce anonymity.
                    .message_id_fn(|message: &GossipsubMessage| {   // To content-address message, we can take the hash of message and use it as an ID.
                        MessageId(sha512_hash(&message.data))
                    })   // content-address messages. No two messages of the same content will be propagated.
                    .max_transmit_size(4*16*65536)
                    .flood_publish(false)
                    .mesh_outbound_min(1)
                    .mesh_n(2)
                    .mesh_n_low(2)
                    .history_gossip(2)
                    .gossip_lazy(3)
                    .max_ihave_length(100)
                    .build()
                    .expect("Valid config")
            };

            // build a gossipsub network behaviour
            let gossipsub: gossipsub::Gossipsub =
                // gossipsub::Gossipsub::new(MessageAuthenticity::Signed(local_key), gossipsub_config)
                gossipsub::Gossipsub::new(MessageAuthenticity::Anonymous, gossipsub_config)
                    .expect("Correct configuration");

            // kademlia
            let store = MemoryStore::new(local_peer_id.clone());
            let kademlia = Kademlia::new(local_peer_id.clone(), store);

            // putting it all together
            let behaviour = MyBehaviour {
                // mdns: mdns,
                ping: ping,
                gossipsub: gossipsub,
                kademlia: kademlia,
            };

            // build the swarm
            SwarmBuilder::new(transport, behaviour, local_peer_id)
                // We want the connection background tasks to be spawned
                // onto the tokio runtime.
                .executor(Box::new(|fut| { tokio::spawn(fut); }))
                .build()
        };


        // retrieve my config
        let my_config = swarm_config.nodes.get(&name).expect("failed to retrieve swarm config info for myself!");

        // Listen on all interfaces and whatever port the OS assigns
        let listen_everywhere = my_config.address.replace(0, |_| { Some(Protocol::Ip4(Ipv4Addr::new(0, 0, 0, 0))) }).unwrap();
        info!("Listening on: {:?}", listen_everywhere);
        libp2p::Swarm::listen_on(&mut swarm, listen_everywhere)?;


        // Reach out to other nodes as specified in swarm config
        for connect_to_name in my_config.connect_to.iter() {
            let connect_to_peer_config = swarm_config.nodes.get(&connect_to_name).expect("failed to retrieve swarm config info for connect_to!");
            match libp2p::Swarm::dial_addr(&mut swarm, connect_to_peer_config.address.clone()) {
                Ok(_) => info!("Dialed {:?}", connect_to_peer_config),
                Err(e) => info!("Dial {:?} failed: {:?}", connect_to_peer_config, e),
            }
        }


        // queues for the gossip unit (commands and outgoing messages)
        let (tx_cmds, mut rx_cmds) = mpsc_channel(1000);

        {
            let _tx_cmds = tx_cmds.clone();
            tokio::spawn(async move {
                info!("Gossip started!");

                let mut subscriptions = HashMap::<String, MpscSender<(Vec<u8>, Vec<u8>)>>::new();
                let mut pending_retrievals = HashMap::<Vec<u8>, Vec<MpscSender<(Vec<u8>, Vec<u8>)>>>::new();

                // swarm.kademlia.bootstrap().expect("Kademlia bootstrap");

                loop {
                    tokio::select! {
                        cmd = rx_cmds.recv() => {
                            // info!("GossipCmd: {:?}", cmd.clone());

                            match cmd {
                                Some(cmd) => match cmd {
                                    GossipCmd::Subscribe(topic, tx) => {
                                        subscriptions.insert(topic.clone(), tx);
                                        swarm.gossipsub.subscribe(&Topic::new(topic.clone())).expect("gossipsub subscribe failed!");
                                    },
                                    GossipCmd::Publish(topic, key, value, ephemeral) => {
                                        if !ephemeral {
                                            let record = Record { key: Key::new(&key.clone()), value: value.clone(), publisher: None, expires: None };
                                            swarm.kademlia.put_record(record, Quorum::One).expect("kademlia put record failed!");
                                        }
                                        
                                        let publish_ret = swarm.gossipsub.publish(Topic::new(topic.clone()), kv_serialize(&key, &value));
                                        if publish_ret.is_err() {
                                            warn!("Gossipsub publish failed: {:?}", publish_ret);
                                        }

                                        // // retrieve entry from Kademlia, rather than local replay
                                        // tx_cmds.send(
                                        //     GossipCmd::Retrieve(
                                        //         key_raw.clone(),
                                        //         subscriptions.get(&topic).expect("topic not subscribed!").clone()
                                        //     )
                                        // ).await.expect("sending GossipCmd::Retrieve failed!");
                                        subscriptions.get(&topic).expect("topic not subscribed!")
                                            .send((key, value)).await.expect("failed local replay!");
                                    },
                                    GossipCmd::Retrieve(key, tx) => {
                                        pending_retrievals.entry(key.clone()).or_insert(Vec::new()).push(tx);
                                        swarm.kademlia.get_record(&Key::new(&key), Quorum::One);
                                    },
                                },
                                None => {
                                    panic!("Stream of incoming gossip commands was closed, terminating ...");
                                }
                            }
                        },

                        ev = swarm.next() => {
                            // info!("Swarm next() returned: {:?}", ev);

                            match ev {
                                // MyOutEvent::Mdns { e: event } => {
                                //     match event {
                                //         MdnsEvent::Discovered(list) => {
                                //             // info!("list length: {}", list.len());
                                //             for (peer, addr) in list {
                                //                 // info!("MdnsEvent::Discovered {} {}", peer, addr);
                                //                 swarm.gossipsub.add_explicit_peer(&peer);
                                //                 swarm.kademlia.add_address(&peer, addr);
                                //             }
                                //         },
                                //         MdnsEvent::Expired(list) => {
                                //             // info!("list length: {}", list.len());
                                //             for (peer, addr) in list {
                                //                 // info!("MdnsEvent::Expired {} {}", peer, addr);
                                //                 if !swarm.mdns.has_node(&peer) {
                                //                     swarm.gossipsub.remove_explicit_peer(&peer);
                                //                     swarm.kademlia.remove_address(&peer, &addr);
                                //                 }
                                //             }
                                //         },
                                //     }
                                // },

                                MyOutEvent::Ping { e: _event } => {
                                    // info!("PingEvent: {:?}", event);
                                },

                                MyOutEvent::Gossipsub { e: event } => {
                                    // info!("GossipsubEvent: {:?}", event);

                                    match event {
                                        GossipsubEvent::Message { message, .. } => {
                                            let (key, value) = kv_deserialize(&message.data);
                                            subscriptions.get(&message.topic.into_string()).expect("topic not subscribed!")
                                                .send((key, value)).await.expect("failed local delivery!");
                                            // let key_raw = message.data;
                                            // tx_cmds.send(
                                            //     GossipCmd::Retrieve(
                                            //         key_raw,
                                            //         subscriptions.get(&message.topic.into_string()).expect("topic not subscribed!").clone()
                                            //     )
                                            // ).await.expect("sending GossipCmd::Retrieve failed!");
                                        },
                                        _ => { },
                                    };
                                },

                                MyOutEvent::Kademlia { e: event } => {
                                    // info!("KademliaEvent: {:?}", event);

                                    match event {
                                        KademliaEvent::QueryResult { result, .. } => match result {
                                            QueryResult::GetRecord(Ok(ok)) => {
                                                for PeerRecord { record: Record { key, value, .. }, ..} in ok.records {
                                                    match pending_retrievals.remove_entry(&key.to_vec()) {
                                                        Some((_k, v)) => {
                                                            for tx in v.iter() {
                                                                tx.send((key.to_vec(), value.clone())).await.expect("delivery of incoming message failed!");
                                                            }
                                                        },
                                                        None => {
                                                            panic!("Received query result for unknown pending retrieval!");
                                                        }
                                                    }
                                                }
                                            },
                                            QueryResult::GetRecord(Err(err)) => {
                                                warn!("Failed to get record: {:?}", err);
                                            },
                                            _ => { },
                                        },
                                        _ => { },
                                    };
                                },
                            }
                        },
                    }
                }

                // info!("Gossip finished!");
            });
        }

        Ok(Self(tx_cmds))
    }

    pub async fn subscribe(&self, topic: String, ephemeral: bool) -> (MpscReceiver<(Vec<u8>, Vec<u8>)>, MpscSender<(Vec<u8>, Vec<u8>)>, MpscSender<(Vec<u8>, Vec<u8>)>) {
        let (tx_outbound_msgs, mut rx_outbound_msgs): (MpscSender<(Vec<u8>, Vec<u8>)>, MpscReceiver<(Vec<u8>, Vec<u8>)>) = mpsc_channel(1000);
        let (tx_inbound_msgs, rx_inbound_msgs) = mpsc_channel(10000);
        let tx_cmds = self.0.clone();

        tx_cmds.send(GossipCmd::Subscribe(topic.clone(), tx_inbound_msgs.clone())).await.expect("sending GossipCmd::Subscribe failed!");

        tokio::spawn(async move {
            loop {
                let msg = rx_outbound_msgs.recv().await;
                match msg {
                    Some(msg) => {
                        tx_cmds.send(GossipCmd::Publish(topic.clone(), msg.0, msg.1, ephemeral)).await.expect("sending GossipCmd::Publish failed!");
                    },
                    None => {
                        warn!("Stream of outbound messages to \"{}\" was closed, terminating ...", topic.clone());
                        return;
                    }
                }
            }
        });

        (rx_inbound_msgs, tx_outbound_msgs.clone(), tx_inbound_msgs.clone())
    }
}
