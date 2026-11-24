//! Myotis's embedded Swarm reader: an [`ant`](https://github.com/freedom-hq/ant) light node,
//! meant to be linked into the engine, which reaches Swarm over libp2p
//! (`.claude/agent/data-sources.md`, the carve-out for Swarm reads of the address index).
//!
//! This crate only fetches single chunks. It hands back what Swarm delivered, in wire form;
//! ant checks a content-addressed chunk against its address, and the caller checks every
//! chunk against what it expects (a SOC's address and owner signature, a CAC's address) before
//! using it, and joins files itself from chunks it has checked. It never serves HTTP and never
//! uploads.
//!
//! The node runs on a tokio runtime of its own, so that starting it needs no ambient runtime
//! (the engine's FFI entry points run on threads without one) and stopping it is certain:
//! [`SwarmNode::shutdown`] (or dropping the node) shuts that runtime down, which ends every
//! task ant spawned and its connections. It is driven through ant's command
//! channel ([`ant_control::ControlCommand`]), the in-process interface `antd`'s own control
//! socket uses.
//!
//! **Linked into the engine behind its `swarm` feature, which every shipped build turns on**
//! (desktop and Android; owner's decision, 2026-11-24). On 2026-11-20 the owner had ant's
//! three open items settled before any release that turns `swarm` on; on 2026-11-24 the owner
//! decided to patch the vendored copy for the first two and send the same changes upstream,
//! and to accept the third for now:
//! - **Fixed by a patch: outbound only** (freedom-hq/ant#269, `rust/vendor/ant-patches/0001`).
//!   Unpatched, ant listens on `0.0.0.0` and runs libp2p's UPnP behaviour, which asks the
//!   local router for a port mapping and announces the public address to peers. This crate
//!   starts every node with ant's `outbound_only`: no listener, no UPnP, and no external
//!   address of ours announced (neither a mapped one nor one a peer observed). Peers see the
//!   address of the connection the node dials them from, as for any node behind NAT.
//! - **Fixed by a patch: no DNS** (freedom-hq/ant#270, `rust/vendor/ant-patches/0002`).
//!   Unpatched, ant resolves `/dnsaddr` bootnodes with hickory-resolver 0.25.2, the version
//!   with known advisories that `myotis-net` keeps out of its own graph on purpose, and on
//!   Android, which has no `/etc/resolv.conf`, through Cloudflare's resolver. The patch makes
//!   that DNS (and libp2p's DNS transport) ant-p2p's `dnsaddr` feature, which this crate
//!   leaves off: neither is built, the node makes no DNS query at all, and it joins Swarm
//!   through [`MAINNET_BOOTNODES`], IP multiaddrs pinned here.
//! - **Accepted for now: a panic aborts the host** (freedom-hq/ant#271, open; owner's
//!   decision, 2026-11-24). ant runs inside the engine under the workspace's
//!   `panic = "abort"`: a panic anywhere in its networking code, on input from any Swarm
//!   peer, ends the host process.
//!
//! ant itself is vendored (`rust/vendor/ant`, its `VENDORED.md`): no build fetches it from
//! GitHub, and the copy is upstream's at one commit plus exactly those patches, each dropped
//! once upstream has merged its PR. Being on no registry, ant has no advisory coverage:
//! nothing reports a security fix in ant, which reaches Myotis only when the copy is moved
//! (`VENDORED.md`, "Watching upstream").

use std::path::PathBuf;
use std::time::{Duration, Instant};

use ant_control::{ControlAck, ControlCommand, StatusSnapshot};
use ant_node::{run_node, NodeConfig};
use libp2p::identity::Keypair;
use tokio::sync::{mpsc, oneshot, watch};

/// Swarm mainnet's network id.
pub const MAINNET_NETWORK_ID: u64 = 1;

