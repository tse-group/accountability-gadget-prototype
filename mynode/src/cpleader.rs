use crate::config::CheckpointCommittee;
use crypto::PublicKey;
use rand::prelude::*;
use rand::rngs::StdRng;


pub type LeaderElector = RandomLeaderElector;


// pub struct RoundRobinLeaderElector {
//     committee: CheckpointCommittee,
// }

// impl RoundRobinLeaderElector {
//     pub fn new(committee: CheckpointCommittee) -> Self {
//         Self { committee }
//     }

//     pub fn get_leader(&self, checkpoint_iteration: usize) -> PublicKey {
//         let mut keys: Vec<_> = self.committee.authorities.keys().cloned().collect();
//         keys.sort();
//         keys[checkpoint_iteration % self.committee.size()]
//     }
// }


fn get_random_seed(p: PublicKey) -> u64 {
    let mut v: u64 = 0;
    for i in 0..8 {
        v = (v<<8) + (p.0[i] as u64);
    }
    v
}

pub struct RandomLeaderElector {
    committee: CheckpointCommittee,
    keys: Vec<PublicKey>,
    rng_seed: u64,
}

impl RandomLeaderElector {
    pub fn new(committee: CheckpointCommittee) -> Self {
        let mut keys: Vec<_> = committee.authorities.keys().cloned().collect();
        keys.sort();
        let rng_seed = get_random_seed(keys[0]) ^ 0xdeadbeefdeadbeef;
        Self { committee, keys, rng_seed }
    }

    pub fn get_leader(&self, checkpoint_iteration: usize) -> PublicKey {
        let mut rng = StdRng::seed_from_u64(self.rng_seed + (checkpoint_iteration as u64));
        let leader_idx = rng.gen::<usize>() % self.committee.size();
        self.keys[leader_idx]
    }
}
