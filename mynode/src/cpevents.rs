use serde::{Deserialize, Serialize};
use crypto::{Digest}; //, Hash};
// use ed25519_dalek::{Sha512, Digest as _};
// use std::convert::{TryFrom, TryInto};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum CheckpointEvent {
    NewProposal(usize, Digest),
    NewCheckpoint(usize, Option<Digest>),
}

// impl TryFrom<Vec<u8>> for CheckpointEvent {
//     type Error = Box<bincode::ErrorKind>;

//     fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
//         bincode::deserialize::<CheckpointEvent>(&value)
//     }
// }

// impl TryFrom<&CheckpointEvent> for Vec<u8> {
//     type Error = Box<bincode::ErrorKind>;

//     fn try_from(value: &CheckpointEvent) -> Result<Self, Self::Error> {
//         bincode::serialize(&value)
//     }
// }

// impl Hash for CheckpointEvent {
//     fn digest(&self) -> Digest {
//         let self_binary: Vec<u8> = self.try_into().unwrap();
//         let mut hasher = Sha512::new();
//         hasher.update(self_binary);
//         Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
//     }
// }
