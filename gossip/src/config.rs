use crypto::{PublicKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use libp2p::{Multiaddr};


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipNode {
    pub name: PublicKey,
    pub address: Multiaddr,
    pub connect_to: Vec<PublicKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipSwarm {
    pub nodes: HashMap<PublicKey, GossipNode>,
}

impl GossipSwarm {
    pub fn new(info: Vec<(PublicKey, Multiaddr, Vec<PublicKey>)>) -> Self {
        Self {
            nodes: info
                .into_iter()
                .map(|(name, address, connect_to)| {
                    let authority = GossipNode {
                        name,
                        address,
                        connect_to,
                    };
                    (name, authority)
                })
                .collect(),
        }
    }

    pub fn size(&self) -> usize {
        self.nodes.len()
    }
}
