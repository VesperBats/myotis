//! Do this network's pinned CL servers serve a light-client bootstrap for one
//! candidate checkpoint? `./gradlew refreshCheckpoint` asks this before it
//! writes a root, so an anchor no pinned server can bootstrap never lands in
//! the tree.
//!
//! ```bash
//! cargo run --release -p myotis-net --example checkpoint_served -- \
//!     --net sepolia --slot 11632064 \
//!     --root 751022ba9937712663e0df63c0f20879c91945c269239cab350bd7b209760ebb
//! ```
//!
//! Why it exists: the v0.1.22 release refreshed Sepolia to slot 11640768, a
//! canonical finalized root that every checkpoint-sync server agreed on. Yet
//! both pinned servers answered ResourceUnavailable / "LC bootstrap
//! unavailable" for it (cold-start run 45142481008) while serving its
//! neighbours. The refresh only asks the checkpoint-sync servers, which answer
//! for the chain, not for what a fresh install's servers can bootstrap.
//!
//! Each pin is asked the way `tests/live_pins_alive.rs` asks it: a cache miss
//! is retried, because roost answers ResourceUnavailable for an uncached root
//! and fills it in the background; and an answer counts only if it passes the
//! production acceptance: the served header hashes to the candidate root and
//! `verify_bootstrap` accepts both Merkle branches. The catch-up (updates) is
//! not asked: it is a property of the period, not of the root, and the
//! release's cold-start check covers it.
//!
//! The floor counts the pins that ANSWERED, not all of them, so it is weaker
//! than the release check (`live_pins_alive` needs two pins serving, reachable
//! or not): this check is about the ROOT, and only the release check is about
//! whether enough pins are up. A pin that cannot be dialled (after one more try
//! 30 s later) says nothing about the root: Sepolia's Nimbus refuses quick
//! reconnects from one address, so a run right after another sees it as
//! "dial failed" while it serves the same root to everyone else. A pin that
//! answers and does not serve the root is the finding this check exists for.
//!
//! Exit status: 0 when at least one pin served it and at least `min(2, pins
//! that answered)` did; 1 when pins answered and fewer served it (the root is
//! not servable); 2 on a usage error; 3 when the network pins no CL server
//! (nothing to check — chiado today); 4 when no pin answered at all (the check
//! could not run, which says nothing about the root); 5 when this process could
//! not start its libp2p host.

use std::sync::Arc;
use std::time::Duration;

use libp2p::{Multiaddr, PeerId};
use myotis_consensus::store::{LightClientProcessor, LightClientStore};
use myotis_consensus::types::LightClientBootstrap;
use myotis_net::codec;
use myotis_net::protocols;
use myotis_net::reqresp::{self, LocalStatus, RequestError};
use myotis_net::status::StatusMessage;
use myotis_net::ChainConfig;

/// Per-request budget, as in `live_pins_alive`.
const PIN_TIMEOUT: Duration = Duration::from_secs(20);
/// Miss retries, as in `live_pins_alive`: one roost refresh tick plus headroom.
const MISS_RETRIES: usize = 2;
const MISS_BACKOFF: Duration = Duration::from_secs(15);
/// The same floor as the release check: two is the smallest number that is
/// not a single point of failure. Applied to the pins that answered.
const MIN_SERVING_PINS: usize = 2;
/// How long a pin that refused the dial waits before its one more try.
const UNREACHABLE_RETRY: Duration = Duration::from_secs(30);

/// Why a pin did not serve the root: it answered without it, or it could not
/// be reached at all (which says nothing about the root).
enum Miss {
    Answered(String),
    Unreachable(String),
}

fn usage(msg: &str) -> ! {
    eprintln!("checkpoint_served: {msg}");
    eprintln!("usage: checkpoint_served --net <mainnet|sepolia|gnosis|chiado> --slot <n> --root <64 hex>");
    std::process::exit(2);
}

fn parse_root(s: &str) -> [u8; 32] {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        usage(&format!("--root must be 64 hex digits, got {s:?}"));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("checked hex");
    }
    out
}

