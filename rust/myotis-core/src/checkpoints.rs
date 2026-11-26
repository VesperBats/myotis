//! The bundled checkpoint table: the hash of every 1024th block, a trust anchor (D1).
//!
//! The rule is `.claude/agent/data-sources.md` (Trust, "The bundled checkpoint table"); the
//! design is `docs/ideas/deep-block-verification.md`. Entry `i` is the hash of block
//! `i * 1024`, from genesis up; the table also carries its **top**, the finalized block the
//! build walked down from, which is generally not a multiple of 1024. A block `n` at or below
//! the top is canonical when the headers from `n` up to [`CheckpointTable::anchor_at_or_above`]
//! hash-link to that anchor: at most [`CheckpointTable::K`] headers, one `GetBlockHeaders`.
//!
//! The table is committed as an asset (`rust/myotis-core/checkpoints/`), never produced by a
//! build, and pinned here by its keccak256: [`CheckpointTable::parse`] refuses bytes that do
//! not match, and [`mainnet`] / [`gnosis`] parse the embedded copy against its pin. A running
//! node never writes into it. Tables are bundled for mainnet (chain 1) and Gnosis Chain (chain
//! 100, owner's decision 2026-11-25).
//!
//! Why the pin covers every entry and the top: the build linked every header by `parentHash`
//! from the top down to the genesis hash, so all entries lie on one hash chain between those
//! two ends. An entry off that chain would break the walk. What the top rests on differs per
//! table: mainnet's was cross-checked against a second client; Gnosis Chain's is a finalized
//! block the light client attested, with no second execution client (its pin says which, and
//! the rule's text for it awaits the owner's sign-off). The running node's gap walk checks
//! either once more (the rule). The pin then proves these are the bytes of that build.
//!
//! Sans-I/O like the rest of this crate: [`mainnet`] and [`gnosis`] read `include_bytes!` data
//! only. [`CheckpointTable::parse`] hashes the whole table (0.83 MB on mainnet, 1.55 MB on
//! Gnosis Chain), so a caller parses once and keeps the table; it is `Copy` and borrows the
//! static bytes.

use crate::keccak::keccak256;
use crate::{err, CoreError};
use alloc::format;

/// One table: its entries (32-byte hashes, entry `i` = block `i * K`) and its top.
/// Built only by [`mainnet`] / [`gnosis`] / [`for_chain`] outside this crate, so every table is the
/// release's, pinned in the source.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CheckpointTable<'a> {
    entries: &'a [u8],
    top_number: u64,
    top_hash: [u8; 32],
}

/// What a table's build pins in the source: everything [`CheckpointTable::parse`] checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TablePin {
    /// keccak256 of the whole entries file.
    pub keccak256: [u8; 32],
    /// The number of entries (the file is `entries * 32` bytes).
    pub entries: u64,
    /// The network's genesis hash, which entry 0 must be.
    pub genesis_hash: [u8; 32],
    /// The finalized block the build walked down from.
    pub top_number: u64,
    /// Its hash.
    pub top_hash: [u8; 32],
}

/// Not the derived one: that would print all 0.83 MB of entries.
impl core::fmt::Debug for CheckpointTable<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CheckpointTable")
            .field("entries", &self.len())
            .field("top_number", &self.top_number)
            .finish_non_exhaustive()
    }
}

impl<'a> CheckpointTable<'a> {
    /// The table's stride: one hash every 1024 blocks, geth's `GetBlockHeaders` limit, so
    /// every lookup is one request.
    pub const K: u64 = 1024;

