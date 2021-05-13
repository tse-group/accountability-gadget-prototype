use crate::config::Committee;
use crate::core::RoundNumber;
use crypto::PublicKey;
use rand::prelude::*;
use rand::rngs::StdRng;


pub type LeaderElector = RandomLeaderElector;


// pub struct RRLeaderElector {
//     committee: Committee,
// }

// impl RRLeaderElector {
//     pub fn new(committee: Committee) -> Self {
//         Self { committee }
//     }

//     pub fn get_leader(&self, round: RoundNumber) -> PublicKey {
//         let mut keys: Vec<_> = self.committee.authorities.keys().cloned().collect();
//         keys.sort();
//         keys[(round / 10) as usize % self.committee.size()]
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
    committee: Committee,
    keys: Vec<PublicKey>,
    rng_seed: u64,
}

impl RandomLeaderElector {
    pub fn new(committee: Committee) -> Self {
        let mut keys: Vec<_> = committee.authorities.keys().cloned().collect();
        keys.sort();
        let rng_seed = get_random_seed(keys[1]) ^ 0xbeefdeadbeefdead;
        Self { committee, keys, rng_seed }
    }

    pub fn get_leader(&self, round: RoundNumber) -> PublicKey {
        // let mut rng = StdRng::seed_from_u64(self.rng_seed + ((round / 2) as u64));
        let mut rng = StdRng::seed_from_u64(self.rng_seed + (round as u64));
        let leader_idx = rng.gen::<usize>() % self.committee.size();
        self.keys[leader_idx]
    }
}