fn split_pin(pin: &str) -> Option<(Multiaddr, PeerId)> {
    let full = pin.parse::<Multiaddr>().ok()?;
    let mut addr = Multiaddr::empty();
    let mut peer = None;
    for proto in full.iter() {
        if let libp2p::multiaddr::Protocol::P2p(id) = proto {
            peer = Some(id);
        } else {
            addr.push(proto);
        }
    }
    Some((addr, peer?))
}

/// Ask one pin, retrying a cache miss. `Ok(())` when it served a bootstrap that
/// passes the production acceptance for `root`; otherwise why not.
async fn ask(
    client: &reqresp::ReqRespClient,
    config: &ChainConfig,
    peer: PeerId,
    addr: &Multiaddr,
    root: [u8; 32],
) -> Result<(), Miss> {
    // The last REAL answer this pin gave. A retry that then fails at the
    // transport level (Nimbus refuses quick reconnects) must not turn an
    // earlier "not available" into "unreachable".
    let mut last_answer: Option<Vec<u8>> = None;
    for attempt in 0..=MISS_RETRIES {
        if attempt > 0 {
            eprintln!("[served] .... {addr} retrying after a miss ({attempt}/{MISS_RETRIES})");
            tokio::time::sleep(MISS_BACKOFF).await;
        }
        let req = codec::encode_request(&root);
        match tokio::time::timeout(
            PIN_TIMEOUT,
            client.request_raw(peer, addr.clone(), protocols::BOOTSTRAP, req),
        )
        .await
        {
            Ok(Ok(raw)) => {
                // Only ResourceUnavailable (roost's cache miss) is worth
                // retrying; decided on the result code, never on the length.
                let miss = raw.first() == Some(&codec::RESULT_RESOURCE_UNAVAILABLE);
                last_answer = Some(raw);
                if !miss {
                    break;
                }
            }
            // Only a dial that never reached the peer says nothing about the
            // root (Shutdown is this process's own side). Everything else
            // reached a connection: a timeout, a closed stream or a refused
            // protocol is the pin's answer, as `request_raw_at` documents.
            Ok(Err(RequestError::DialFailure | RequestError::Shutdown)) => {
                if last_answer.is_some() {
                    break;
                }
                return Err(Miss::Unreachable("dial failed".to_string()));
            }
            Ok(Err(e)) => {
                if last_answer.is_some() {
                    break;
                }
                return Err(Miss::Answered(e.to_string()));
            }
            // Our own budget ran out: count it against the root. A trust
            // anchor gate errs towards refusing; the walk then moves on.
            Err(_) => {
                if last_answer.is_some() {
                    break;
                }
                return Err(Miss::Answered(format!("no answer within {}s", PIN_TIMEOUT.as_secs())));
            }
        }
    }
    let raw = last_answer.expect("every path that reaches here stored an answer");
    let answered = |why: String| Miss::Answered(why);
    if let Some((code, msg)) = codec::leading_error(&raw) {
        return Err(answered(format!("result code {code} {msg:?}")));
    }
    let d = codec::decode_response(&raw, true).map_err(|e| answered(format!("undecodable answer: {e}")))?;
    if d.ssz_payload.is_empty() {
        return Err(answered("empty payload".to_string()));
    }
    let fork = config.lc_fork_of_chunk(&d.fork_digest, d.ssz_payload.len(), LightClientBootstrap::GLOAS_SIZE);
    let b = LightClientBootstrap::decode_for(fork, &d.ssz_payload)
        .map_err(|e| answered(format!("bootstrap did not decode: {e}")))?;
    if b.header.beacon.hash_tree_root() != root {
        return Err(answered("served a bootstrap for another root".to_string()));
    }
    let processor = LightClientProcessor::new(
        LightClientStore::new(config.slots_per_period()),
        config.fork_schedule.clone(),
        config.genesis_validators_root,
    );
    processor.verify_bootstrap(&b).map_err(|reason| answered(reason.to_string()))
}