    /// Parse `bytes` against `pin`: the keccak256, the length, entry 0 against the genesis
    /// hash, and the top at or above the last entry and below the next one (so the top is
    /// the only anchor above the last entry, and no anchor is more than `K` blocks above
    /// any block it serves).
    pub(crate) fn parse(bytes: &'a [u8], pin: &TablePin) -> Result<Self, CoreError> {
        if keccak256(bytes) != pin.keccak256 {
            return err("checkpoint table does not match its pinned keccak256");
        }
        if bytes.len() as u64 != pin.entries.saturating_mul(32) || pin.entries == 0 {
            return err(format!(
                "checkpoint table is {} bytes, the pin says {} entries",
                bytes.len(),
                pin.entries
            ));
        }
        if bytes[..32] != pin.genesis_hash {
            return err("checkpoint table entry 0 is not the genesis hash");
        }
        let last_entry = (pin.entries - 1) * Self::K;
        if pin.top_number < last_entry || pin.top_number >= last_entry + Self::K {
            return err(format!(
                "checkpoint table top {} is not within [{last_entry}, {}) of its last entry",
                pin.top_number,
                last_entry + Self::K
            ));
        }
        if pin.top_number == last_entry && bytes[bytes.len() - 32..] != pin.top_hash {
            return err("checkpoint table top is its last entry but the hashes differ");
        }
        Ok(Self { entries: bytes, top_number: pin.top_number, top_hash: pin.top_hash })
    }

    /// [`Self::parse`] for tests elsewhere in the workspace (`test-util` feature): a
    /// synthetic table never reaches a production build.
    #[cfg(any(test, feature = "test-util"))]
    pub fn parse_for_test(bytes: &'a [u8], pin: &TablePin) -> Result<Self, CoreError> {
        Self::parse(bytes, pin)
    }

    /// The top: the highest block the table anchors.
    pub fn top(&self) -> (u64, [u8; 32]) {
        (self.top_number, self.top_hash)
    }

    /// The number of entries.
    pub fn len(&self) -> u64 {
        self.entries.len() as u64 / 32
    }

    /// Never true for a parsed table (entry 0 is genesis).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The hash of block `i * K`, if the table holds entry `i`.
    pub fn entry(&self, i: u64) -> Option<[u8; 32]> {
        let start = usize::try_from(i.checked_mul(32)?).ok()?;
        let bytes = self.entries.get(start..start.checked_add(32)?)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(bytes);
        Some(out)
    }

    /// The anchor a block `n` verifies against: the first entry at or above `n`, or the top
    /// for a block above the last entry. `None` above the top (today's window from the
    /// attested finalized block serves those). The anchor is at most `K - 1` blocks above
    /// `n`, so the walk `[n ..= anchor]` is at most `K` headers.
    pub fn anchor_at_or_above(&self, n: u64) -> Option<(u64, [u8; 32])> {
        if n > self.top_number {
            return None;
        }
        let i = n.div_ceil(Self::K);
        match self.entry(i) {
            Some(hash) => Some((i * Self::K, hash)),
            None => Some((self.top_number, self.top_hash)),
        }
    }
}

/// The mainnet table's pin. First built 2026-11-20 by eth2swarm's `ethswarm checkpoint-table`
/// from Erigon 3.8.1, every header hashed and linked by `parentHash` down to the genesis hash;
/// extended for v0.1.21 (2026-11-22) by `--extend` (eth2swarm `2707995`), which reproduced the
/// old last entry (block 26,457,088) and linked every header from it through the old top up to
/// the new one; extended again for v0.1.22 (2026-11-23) the same way, from the old last entry
/// (block 26,467,328) through the old top (26,467,996), for v0.1.23 (2026-11-24), from the
/// old last entry (block 26,476,544) through the old top (26,476,643), and for v0.1.24
/// (2026-11-25), from the old last entry (block 26,485,760) through the old top (26,485,995).
/// Each top is the source node's finalized block, cross-checked against Geth 1.17.7 (203 entries
/// and the top equal). The sidecar `mainnet-1024.hashes.json` records the same, `extended_from`
/// included.
pub const MAINNET_PIN: TablePin = TablePin {
    keccak256: hex32("010a1eb489899bb50571de634be2acf8ebaf3863dc8e3f6a5a120554f029f9be"),
    entries: 25_871,
    genesis_hash: hex32("d4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3"),
    top_number: 26_491_578,
    top_hash: hex32("3a2aa2189683177870dbb0bd13794f868111584f702c5c35c8879eda3165313a"),
};

