mod lcblocks;
// mod gossip;

use lcblocks::{LcBlock, LcBlockPayload};
use gossip::{Gossip};

use serde::{Deserialize, Serialize};
// use anyhow::{Result};
// use bytes::BufMut as _;
// use bytes::BytesMut;
// use clap::{crate_name, crate_version, App, AppSettings};
use env_logger::Env;
// use futures::future::join_all;
// use futures::sink::SinkExt as _;
use log::{info};
// use std::net::SocketAddr;
// use tokio::net::TcpStream;
// use tokio::time::{interval, sleep, Duration, Instant};
// use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio::time::{self, Duration};
use tokio::sync::mpsc::{channel as mpsc_channel, Receiver, Sender};
use std::convert::{TryInto, TryFrom};
use crypto::{Hash};
use rand::Rng;
use std::error::{Error};
use std::collections::{HashMap, HashSet};



// #[tokio::main]
// async fn main() -> std::result::Result<(), Box<dyn Error>> {
fn main() {

}