#[tokio::main]
async fn main() {
    let mut net = None;
    let mut slot = None;
    let mut root = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--net" => net = args.next(),
            "--slot" => slot = args.next(),
            "--root" => root = args.next(),
            other => usage(&format!("unknown argument {other:?}")),
        }
    }
    for var in ["MYOTIS_CL_STATIC_PEERS", "MYOTIS_CL_DISABLE_DISCV5"] {
        if std::env::var_os(var).is_some() {
            usage(&format!("{var} is set: it replaces the shipped pin list, so the answer would not be about the pins this build ships"));
        }
    }
    let mut config = match net.as_deref() {
        Some("mainnet") => ChainConfig::mainnet(),
        Some("sepolia") => ChainConfig::sepolia(),
        Some("gnosis") => ChainConfig::gnosis(),
        Some("chiado") => ChainConfig::chiado(),
        Some(other) => usage(&format!("unknown --net {other:?}")),
        None => usage("--net is required"),
    };
    let slot: u64 = match slot.as_deref().map(str::parse) {
        Some(Ok(s)) => s,
        Some(Err(_)) => usage("--slot must be a number"),
        None => usage("--slot is required"),
    };
    let root = parse_root(root.as_deref().unwrap_or_else(|| usage("--root is required")));
    let net = net.expect("matched above");

    if config.static_peers.is_empty() {
        eprintln!("[served] {net} pins no CL server: nothing to check");
        std::process::exit(3);
    }
    // The local status names the candidate, as a fresh install anchored on it
    // would.
    config.checkpoint_root = root;
    config.checkpoint_slot = slot;
    let local = LocalStatus::new(StatusMessage {
        fork_digest: config.current_fork_digest(),
        finalized_root: root,
        finalized_epoch: slot / config.slots_per_epoch.max(1),
        head_root: root,
        head_slot: slot,
        earliest_available_slot: 0,
    });
    let client = match reqresp::start_host(Arc::clone(&local)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[served] could not start a libp2p host: {e}");
            std::process::exit(5);
        }
    };

    let pins = config.static_peers.clone();
    let mut serving = 0usize;
    let mut answered = 0usize;
    let mut unreachable = Vec::new();
    for round in 0..2 {
        let todo: Vec<String> = if round == 0 { pins.clone() } else { std::mem::take(&mut unreachable) };
        if todo.is_empty() {
            break;
        }
        if round > 0 {
            // A pin that refused the dial gets one more try, later: Sepolia's
            // Nimbus refuses a quick reconnect from the same address.
            eprintln!("[served] .... {} pin(s) unreachable; one more try in {}s", todo.len(), UNREACHABLE_RETRY.as_secs());
            tokio::time::sleep(UNREACHABLE_RETRY).await;
        }
        for pin in &todo {
            let Some((addr, peer)) = split_pin(pin) else {
                eprintln!("[served] BAD  {pin} — unparseable, or no /p2p/ peer id");
                continue;
            };
            match ask(&client, &config, peer, &addr, root).await {
                Ok(()) => {
                    serving += 1;
                    answered += 1;
                    eprintln!("[served] OK   {addr}");
                }
                Err(Miss::Answered(why)) => {
                    answered += 1;
                    eprintln!("[served] NO   {addr} — {why}");
                }
                Err(Miss::Unreachable(why)) => {
                    eprintln!("[served] DOWN {addr} — {why} (says nothing about the root)");
                    unreachable.push(pin.clone());
                }
            }
        }
    }
    let hex: String = root[..4].iter().map(|b| format!("{b:02x}")).collect();
    if answered == 0 {
        eprintln!("[served] no pinned {net} server answered: the check could not run");
        std::process::exit(4);
    }
    let floor = MIN_SERVING_PINS.min(answered);
    eprintln!(
        "[served] {serving} of {answered} answering pinned {net} servers ({} pinned) served slot {slot} (0x{hex}…); need {floor}",
        pins.len()
    );
    std::process::exit(if serving >= floor { 0 } else { 1 });
}