/// Every mainnet table shipped so far, as `(entries, keccak256 of those entries)`, oldest
/// first. Moving [`MAINNET_PIN`] appends the new pin here (release step 6): the test
/// `every_shipped_table_is_a_prefix` then checks that a release only appended.
pub const MAINNET_SHIPPED: &[(u64, [u8; 32])] =
    &[
        (25_838, hex32("ccf5ad8849bb513d7aef7b7bc732b264e2874c6b6e24cc4880753ceb68cc03d0")),
        (25_848, hex32("2e243c4026af847fb4c31486cec58b4ca8c53022b899562bbb5c5490d8362778")),
        (25_857, hex32("e6cbcf4709eaf0b6023af03903091a9783e4faac24ef7c62b34b3465e9e8013f")),
        (25_866, hex32("cb318629e238aa34b1439cc62e6ae3bcfc1a564906dcb0e7fc3b4cf5bb7c523a")),
        (25_871, hex32("010a1eb489899bb50571de634be2acf8ebaf3863dc8e3f6a5a120554f029f9be")),
    ];

/// The embedded mainnet table's bytes, as committed.
pub static MAINNET_BYTES: &[u8] = include_bytes!("../checkpoints/mainnet-1024.hashes");

/// The embedded mainnet table, parsed against [`MAINNET_PIN`].
pub fn mainnet() -> Result<CheckpointTable<'static>, CoreError> {
    CheckpointTable::parse(MAINNET_BYTES, &MAINNET_PIN)
}

/// The Gnosis Chain table's pin. Built 2026-11-26 by eth2swarm's `ethswarm checkpoint-table`
/// (eth2swarm at `4936aea`, which hashes the AuRa-sealed headers before the merge) from the
/// owner's Gnosis Erigon 3.8.1 archive node, every header hashed and linked by `parentHash`
/// down to the genesis hash. The top is the finalized block of Myotis's Gnosis light client
/// (`myotis-rpcd --network gnosis`, Myotis `ae4dcd2d`), a sync-committee-attested hash passed
/// as `--top-hash`: the execution payload of beacon block 31,331,072, a finalized checkpoint.
/// No second Gnosis execution client was at hand for the 200-entry cross-check mainnet's
/// builds ran. `checkpoints/README.md` records the evidence; the sidecar names only
/// `--top-hash`.
pub const GNOSIS_PIN: TablePin = TablePin {
    keccak256: hex32("af23cdadb52bd536eccc8fc73c5af7a50c26d5c7152ddf4833f3db0d22de45eb"),
    entries: 48_310,
    genesis_hash: hex32("4f1dd23188aab3a76b463e4af801b52b1248ef073c648cbdc4c9333d3da79756"),
    top_number: 49_469_393,
    top_hash: hex32("432535a00c6b7659aec02ce7ff34b693a9e4b36a04110b66e7d5f87e0f1f90e0"),
};

/// Every Gnosis Chain table shipped so far, as [`MAINNET_SHIPPED`] is for mainnet.
pub const GNOSIS_SHIPPED: &[(u64, [u8; 32])] = &[(48_310, hex32("af23cdadb52bd536eccc8fc73c5af7a50c26d5c7152ddf4833f3db0d22de45eb"))];

/// The embedded Gnosis Chain table's bytes, as committed.
pub static GNOSIS_BYTES: &[u8] = include_bytes!("../checkpoints/gnosis-1024.hashes");

/// The embedded Gnosis Chain table, parsed against [`GNOSIS_PIN`].
pub fn gnosis() -> Result<CheckpointTable<'static>, CoreError> {
    CheckpointTable::parse(GNOSIS_BYTES, &GNOSIS_PIN)
}

/// Gnosis Chain's last block sealed by AuRa: the merge came after it. Its headers and those
/// below carry the AuRa step and signature where [`crate::header::BlockHeader`] reads
/// `mixHash` and `nonce`, so they do not decode, though the table holds their hashes.
pub const GNOSIS_LAST_AURA_BLOCK: u64 = 25_349_536;

/// The table for `chain_id`, if one is bundled: mainnet and Gnosis Chain.
pub fn for_chain(chain_id: u64) -> Option<Result<CheckpointTable<'static>, CoreError>> {
    match chain_id {
        1 => Some(mainnet()),
        100 => Some(gnosis()),
        _ => None,
    }
}

