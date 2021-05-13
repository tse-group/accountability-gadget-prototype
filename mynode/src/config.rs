use crate::mynode::NodeError;

use consensus::{Committee as ConsensusCommittee, Parameters as ConsensusParameters};
use crypto::{generate_keypair, generate_production_keypair, PublicKey, SecretKey};
use mempool::{Committee as MempoolCommittee, Parameters as MempoolParameters};
use gossip::GossipSwarm;

use rand::rngs::StdRng;
use rand::SeedableRng as _;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::BufWriter;
use std::io::Write as _;
use std::collections::HashMap;


pub trait Export: Serialize + DeserializeOwned {
    fn read(path: &str) -> Result<Self, NodeError> {
        let reader = || -> Result<Self, std::io::Error> {
            let data = fs::read(path)?;
            Ok(serde_json::from_slice(data.as_slice())?)
        };
        reader().map_err(|e| NodeError::ReadError {
            file: path.to_string(),
            message: e.to_string(),
        })
    }

    fn write(&self, path: &str) -> Result<(), NodeError> {
        let writer = || -> Result<(), std::io::Error> {
            let file = OpenOptions::new().create(true).write(true).open(path)?;
            let mut writer = BufWriter::new(file);
            let data = serde_json::to_string_pretty(self).unwrap();
            writer.write_all(data.as_ref())?;
            writer.write_all(b"\n")?;
            Ok(())
        };
        writer().map_err(|e| NodeError::WriteError {
            file: path.to_string(),
            message: e.to_string(),
        })
    }
}


// META PARAMETERS AND COMMITTEE FOR NODE

#[derive(Serialize, Deserialize, Default)]
pub struct Parameters {
    pub consensus: ConsensusParameters,
    pub mempool: MempoolParameters,
    pub checkpoint: CheckpointParameters,
}

impl Export for Parameters {}

#[derive(Serialize, Deserialize)]
pub struct Secret {
    pub name: PublicKey,
    pub secret: SecretKey,
}

impl Secret {
    pub fn new() -> Self {
        let (name, secret) = generate_production_keypair();
        Self { name, secret }
    }
}

impl Export for Secret {}

impl Default for Secret {
    fn default() -> Self {
        let mut rng = StdRng::from_seed([0; 32]);
        let (name, secret) = generate_keypair(&mut rng);
        Self { name, secret }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Committee {
    pub consensus: ConsensusCommittee,
    pub mempool: MempoolCommittee,
    pub checkpoint: CheckpointCommittee,
    pub gossip: GossipSwarm,
}

impl Export for Committee {}


// CHECKPOINTING COMMITTEE

pub type Stake = u32;
pub type EpochNumber = u128;

#[derive(Clone, Serialize, Deserialize)]
pub struct Authority {
    pub name: PublicKey,
    pub stake: Stake,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CheckpointCommittee {
    pub authorities: HashMap<PublicKey, Authority>,
    pub epoch: EpochNumber,
}

impl CheckpointCommittee {
    pub fn new(info: Vec<(PublicKey, Stake)>, epoch: EpochNumber) -> Self {
        Self {
            authorities: info
                .into_iter()
                .map(|(name, stake)| {
                    let authority = Authority {
                        name,
                        stake,
                    };
                    (name, authority)
                })
                .collect(),
            epoch,
        }
    }

    pub fn size(&self) -> usize {
        self.authorities.len()
    }

    pub fn stake(&self, name: &PublicKey) -> Stake {
        self.authorities.get(&name).map_or_else(|| 0, |x| x.stake)
    }

    pub fn quorum_threshold(&self) -> Stake {
        // If N = 3f + 1 + k (0 <= k < 3)
        // then (2 N + 3) / 3 = 2f + 1 + (2k + 2)/3 = 2f + 1 + k = N - f
        let total_votes: Stake = self.authorities.values().map(|x| x.stake).sum();
        2 * total_votes / 3 + 1
    }
}


// CHECKPOINTING NODE PARAMETERS

#[derive(Serialize, Deserialize, Clone)]
pub struct CheckpointParameters {
    pub propose_k_deep: usize,
    pub propose_inter_checkpoint_delay: u64,
    pub propose_timeout: u64,
    pub lcmine_slot: u64,
    pub lcmine_slot_offset: u64,
    pub lctree_logava_k_deep: usize,
}

impl Default for CheckpointParameters {
    fn default() -> Self {
        Self {
            propose_k_deep: 5,
            propose_inter_checkpoint_delay: 300_000,
            propose_timeout: 60_000,
            lcmine_slot: 12_000,
            lcmine_slot_offset: 500,
            lctree_logava_k_deep: 6,
        }
    }
}