/// Swarm mainnet's bootnodes, pinned as IP multiaddrs with their peer ids: the node resolves
/// no names (ant's `dnsaddr` feature is off), so it makes no DNS query at run time.
///
/// Resolved 2026-11-24 from `/dnsaddr/mainnet.ethswarm.org`, the one bootnode entry bee and
/// ant publish, by walking its `_dnsaddr` TXT records recursively (`dig TXT
/// _dnsaddr.<name>`). The root answers by the asker's location (its nameservers
/// `ns1`/`ns2.mainnet.ethswarm.org` return one of `emea`, `apac`, `amer`), so all three regions
/// were walked: `emea` → `ams`, `hel`, `jhb`; `apac` → `sgp`, `syd`; `amer` → `hil`, `sao`,
/// `tor`. The same answers came from both authoritative servers and from 1.1.1.1, 8.8.8.8 and
/// 9.9.9.9. Kept: every `/ip4/…/tcp/1634/p2p/…` leaf, once (`sao` lists `tor`'s), in that
/// order. Left out: the `/tcp/1635/tls/sni/…/ws` leaves (ant dials TCP only), among them the
/// only address of `5.78.94.214` (`hil`). Each answered on TCP 1634 that day, and a node
/// started from them alone had 24 peers within 11 s. The reader keeps no peer store
/// ([`SwarmConfig::mainnet`]'s `peerstore_path` is `None`), so every start joins through this
/// list; one live entry is enough, as peers then come through hive gossip. Refreshed in a
/// release PR, like Ethereum's bootnodes (`.claude/agent/releases.md`), with
/// `mainnet_through_the_pinned_bootnodes` (a test, `--ignored`) as the check.
pub const MAINNET_BOOTNODES: [&str; 7] = [
    "/ip4/159.223.6.181/tcp/1634/p2p/QmP9b7MxjyEfrJrch5jUThmuFaGzvUPpWEJewCpx5Ln6i8", // ams
    "/ip4/135.181.84.53/tcp/1634/p2p/QmTxX73q8dDiVbmXU7GqMNwG3gWmjSFECuMoCsTW4xp6CK", // hel
    "/ip4/139.84.229.70/tcp/1634/p2p/QmRa6rSrUWJ7s68MNmV94bo2KAa9pYcp6YbFLMHZ3r7n2M", // jhb
    "/ip4/172.104.43.205/tcp/1634/p2p/QmeovveLJmgyfjiA9mJnvFTawHyisuJMCYicJffdWdxNmr", // sgp
    "/ip4/170.64.184.25/tcp/1634/p2p/Qmeh2e7U2FWrSooyrjWjnNKGceJWbRxLLx8Ppy5CimzsGH", // syd
    "/ip4/216.238.102.247/tcp/1634/p2p/QmQYFDafiKuWUDknur8VcTUVgJgNxJevLxzYRKDKKvvv1r", // hil
    "/ip4/172.105.9.172/tcp/1634/p2p/QmQq7zXgZ2Up5NF1tsCP2odgxzU4N3Evx2trkmFYnHm27w", // tor, sao
];

/// Peers a reader keeps by default. A reader needs few; ant's own default (100) suits a node
/// that serves.
pub const READER_PEERS: usize = 24;

/// The text a [`SwarmError::NotReady`] starts with once rendered: how a caller that only
/// sees the message (the addr1 reader's `ChunkSource` carries text) tells it apart.
pub const NOT_READY_TEXT: &str = "the Swarm node has no usable peers yet";

/// A bootnode's address, as [`SwarmConfig::bootnodes`] takes it.
pub use libp2p::Multiaddr;

/// Whether `addr` names a host by DNS (`/dnsaddr/`, `/dns/`, `/dns4/`, `/dns6/`): such an
/// address cannot be a bootnode here, since the node resolves no names.
pub fn needs_dns(addr: &Multiaddr) -> bool {
    use libp2p::multiaddr::Protocol;
    addr.iter().any(|p| {
        matches!(
            p,
            Protocol::Dnsaddr(_) | Protocol::Dns(_) | Protocol::Dns4(_) | Protocol::Dns6(_)
        )
    })
}

/// Errors of the reader.
#[derive(Debug)]
pub enum SwarmError {
    /// The node is not running (its runtime could not start, or the node stopped).
    Stopped,
    /// The node has no usable peers yet (a cold start); worth a retry.
    NotReady(String),
    /// Anything else ant reported, or an answer of an unexpected kind.
    Failed(String),
    /// No answer in time.
    Timeout,
}

