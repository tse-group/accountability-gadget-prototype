use crate::error::{CheckpointResult, CheckpointError};
use crate::ensure;
use crate::config::CheckpointCommittee;

use crypto::{Digest, Hash, PublicKey, Signature, SignatureService};

use std::convert::{TryFrom, TryInto};
use serde::{Deserialize, Serialize};
use ed25519_dalek::Digest as _;
use ed25519_dalek::Sha512;
use std::fmt;


#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum CheckpointAction {
    Propose(usize, Digest),
    Accept(usize, Digest),
    Reject(usize),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckpointTransaction {
    pub author: PublicKey,
    pub action: CheckpointAction,
    pub signature: Signature,
}

impl CheckpointTransaction {
    pub async fn new(author: PublicKey, action: CheckpointAction, mut signature_service: SignatureService) -> Self {
        let transaction = Self {
            author,
            action,
            signature: Signature::default(),
        };
        let signature = signature_service.request_signature(transaction.digest()).await;
        Self { signature, ..transaction }
    }

    pub fn verify_signature(&self, committee: &CheckpointCommittee) -> CheckpointResult<()> {
        // Ensure the authority has voting rights.
        let voting_rights = committee.stake(&self.author);
        ensure!(
            voting_rights > 0,
            CheckpointError::UnknownAuthority(self.author)
        );

        // Check the signature.
        let mut self_without_signature: CheckpointTransaction = self.clone();
        self_without_signature.signature = Signature::default();
        self.signature.verify(&self_without_signature.digest(), &self.author)?;

        Ok(())
    }
}

impl TryFrom<Vec<u8>> for CheckpointTransaction {
    type Error = Box<bincode::ErrorKind>;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        bincode::deserialize::<CheckpointTransaction>(&value)
    }
}

impl TryFrom<&CheckpointTransaction> for Vec<u8> {
    type Error = Box<bincode::ErrorKind>;

    fn try_from(value: &CheckpointTransaction) -> Result<Self, Self::Error> {
        bincode::serialize(&value)
    }
}

impl TryFrom<CheckpointTransaction> for Vec<u8> {
    type Error = Box<bincode::ErrorKind>;

    fn try_from(value: CheckpointTransaction) -> Result<Self, Self::Error> {
        (&value).try_into()
    }
}

impl Hash for CheckpointTransaction {
    fn digest(&self) -> Digest {
        let self_binary: Vec<u8> = self.try_into().unwrap();
        let mut hasher = Sha512::new();
        hasher.update(self_binary);
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}

impl fmt::Display for CheckpointTransaction {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "ChkptTx({}, {:?})", self.author, self.action, )
    }
}
