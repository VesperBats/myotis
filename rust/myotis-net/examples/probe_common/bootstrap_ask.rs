//! Ask ONE pinned CL server for the light-client bootstrap of one root, the
//! way a fresh install does, and apply the production acceptance. Shared by
//! `tests/live_pins_alive.rs` (the release check) and
//! `examples/checkpoint_served.rs` (`./gradlew refreshCheckpoint`'s check), so
//! the two cannot drift apart: a refresh that passes a root the release check
//! then refuses is the incident the refresh check exists for (v0.1.22's
//! Sepolia anchor). Each includes it with
//! `#[path = "…/probe_common/bootstrap_ask.rs"] mod bootstrap_ask;` and keeps
//! its own reporting on top.

// Each includer uses its own subset of the result.
#![allow(dead_code)]

use std::time::Duration;

use libp2p::{Multiaddr, PeerId};
use myotis_consensus::store::{LightClientProcessor, LightClientStore};
use myotis_consensus::types::LightClientBootstrap;
use myotis_net::codec;
use myotis_net::protocols;
use myotis_net::reqresp::{self, RequestError};
use myotis_net::ChainConfig;

/// Per-request budget. Generous: a healthy server answers in well under a
/// second, and a slow-but-alive one must not be reported as dead.
pub const PIN_TIMEOUT: Duration = Duration::from_secs(20);

/// A first bootstrap request may legitimately MISS. roost's handler is a pure
/// cache read — the design forbids I/O on the swarm task — so an uncached root
/// answers `ResourceUnavailable` and queues a background fetch, expecting the
/// wallet to retry (`rust/roost/src/store.rs`, "a miss stays
/// ResourceUnavailable, with a background task filling it"). roost drops its
/// cached bootstraps whenever the fork/blob schedule changes, and starts empty
/// after a restart, so a single-shot check reports the project's own primary
/// server as dead on a cold cache. Retry the way a wallet does.
/// One full roost REFRESH tick (12 s, `roost/src/serve.rs`) plus fetch
/// headroom: `fill_bootstrap_misses` runs once per tick, so a shorter backoff
/// retries BEFORE the fill it is waiting for and reports a cold-but-healthy
/// server as dead — the false reading this retry exists to remove.
pub const MISS_RETRIES: usize = 2;

/// Budget for the plain TCP connect that tells "unreachable" from "silent".
/// libp2p's TCP dial has no timeout of its own here, so a dial into a black
/// hole waits out the OS's SYN retries (about two minutes on Linux) and would
/// hit [`PIN_TIMEOUT`] first, reading as an answer from a pin that never got
/// the request.
pub const TCP_REACH_TIMEOUT: Duration = Duration::from_secs(10);

/// Can a TCP connection to `addr` be opened at all? `None` when `addr` is not
/// an `/ip4|ip6/.../tcp/<port>` address (a QUIC pin): nothing to pre-check.
async fn tcp_reachable(addr: &Multiaddr) -> Option<Result<(), String>> {
    use libp2p::multiaddr::Protocol;
    let mut ip = None;
    let mut port = None;
    for p in addr.iter() {
        match p {
            Protocol::Ip4(a) => ip = Some(std::net::IpAddr::V4(a)),
            Protocol::Ip6(a) => ip = Some(std::net::IpAddr::V6(a)),
            Protocol::Tcp(p) => port = Some(p),
            _ => {}
        }
    }
    let sock = std::net::SocketAddr::new(ip?, port?);
    Some(
        match tokio::time::timeout(TCP_REACH_TIMEOUT, tokio::net::TcpStream::connect(sock)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(format!("tcp connect failed: {e}")),
            Err(_) => Err(format!("tcp connect timed out after {TCP_REACH_TIMEOUT:?}")),
        },
    )
}
pub const MISS_BACKOFF: Duration = Duration::from_secs(15);

/// A bootstrap that passed the production acceptance, with the processor that
/// accepted it (the release check goes on to run the catch-up through it).
pub struct Served {
    pub bootstrap: LightClientBootstrap,
    pub processor: LightClientProcessor,
    pub payload_len: usize,
}

/// Why the pin did not serve it.
pub enum NotServed {
    /// The dial never reached the pin. Says nothing about the root.
    Unreachable(String),
    /// It connected, then gave no answer: a timeout, a closed stream, a
    /// refused protocol. That is the pin's answer, as `request_raw_at`
    /// documents.
    NoAnswer(String),
    /// It answered without a bootstrap: an error code (a miss the retries did
    /// not outlast among them) or an empty payload.
    Unavailable(String),
    /// It answered with something the production acceptance refuses.
    Invalid(String),
}