impl std::fmt::Display for SwarmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "the Swarm node is not running"),
            Self::NotReady(m) => write!(f, "{NOT_READY_TEXT}: {m}"),
            Self::Failed(m) => write!(f, "Swarm read failed: {m}"),
            Self::Timeout => write!(f, "Swarm read timed out"),
        }
    }
}

impl std::error::Error for SwarmError {}

/// Where and how the node joins Swarm.
#[derive(Clone, Debug)]
pub struct SwarmConfig {
    pub network_id: u64,
    pub bootnodes: Vec<Multiaddr>,
    /// Dial peers' private and loopback addresses: a local dev network only.
    pub allow_private_dials: bool,
    /// File for the warm-restart peer snapshot (`None`: start cold every time).
    pub peerstore_path: Option<PathBuf>,
    /// Peers the node keeps connected.
    pub target_peers: usize,
    /// How long one chunk read may take, queueing included.
    pub chunk_timeout: Duration,
}

impl SwarmConfig {
    /// Swarm mainnet through its pinned bootnodes ([`MAINNET_BOOTNODES`]).
    pub fn mainnet() -> Self {
        Self {
            network_id: MAINNET_NETWORK_ID,
            bootnodes: MAINNET_BOOTNODES
                .iter()
                .map(|a| {
                    a.parse()
                        .expect("MAINNET_BOOTNODES are multiaddrs (a test checks them)")
                })
                .collect(),
            allow_private_dials: false,
            peerstore_path: None,
            target_peers: READER_PEERS,
            chunk_timeout: Duration::from_secs(60),
        }
    }

    /// A local network (bee-factory): its network id and the multiaddrs of its nodes.
    pub fn dev(network_id: u64, bootnodes: Vec<Multiaddr>) -> Self {
        Self {
            network_id,
            bootnodes,
            allow_private_dials: true,
            peerstore_path: None,
            target_peers: READER_PEERS,
            chunk_timeout: Duration::from_secs(30),
        }
    }
}

/// A running embedded node, on its own runtime.
pub struct SwarmNode {
    commands: mpsc::Sender<ControlCommand>,
    status: watch::Receiver<StatusSnapshot>,
    chunk_timeout: Duration,
    runtime: std::sync::Mutex<Option<tokio::runtime::Runtime>>,
}

