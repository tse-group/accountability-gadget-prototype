use serde::{Deserialize, Serialize};
use crypto::{Digest, Hash};
use ed25519_dalek::{Sha512, Digest as _};
use std::convert::{TryFrom, TryInto};
use std::collections::{HashMap, HashSet};
use std::fmt;


// LC BLOCK PAYLOAD

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct LcBlockPayload(pub String, pub Vec<u8>);

impl LcBlockPayload {
    pub fn new(s: String) -> LcBlockPayload {
        // effective throughput is 22KB/12s according to Eth2 spec: https://benjaminion.xyz/eth2-annotated-spec/phase0/beacon-chain/#max-operations-per-block
        return LcBlockPayload(s, vec![0; 22000]);
    }
}

impl fmt::Debug for LcBlockPayload {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_tuple("LcBlockPayload::new")
            .field(&self.0)
            .finish()
    }
}


// LC BLOCK

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct LcBlock {
    pub parent: Option<Digest>,
    pub payload: LcBlockPayload,
    pub slot: u64,
}

impl LcBlock {
    pub fn new(parent: Option<Digest>, payload: LcBlockPayload, slot: u64) -> Self {
        Self { parent, payload, slot }
    }

    pub fn genesis() -> Self {
        let mut blk = LcBlock::default();
        blk.payload.0 = "Genesis".to_string();
        blk
    }

    pub fn is_adversarial(&self) -> bool {
        self.payload.0.ends_with("[adv]")
    }
}

impl fmt::Debug for LcBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LcBlock")
         .field("_id", &self.digest())
         .field("parent", &self.parent)
         .field("payload", &self.payload)
         .field("slot", &self.slot)
         .finish()
    }
}

impl TryFrom<Vec<u8>> for LcBlock {
    type Error = Box<bincode::ErrorKind>;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        bincode::deserialize::<LcBlock>(&value)
    }
}

impl TryFrom<&LcBlock> for Vec<u8> {
    type Error = Box<bincode::ErrorKind>;

    fn try_from(value: &LcBlock) -> Result<Self, Self::Error> {
        bincode::serialize(&value)
    }
}

impl Hash for LcBlock {
    fn digest(&self) -> Digest {
        let self_binary: Vec<u8> = self.try_into().unwrap();
        let mut hasher = Sha512::new();
        hasher.update(self_binary);
        Digest(hasher.finalize().as_slice()[..32].try_into().unwrap())
    }
}


// LC BLOCK TREE

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct LcBlockTree {
    pub lcblocks: HashMap<Digest, LcBlock>,
    pub precomputed_height: HashMap<Digest, usize>,
    pub idx_tips: HashSet<Digest>,
}

impl LcBlockTree {
    pub fn new() -> LcBlockTree {
        let mut lcblocks = HashMap::new();
        let mut precomputed_height = HashMap::new();
        let mut idx_tips = HashSet::new();

        let blk = LcBlock::genesis();
        let blk_digest = blk.digest();

        lcblocks.insert(blk_digest.clone(), blk);
        precomputed_height.insert(blk_digest.clone(), 0);

        idx_tips.insert(blk_digest.clone());

        Self { lcblocks, precomputed_height, idx_tips }
    }

    pub fn add(&mut self, blk: LcBlock) -> Result<(), &'static str> {
        let blk_digest = blk.digest();
        let blk_parent_digest = blk.parent.as_ref();

        if blk_parent_digest == None {
            return Err("Parent is None, there can be only one genesis block!");
        }

        let blk_parent_digest = blk_parent_digest.unwrap();
        let blk_parent_height = match self.precomputed_height.get(blk_parent_digest) {
            Some(h) => {
                h
            },
            None => {
                return Err("Parent is unknown!");
            }
        };

        self.idx_tips.remove(blk_parent_digest);
        self.idx_tips.insert(blk_digest.clone());

        self.lcblocks.insert(blk_digest.clone(), blk);
        let new_height = blk_parent_height + 1;  // strangely unnecessary, but satisfies https://github.com/rust-lang/rust/issues/59159
        self.precomputed_height.insert(blk_digest.clone(), new_height);