/// Ask `peer` for the bootstrap of `root`, retrying a cache miss, and accept
/// it the way the production bootstrap path does (`try_bootstrap` in
/// `sync.rs`): the served header hashes to `root` — we chose the root, the
/// peer chose the payload — and `verify_bootstrap` accepts both Merkle
/// branches at the header's fork. `log` prefixes the retry lines.
pub async fn ask_bootstrap(
    client: &reqresp::ReqRespClient,
    config: &ChainConfig,
    peer: PeerId,
    addr: &Multiaddr,
    root: [u8; 32],
    log: &str,
) -> Result<Served, NotServed> {
    // The last REAL answer. A retry that then fails at the transport level
    // (Sepolia's Nimbus refuses quick reconnects) must not turn an earlier
    // "not available" into "unreachable".
    let mut last_answer: Option<Vec<u8>> = None;
    if let Some(Err(why)) = tcp_reachable(addr).await {
        return Err(NotServed::Unreachable(why));
    }
    for attempt in 0..=MISS_RETRIES {
        if attempt > 0 {
            eprintln!("{log} .... {addr} retrying after a miss ({attempt}/{MISS_RETRIES})");
            tokio::time::sleep(MISS_BACKOFF).await;
        }
        let req = codec::encode_request(&root);
        let failed = match tokio::time::timeout(
            PIN_TIMEOUT,
            client.request_raw(peer, addr.clone(), protocols::BOOTSTRAP, req),
        )
        .await
        {
            Ok(Ok(raw)) => {
                // Only a cache MISS is worth retrying. Decide that on the eth2
                // RESULT CODE — byte 0 of the response frame — never on the
                // response's length: an error chunk is `code || varint ||
                // snappy(msg)`, so a long message outweighs any threshold.
                // InvalidRequest and ServerError are final answers; only
                // ResourceUnavailable is the miss roost fills in the background.
                let miss = raw.first() == Some(&codec::RESULT_RESOURCE_UNAVAILABLE);
                last_answer = Some(raw);
                if miss {
                    continue;
                }
                break;
            }
            // Shutdown is this process's own side, like a dial that never left.
            Ok(Err(RequestError::DialFailure | RequestError::Shutdown)) => {
                NotServed::Unreachable("dial failed".to_string())
            }
            Ok(Err(e)) => NotServed::NoAnswer(e.to_string()),
            Err(_) => NotServed::NoAnswer(format!("no answer in {PIN_TIMEOUT:?}")),
        };
        if last_answer.is_some() {
            break;
        }
        return Err(failed);
    }
    let raw = last_answer.expect("every path that leaves the loop stored an answer");

    let d = match codec::decode_response(&raw, true) {
        Ok(d) if !d.ssz_payload.is_empty() => d,
        Ok(_) => {
            return Err(NotServed::Unavailable(
                "success code with an empty payload".to_string(),
            ))
        }
        Err(e) => {
            // An error answer's message is snappy-framed, which
            // `decode_response` prints raw; `leading_error` reads it.
            let why = match codec::leading_error(&raw) {
                Some((code, msg)) => format!("result code {code} {msg:?}"),
                None => e.to_string(),
            };
            return Err(NotServed::Unavailable(why));
        }
    };
    // Deliberately NOT compared against current_fork_digest(): a bootstrap's
    // context bytes follow the slot of the block it anchors to, not the wall
    // clock, and a still-valid checkpoint can sit behind a fork boundary —
    // roost stamps them that way on purpose (`roost/src/serve.rs`). The
    // production bootstrap path does not check it either; the root pin and the
    // two branches below are the real proof.
    let processor = LightClientProcessor::new(
        LightClientStore::new(config.slots_per_period()),
        config.fork_schedule.clone(),
        config.genesis_validators_root,
    );
    let fork = config.lc_fork_of_chunk(
        &d.fork_digest,
        d.ssz_payload.len(),
        LightClientBootstrap::GLOAS_SIZE,
    );
    let bootstrap = LightClientBootstrap::decode_for(fork, &d.ssz_payload)
        .map_err(|e| NotServed::Invalid(format!("bootstrap did not decode: {e}")))?;
    if bootstrap.header.beacon.hash_tree_root() != root {
        let got: String = bootstrap.header.beacon.hash_tree_root()[..8]
            .iter()
            .map(|x| format!("{x:02x}"))
            .collect();
        return Err(NotServed::Invalid(format!(
            "served a bootstrap for root {got}, not the one asked for"
        )));
    }
    processor
        .verify_bootstrap(&bootstrap)
        .map_err(|reason| NotServed::Invalid(reason.to_string()))?;
    Ok(Served {
        bootstrap,
        processor,
        payload_len: d.ssz_payload.len(),
    })
}