impl SwarmNode {
    /// Starts a node on a runtime of its own (two worker threads). Callable from any thread,
    /// with or without a runtime. The node is outbound only: it listens nowhere, maps no port
    /// over UPnP and announces no address of its own. A bootnode named by DNS ([`needs_dns`])
    /// is refused: this build resolves no names. Its identity (the BZZ signing key, the
    /// overlay nonce and the libp2p key, unrelated to each other) is fresh for every start: a
    /// reader needs no stable identity, and a fresh one links fewer sessions together.
    pub fn start(cfg: SwarmConfig) -> Result<Self, SwarmError> {
        if let Some(addr) = cfg.bootnodes.iter().find(|a| needs_dns(a)) {
            return Err(SwarmError::Failed(format!(
                "bootnode {addr} needs DNS, which this Swarm reader does not do: give /ip4/ or /ip6/ multiaddrs"
            )));
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("myotis-swarm")
            .enable_all()
            .build()
            .map_err(|e| SwarmError::Failed(format!("starting the Swarm runtime: {e}")))?;
        let (commands, rx) = mpsc::channel(64);
        let (status_tx, status) = watch::channel(StatusSnapshot::default());
        // A reader settles nothing on chain and pays nobody: free-tier retrieval only.
        let node = NodeConfig::mainnet_default(
            ant_crypto::random_secp256k1_secret(),
            ant_crypto::random_overlay_nonce(),
            cfg.bootnodes,
            Keypair::generate_secp256k1(),
        )
        .with_network_id(cfg.network_id)
        .with_allow_private_dials(cfg.allow_private_dials)
        // A reader: no listener, no UPnP mapping, no address of ours announced (ant#269).
        .with_outbound_only(true)
        .with_status(status_tx)
        .with_commands(rx)
        .with_peerstore_path(cfg.peerstore_path)
        .with_target_peers(Some(cfg.target_peers))
        .with_process_start(Instant::now())
        .with_swap_enabled(false);
        runtime.spawn(async move {
            if let Err(e) = run_node(node).await {
                tracing::warn!(target: "myotis_swarm", "Swarm node stopped: {e}");
            }
        });
        Ok(Self {
            commands,
            status,
            chunk_timeout: cfg.chunk_timeout,
            runtime: std::sync::Mutex::new(Some(runtime)),
        })
    }

    /// Peers with a completed Swarm handshake.
    pub fn connected_peers(&self) -> u32 {
        self.status.borrow().peers.connected
    }

    /// Whether the node is still running (its command channel is open).
    pub fn is_running(&self) -> bool {
        !self.commands.is_closed()
    }

    /// The chunk at `address` in wire form (`span ‖ payload` for a content-addressed chunk,
    /// `identifier ‖ signature ‖ span ‖ payload` for a SOC), or `Ok(None)` when it is
    /// unavailable from the peers asked: at most [`SwarmConfig::target_peers`], fewer with
    /// `expect_missing`, and any of them can withhold it. So `None` is "not delivered", never
    /// proof that the chunk does not exist. `expect_missing` asks fewer peers: for a read
    /// whose miss is an expected answer (a head update not published yet).
    ///
    /// Callable from any tokio runtime (the timeout uses the caller's).
    pub async fn chunk(
        &self,
        address: [u8; 32],
        expect_missing: bool,
    ) -> Result<Option<Vec<u8>>, SwarmError> {
        let read = async {
            let (ack, rx) = oneshot::channel();
            self.commands
                .send(ControlCommand::GetChunkRaw {
                    reference: address,
                    fast_miss: expect_missing,
                    bypass_cache: false,
                    ack,
                })
                .await
                .map_err(|_| SwarmError::Stopped)?;
            match rx.await {
                Err(_) => Err(SwarmError::Stopped),
                Ok(ControlAck::Bytes { data }) => Ok(Some(data)),
                Ok(other) => match classify(other) {
                    Miss::Unavailable => Ok(None),
                    Miss::Error(e) => Err(e),
                },
            }
        };
        tokio::time::timeout(self.chunk_timeout, read)
            .await
            .unwrap_or(Err(SwarmError::Timeout))
    }

    /// [`Self::chunk`] with the error the addr1 reader's
    /// [`ChunkSource`](myotis_addr1::client::ChunkSource) takes: a stopped node is
    /// `Stopped`, every other failure (no usable peers yet, a timeout, a failed fetch)
    /// `Transient`, for this read only.
    async fn chunk_typed(
        &self,
        address: [u8; 32],
        expect_missing: bool,
    ) -> Result<Option<Vec<u8>>, myotis_addr1::client::SourceError> {
        use myotis_addr1::client::SourceError;
        self.chunk(address, expect_missing).await.map_err(|e| match e {
            SwarmError::Stopped => SourceError::Stopped(e.to_string()),
            other => SourceError::Transient(other.to_string()),
        })
    }

    /// Stops the node: shuts its runtime down, which ends every task ant spawned (its swarm
    /// loop, listener and connections), waiting up to `timeout` for them. Blocking: call it
    /// from a thread that may block (not from inside an async task).
    pub fn shutdown(self, timeout: Duration) {
        let rt = self.runtime.lock().ok().and_then(|mut r| r.take());
        if let Some(rt) = rt {
            rt.shutdown_timeout(timeout);
        }
    }

    /// Stops the node now, from a shared reference, without waiting: every read in flight
    /// ends with [`SwarmError::Stopped`] and the node's tasks, listener and connections end
    /// with its runtime. Safe inside an async context. For a switch that must take effect at
    /// once (Tor routing turned on), however many readers still hold the node.
    pub fn stop(&self) {
        let rt = self.runtime.lock().ok().and_then(|mut r| r.take());
        if let Some(rt) = rt {
            rt.shutdown_background();
        }
    }
}

/// The addr1 reader reads through the node: every chunk it gets is checked there before use.
impl myotis_addr1::client::ChunkSource for SwarmNode {
    fn chunk(
        &self,
        address: [u8; 32],
        expect_missing: bool,
    ) -> impl std::future::Future<Output = Result<Option<Vec<u8>>, myotis_addr1::client::SourceError>> + Send {
        self.chunk_typed(address, expect_missing)
    }
}

impl Drop for SwarmNode {
    /// Dropping the node stops it too, without waiting (safe inside an async context).
    fn drop(&mut self) {
        self.stop();
    }
}

/// How a read that delivered nothing ended.
enum Miss {
    /// The chunk was not delivered (peers answered "not found", or ant said so in words).
    Unavailable,
    Error(SwarmError),
}

/// What an answer other than the chunk means. ant reports a missing chunk as `NotFound` on
/// some paths and inside a plain error message on others ("storage: not found"), and a node
/// without peers as `NotReady` or as "no BZZ peers available".
fn classify(ack: ControlAck) -> Miss {
    match ack {
        ControlAck::NotFound { .. } => Miss::Unavailable,
        ControlAck::NotReady { message } => Miss::Error(SwarmError::NotReady(message)),
        ControlAck::Error { message } => {
            let m = message.to_ascii_lowercase();
            if m.contains("no bzz peers") || m.contains("no peers available") {
                Miss::Error(SwarmError::NotReady(message))
            } else if m.contains("not found") {
                Miss::Unavailable
            } else {
                Miss::Error(SwarmError::Failed(message))
            }
        }
        other => Miss::Error(SwarmError::Failed(format!("unexpected answer {other:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn misses_and_errors_are_classified() {
        let err = |m: &str| {
            classify(ControlAck::Error {
                message: m.to_string(),
            })
        };
        assert!(matches!(
            err("retrieval: no BZZ peers available"),
            Miss::Error(SwarmError::NotReady(_))
        ));
        assert!(matches!(
            err("no peers available"),
            Miss::Error(SwarmError::NotReady(_))
        ));
        assert!(matches!(
            err("chunk abc: storage: not found"),
            Miss::Unavailable
        ));
        assert!(matches!(
            err("bad span"),
            Miss::Error(SwarmError::Failed(_))
        ));
        assert!(matches!(
            classify(ControlAck::NotFound {
                message: String::new()
            }),
            Miss::Unavailable
        ));
    }

    /// The pinned bootnodes are what ant can dial without DNS: TCP over IPv4, each with its
    /// peer id, no name anywhere, no duplicate.
    #[test]
    fn mainnet_bootnodes_are_ip_multiaddrs_with_peer_ids() {
        use libp2p::multiaddr::Protocol;
        let cfg = SwarmConfig::mainnet();
        assert_eq!(cfg.bootnodes.len(), MAINNET_BOOTNODES.len());
        let mut peers = std::collections::HashSet::new();
        for addr in &cfg.bootnodes {
            let parts: Vec<Protocol> = addr.iter().collect();
            assert!(
                matches!(
                    parts.as_slice(),
                    [Protocol::Ip4(ip), Protocol::Tcp(1634), Protocol::P2p(peer)]
                        if !ip.is_private() && !ip.is_loopback() && peers.insert(*peer)
                ),
                "{addr}"
            );
        }
    }

    /// A node with nowhere to connect starts from a thread without a runtime, says it has no
    /// peers instead of hanging, and is gone after `shutdown`: its status sender (owned by
    /// ant's node loop) is dropped. It opened no listener and announced no address (ant's
    /// `outbound_only`), which is also why this test can run by default.
    #[test]
    fn a_node_without_peers_starts_answers_and_stops() {
        let mut cfg = SwarmConfig::dev(4020, Vec::new());
        cfg.chunk_timeout = Duration::from_secs(10);
        let node = SwarmNode::start(cfg).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut status = node.status.clone();
        rt.block_on(async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert!(node.is_running(), "the node did not come up");
            assert_eq!(node.connected_peers(), 0);
            let st = node.status.borrow().clone();
            assert!(st.listeners.is_empty(), "listening on {:?}", st.listeners);
            assert!(
                st.external_addresses.is_empty(),
                "announcing {:?}",
                st.external_addresses
            );
            let got = node.chunk([7u8; 32], false).await;
            assert!(
                matches!(got, Err(SwarmError::NotReady(_)) | Ok(None)),
                "a peerless node answered {got:?}"
            );
        });
        node.shutdown(Duration::from_secs(5));
        rt.block_on(async {
            let gone = tokio::time::timeout(Duration::from_secs(5), async {
                while status.changed().await.is_ok() {}
            })
            .await;
            assert!(gone.is_ok(), "the node's status sender outlived shutdown");
        });
    }

    /// A DNS bootnode is refused before anything starts, and told from an IP one.
    #[test]
    fn a_dns_bootnode_is_refused() {
        for dns in [
            "/dnsaddr/mainnet.ethswarm.org",
            "/dns4/example.org/tcp/1634",
        ] {
            let addr: Multiaddr = dns.parse().unwrap();
            assert!(needs_dns(&addr));
            let got = SwarmNode::start(SwarmConfig::dev(4020, vec![addr]));
            assert!(
                matches!(&got, Err(SwarmError::Failed(m)) if m.contains("needs DNS")),
                "{dns}"
            );
        }
        assert!(!needs_dns(&MAINNET_BOOTNODES[0].parse().unwrap()));
    }

    /// Swarm mainnet through the pinned bootnodes alone (not run by default: it needs the
    /// network). The node gets peers, a read is answered (a random address: not delivered),
    /// and it never listens or announces an address of its own. Run it when the pins move:
    /// `cargo test --manifest-path rust/swarm/Cargo.toml -- --ignored --nocapture mainnet`.
    #[test]
    #[ignore]
    fn mainnet_through_the_pinned_bootnodes() {
        let node = SwarmNode::start(SwarmConfig::mainnet()).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let t = Instant::now();
            let mut first = None;
            while node.connected_peers() < 8 && t.elapsed() < Duration::from_secs(90) {
                if first.is_none() && node.connected_peers() > 0 {
                    first = Some(t.elapsed());
                }
                let st = node.status.borrow().clone();
                assert!(st.listeners.is_empty(), "listening on {:?}", st.listeners);
                assert!(
                    st.external_addresses.is_empty(),
                    "announcing {:?}",
                    st.external_addresses
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            println!(
                "first peer after {first:?}; {} peers after {:?}",
                node.connected_peers(),
                t.elapsed()
            );
            assert!(
                node.connected_peers() > 0,
                "no peer through the pinned bootnodes"
            );
            let t = Instant::now();
            let got = node.chunk([0x5a; 32], true).await;
            println!("random chunk: {got:?} after {:?}", t.elapsed());
            // Not delivered, as ant reports it: "not found", or every asked peer failed.
            assert!(
                matches!(&got, Ok(None))
                    || matches!(&got, Err(SwarmError::Failed(m)) if m.contains("all peers failed")),
                "{got:?}"
            );
        });
        node.shutdown(Duration::from_secs(5));
    }

    /// Against a running bee-factory (not run by default):
    /// `SWARM_DEV_BOOTNODE=<multiaddr of a node> SWARM_DEV_NETWORK=<network id>
    /// SWARM_DEV_CHUNK=<hex address> cargo test --manifest-path rust/swarm/Cargo.toml --
    /// --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_a_chunk_from_a_dev_network() {
        let bootnode: Multiaddr = std::env::var("SWARM_DEV_BOOTNODE")
            .unwrap()
            .parse()
            .unwrap();
        let network: u64 = std::env::var("SWARM_DEV_NETWORK").unwrap().parse().unwrap();
        let address: [u8; 32] = hex_32(&std::env::var("SWARM_DEV_CHUNK").unwrap());
        let node = SwarmNode::start(SwarmConfig::dev(network, vec![bootnode])).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            for _ in 0..60 {
                if node.connected_peers() > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            println!("{} peers", node.connected_peers());
            let chunk = node
                .chunk(address, false)
                .await
                .unwrap()
                .expect("chunk exists");
            println!("chunk of {} bytes", chunk.len());
        });
        node.shutdown(Duration::from_secs(5));
    }

    fn hex_32(s: &str) -> [u8; 32] {
        let s = s.trim_start_matches("0x");
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        out
    }
}