        Ok(())
    }

    pub fn is_prefix_of(&self, blk_short_digest: &Digest, blk_long_digest: &Digest) -> Result<bool, &'static str> {
        if !self.lcblocks.contains_key(blk_short_digest) {
            return Err("Unknown block blk_short_digest!");
        }
        if !self.lcblocks.contains_key(blk_long_digest) {
            return Err("Unknown block blk_long_digest!");
        }

        if self.precomputed_height[blk_short_digest] > self.precomputed_height[blk_long_digest] {
            return Ok(false);
        }

        if blk_long_digest == blk_short_digest {
            return Ok(true);
        } else {
            match &self.lcblocks[blk_long_digest].parent {
                None => {
                    return Ok(false);
                },
                Some(blk_digest) => {
                    return self.is_prefix_of(blk_short_digest, &blk_digest);
                },
            }
        }
    }

    pub fn tie_break<'a>(&'a self, d1: &'a Digest, d2: &'a Digest) -> &'a Digest {
        let h1 = self.height(d1).unwrap();
        let h2 = self.height(d2).unwrap();
        let b1 = &self.lcblocks[d1];
        let b2 = &self.lcblocks[d2];

        if h1 > h2 {
            return d1;
        }
        if h2 > h1 {
            return d2;
        }

        // h1 == h2

        if b1.is_adversarial() && !b2.is_adversarial() {
            return d1;
        }
        if b2.is_adversarial() && !b1.is_adversarial() {
            return d2;
        }

        // b1 and b2 are either both adversarial or both honest

        if *d1 < *d2 {
            return d1;
        } else {
            return d2;
        }
    }

    pub fn leading_tip_extending<'a>(&'a self, blk_checkpoint: &'a Digest) -> &'a Digest {
        let mut max_height_tip = blk_checkpoint;

        for blk_digest in self.idx_tips.iter() {
            if self.tie_break(blk_digest, max_height_tip) == blk_digest
            && self.is_prefix_of(blk_checkpoint, blk_digest).expect("unexpected error in comparing blocks") {
                max_height_tip = blk_digest;
            }
        }
        
        max_height_tip
    }

    pub fn k_deep<'a>(&'a self, k: usize, b: &'a Digest) -> &'a Digest {
        if k == 0 {
            return b;
        } else {
            if self.lcblocks[b].parent == None {
                return b;
            } else {
                return self.k_deep(k-1, self.lcblocks[b].parent.as_ref().unwrap());
            }
        }
    }

    pub fn higher_of<'a>(&'a self, b1: &'a Digest, b2: &'a Digest) -> Result<&'a Digest, &'static str> {
        if !self.lcblocks.contains_key(b1) {
            return Err("Unknown block b1!");
        }
        if !self.lcblocks.contains_key(b2) {
            return Err("Unknown block b2!");
        }

        if self.precomputed_height[b1] >= self.precomputed_height[b2] {
            return Ok(b1);
        } else {
            return Ok(b2);
        }
    }

    pub fn height(&self, b: &Digest) -> Result<usize, &'static str> {
        if !self.lcblocks.contains_key(b) {
            return Err("Unknown block!");
        }

        Ok(self.precomputed_height[b])
    }

    pub fn height_honest(&self, b: &Digest) -> Result<usize, &'static str> {
        if !self.lcblocks.contains_key(b) {
            return Err("Unknown block!");
        }

        let blk = &self.lcblocks[b];
        Ok(match &(*blk).parent {
            None => 0,
            Some(d) => self.height_honest(&d.clone()).unwrap() + if !blk.is_adversarial() { 1 } else { 0 }
        })
    }

    pub fn parent(&self, b: &Digest) -> Result<Option<Digest>, &'static str> {
        if !self.lcblocks.contains_key(b) {
            return Err("Unknown block!");
        }

        Ok(self.lcblocks[b].parent.clone())
    }

    pub fn block(&self, b: &Digest) -> Result<LcBlock, &'static str> {
        if !self.lcblocks.contains_key(b) {
            return Err("Unknown block!");
        }

        Ok(self.lcblocks[b].clone())
    }

    pub fn dump_dot(&self, cps: &Vec<Digest>, cps_iterations: &Vec<usize>) -> String {
        fn export_digest(d: Digest) -> String {
            return format!("{}", d).replace("+", "").replace("/", "");
        }

        let mut v: String = "digraph G_lc {\n  rankdir=BT;\n  style=filled;\n  color=lightgrey;\n  node [shape=box,style=filled,color=white];\n".to_string();
        for blk in self.lcblocks.values() {
            let mut checkpoints: Vec<usize> = vec![];
            for (i, cp) in cps.iter().enumerate() {
                if *cp == blk.digest() {
                    checkpoints.push(cps_iterations[i]);
                }
            }
            v = format!(
                    "{}  lcblk_{} [label=\"{}\\n{}\\n{:?}\"];\n",
                    v,
                    export_digest(blk.digest()),
                    blk.digest(),
                    blk.payload.0.replace("\"", ""),
                    checkpoints
                );
        }
        for blk in self.lcblocks.values() {
            if blk.parent.clone() != None {
                v = format!("{}  lcblk_{} -> lcblk_{};\n", v, export_digest(blk.digest()), export_digest(blk.parent.clone().unwrap()));
            }
        }
        v = format!("{}}}\n", v);
        v
    }
}