/// The lowest block a deep read through `chain_id`'s table serves: the first one whose header
/// this crate decodes. 0 on mainnet; on Gnosis Chain the first block after the merge
/// ([`GNOSIS_LAST_AURA_BLOCK`] + 1). Below it a read keeps the refusal it had without a table,
/// instead of a window no peer's answer would decode.
pub fn first_served(chain_id: u64) -> u64 {
    match chain_id {
        100 => GNOSIS_LAST_AURA_BLOCK + 1,
        _ => 0,
    }
}

/// A 32-byte hex constant (64 lowercase hex digits, no prefix), at compile time.
const fn hex32(s: &str) -> [u8; 32] {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => panic!("not a lowercase hex digit"),
        }
    }
    let b = s.as_bytes();
    assert!(b.len() == 64, "a 32-byte hash is 64 hex digits");
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = nibble(b[2 * i]) << 4 | nibble(b[2 * i + 1]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn mainnet_parses_against_its_pin() {
        let t = mainnet().expect("the committed table matches its pin");
        assert_eq!(t.len(), MAINNET_PIN.entries);
        assert_eq!(t.top(), (MAINNET_PIN.top_number, MAINNET_PIN.top_hash));
        assert_eq!(t.entry(0), Some(MAINNET_PIN.genesis_hash));
        assert_eq!(t.entry(MAINNET_PIN.entries), None);
        assert!(for_chain(1).is_some());
        assert_eq!(first_served(1), 0);
    }

    #[test]
    fn gnosis_parses_against_its_pin() {
        let t = gnosis().expect("the committed table matches its pin");
        assert_eq!(t.len(), GNOSIS_PIN.entries);
        assert_eq!(t.top(), (GNOSIS_PIN.top_number, GNOSIS_PIN.top_hash));
        assert_eq!(t.entry(0), Some(GNOSIS_PIN.genesis_hash));
        assert_eq!(t.entry(GNOSIS_PIN.entries), None);
        assert_eq!(for_chain(100).map(|r| r.unwrap()), Some(t));
        assert_eq!(first_served(100), 25_349_537);
        // Sepolia and Chiado have none.
        assert!(for_chain(11_155_111).is_none());
        assert!(for_chain(10_200).is_none());
    }

    /// Entries of the Gnosis table read from the source node by `debug_getRawHeader` and
    /// hashed apart from the build: block 1024 (AuRa), block 25,349,120 (the last entry below
    /// the merge, AuRa with `baseFeePerGas`), block 25,350,144 (the first entry after it) and
    /// block 45,000,704 (inside the one-year addr1 index, 43,329,216 to 49,465,471).
    #[test]
    fn gnosis_known_entries() {
        let t = gnosis().unwrap();
        for (block, hash) in GNOSIS_SAMPLES {
            assert_eq!(t.entry(block / CheckpointTable::K), Some(hex32(hash)), "block {block}");
        }
    }

    const GNOSIS_SAMPLES: [(u64, &str); 4] = [
        (1024, "d631f578dbf72c0bd8d0a789258c27c82912cb1941a6b79f3b7efd5f21b12818"),
        (25_349_120, "d69325d988375d0fb34fb79a91f56c699e9114e7d3eea47106896a5e091bb974"),
        (25_350_144, "a96fddea91bdd692a218b400c7132b9678dab5cb3a66c2a066e95e0bbf2ce0c6"),
        (45_000_704, "761d24f7ee24c89a7a9e1bf44cd30869dfb48f8479b9d158285890ea23777c82"),
    ];

    /// Release step 6 only appends: every shipped table is a prefix of this one, and the
    /// current pin is the last shipped one.
    #[test]
    fn every_shipped_table_is_a_prefix() {
        for (shipped, bytes, pin) in
            [(MAINNET_SHIPPED, MAINNET_BYTES, &MAINNET_PIN), (GNOSIS_SHIPPED, GNOSIS_BYTES, &GNOSIS_PIN)]
        {
            let mut last = 0;
            for &(entries, hash) in shipped {
                assert!(entries > last, "shipped tables grow");
                let n = usize::try_from(entries * 32).unwrap();
                assert_eq!(keccak256(&bytes[..n]), hash, "{entries} entries");
                last = entries;
            }
            assert_eq!(shipped.last(), Some(&(pin.entries, pin.keccak256)));
        }
    }

    /// The first table's last entry (block 26,457,088), and the DAO-fork block 1,920,000
    /// (entry 1875), whose hash is widely published.
    #[test]
    fn mainnet_known_entries() {
        let t = mainnet().unwrap();
        assert_eq!(
            t.entry(25_837),
            Some(hex32("f84279a9a1b4b7cb664c47bd0f21eb4ccc0f3bdc6ad4932063858de69eebf40b"))
        );
        assert_eq!(
            t.entry(1875),
            Some(hex32("4985f5ca3d2afbec36529aa96f74de3cc10a2a4a6c44f2157a57d2c6059a11bb"))
        );
    }

    #[test]
    fn anchors() {
        let t = mainnet().unwrap();
        const K: u64 = CheckpointTable::K;
        let (top, _) = t.top();
        let last = (MAINNET_PIN.entries - 1) * K;
        // An entry is its own anchor: a walk of one header.
        assert_eq!(t.anchor_at_or_above(0), Some((0, MAINNET_PIN.genesis_hash)));
        assert_eq!(t.anchor_at_or_above(2048).map(|a| a.0), Some(2048));
        // Otherwise the next multiple of 1024.
        assert_eq!(t.anchor_at_or_above(1).map(|a| a.0), Some(1024));
        assert_eq!(t.anchor_at_or_above(2049).map(|a| a.0), Some(3072));
        assert_eq!(t.anchor_at_or_above(last).map(|a| a.0), Some(last));
        // Above the last entry, the top; above the top, none.
        if top > last {
            assert_eq!(t.anchor_at_or_above(last + 1), Some(t.top()));
        }
        assert_eq!(t.anchor_at_or_above(top), Some(t.top()));
        assert_eq!(t.anchor_at_or_above(top + 1), None);
        // The walk is never longer than K.
        for n in [1u64, 1023, 1025, 12_345_678, last + 1, top] {
            let (a, _) = t.anchor_at_or_above(n).unwrap();
            assert!(a >= n && a - n < K, "{n} → {a}");
        }
    }

    fn pin_for(bytes: &[u8], top_number: u64, top_hash: [u8; 32]) -> TablePin {
        let mut genesis = [0u8; 32];
        genesis.copy_from_slice(&bytes[..32]);
        TablePin {
            keccak256: keccak256(bytes),
            entries: bytes.len() as u64 / 32,
            genesis_hash: genesis,
            top_number,
            top_hash,
        }
    }

    #[test]
    fn parse_refuses_what_does_not_match_the_pin() {
        let bytes: Vec<u8> = (0..3u8).flat_map(|i| [i; 32]).collect();
        let good = pin_for(&bytes, 2048 + 5, [9; 32]);
        assert!(CheckpointTable::parse(&bytes, &good).is_ok());

        let mut tampered = bytes.clone();
        tampered[40] ^= 1;
        assert!(CheckpointTable::parse(&tampered, &good).is_err(), "keccak");

        let short = &bytes[..64];
        assert!(CheckpointTable::parse(short, &pin_for(short, 2048, [9; 32])).is_err(), "top");
        let wrong_len = TablePin { entries: 3, ..pin_for(short, 1024, [1; 32]) };
        assert!(CheckpointTable::parse(short, &wrong_len).is_err(), "length");
        assert!(CheckpointTable::parse(&bytes, &TablePin { genesis_hash: [7; 32], ..good })
            .is_err(), "genesis");
        assert!(CheckpointTable::parse(&bytes, &TablePin { top_number: 3072, ..good }).is_err());
        assert!(CheckpointTable::parse(&bytes, &TablePin { top_number: 2047, ..good }).is_err());
        // A top that IS the last entry must carry that entry's hash.
        assert!(CheckpointTable::parse(&bytes, &TablePin { top_number: 2048, ..good }).is_err());
        assert!(CheckpointTable::parse(
            &bytes,
            &TablePin { top_number: 2048, top_hash: [2; 32], ..good }
        )
        .is_ok());
        // A zero-entry pin must be refused before `bytes[..32]` is read.
        let empty = TablePin {
            keccak256: keccak256(&[]),
            entries: 0,
            genesis_hash: [0; 32],
            top_number: 0,
            top_hash: [0; 32],
        };
        assert!(CheckpointTable::parse(&[], &empty).is_err(), "empty");
    }
}
