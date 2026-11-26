//! The EEZ settlement anchor: an L2 block hash read from its registry on L1.
//! **DEV-ONLY** (owner's decision, 2026-11-26; docs/ideas/gnosis-eez.md §2,
//! §4). Nothing here is wired into a chain, a host or a wallet read, and the
//! compiler keeps it that way: the module exists only with the `eez-dev`
//! feature, which this crate turns on only through its own dev-dependency
//! (as `fixture-recorder`), so tests and examples see it and no release build
//! does (owner's decision, 2026-11-26, on the review of #932).
//!
//! An EEZ zone (Gnosis Chain after its migration, the RollupX networks today)
//! has no sync committee. What it has is its registry on L1, which keeps
//! `mapping(uint64 => RollupConfig) rollups` at storage slot
//! [`ROLLUPS_MAPPING_SLOT`], with `RollupConfig = { address rollupContract;
//! bytes32 root; uint256 etherBalance }`. `root` is the L2 BLOCK HASH of the
//! latest settled block (checked on mainnet against RollupX chain 696991: at
//! L1 finalized it is the hash of the sequencer's `finalized` block).
//!
//! The chain of checks, nothing taken on a peer's word:
//!
//! 1. an L1 state root the L1's own anchor verified (for a mainnet-settled
//!    zone, the sync-committee [`super::anchor::ExecAnchor`] of mainnet);
//! 2. [`settled_l2_hash`]: the registry's account proof against that root, and
//!    the storage proof of [`root_slot`] against the account's proven
//!    `storageRoot`, give the L2 block hash;
//! 3. [`SettledAnchor::resolve_header`]: an L2 header is adopted only if the
//!    keccak of its own RLP is that hash. Its `stateRoot` then anchors the L2's
//!    snap proofs, and its `parentHash` the walk to older L2 blocks.
//!
//! # Why not `ExecAnchor`
//!
//! [`super::anchor::ExecAnchor`]'s Gloas path has the same shape (a hash
//! first, the header after), but not the same meaning: it orders finalities by
//! beacon slot and files every resolved root in the `stateRootMatch` window as
//! proven under a sync-committee signature. A settled L2 root rests on more
//! than that: on the registry's acceptance rule (the rollup's proof system;
//! on RollupX an attester quorum). It is NOT one of the trust anchors of
//! `CLAUDE.md` §Trust. So it lives in its own type, whose roots can never
//! enter a window that names the sync committee, and whose reads will need
//! their own `verifyMethod` once the owner decides to expose them.

use std::sync::Mutex;

use myotis_core::keccak::keccak256;
use myotis_core::header::BlockHeader;
use myotis_core::trie::{
    decode_account_leaf, decode_storage_leaf, verify_proof, ProofResult, EMPTY_CODE_HASH,
    EMPTY_TRIE_ROOT,
};
use myotis_core::CoreError;

use crate::el::eth::messages::VerifiedHeader;

/// Storage slot of the registry's `rollups` mapping.
pub const ROLLUPS_MAPPING_SLOT: u64 = 2;

/// Offset of `root` inside a `RollupConfig` (after `rollupContract`).
const ROOT_FIELD_OFFSET: u8 = 1;

/// An EEZ zone's registry on its L1: the contract and the zone's rollup id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registry {
    pub address: [u8; 20],
    pub rollup_id: u64,
}

impl Registry {
    /// The storage slot of this zone's `rollups[rollup_id].root`.
    pub fn root_slot(&self) -> [u8; 32] {
        root_slot(self.rollup_id)
    }
}

/// `keccak256(uint256(rollup_id) ‖ uint256(ROLLUPS_MAPPING_SLOT)) + 1`: the
/// slot of `rollups[rollup_id].root` (Solidity's mapping layout; the struct's
/// second field). Rollup 2 (RollupX chain 696991) is
/// `0x679795a0…a28d`.
pub fn root_slot(rollup_id: u64) -> [u8; 32] {
    let mut preimage = [0u8; 64];
    preimage[24..32].copy_from_slice(&rollup_id.to_be_bytes());
    preimage[56..64].copy_from_slice(&ROLLUPS_MAPPING_SLOT.to_be_bytes());
    add_be(keccak256(&preimage), ROOT_FIELD_OFFSET)
}

/// Big-endian `word + n`, wrapping at 2^256 as the EVM's slot arithmetic does.
fn add_be(mut word: [u8; 32], n: u8) -> [u8; 32] {
    let mut carry = n as u16;
    for byte in word.iter_mut().rev() {
        let sum = *byte as u16 + carry;
        *byte = sum as u8;
        carry = sum >> 8;
        if carry == 0 {
            break;
        }
    }
    word
}

/// A verified L1 state: its root, and the L1 beacon slot whose FINALIZED
/// execution state it is. The pairing is the caller's to vouch for: both come
/// from the L1 chain's own anchor (on mainnet the sync-committee
/// `ExecAnchor`'s finalized execution), never from a peer and never from a
/// head that can still reorg. The slot is what orders settlements, so it
/// travels with the root into [`settled_l2_hash`] and out in [`SettledHash`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L1State {
    pub slot: u64,
    pub state_root: [u8; 32],
}

/// An L2 block hash proven in the registry of one [`L1State`]. Built only by
/// [`settled_l2_hash`], so its slot is the slot of the root it was proven
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettledHash {
    l1_slot: u64,
    hash: [u8; 32],
}

impl SettledHash {
    /// The L1 slot whose finalized state named this hash.
    pub fn l1_slot(&self) -> u64 {
        self.l1_slot
    }

    /// The settled L2 block hash.
    pub fn hash(&self) -> [u8; 32] {
        self.hash
    }
}

/// The L2 block hash the registry holds for `registry.rollup_id`, proven
/// against `l1.state_root`. `account_proof` and `storage_proof` are root-first
/// node paths, as `eth_getProof` lists them and as
/// `myotis_wire::snap::fetch::verify_account_proof` /
/// `verify_storage_proof` return them from a snap answer.
///
/// `Ok(None)`: the slot is provably zero — the rollup is not registered, or
/// nothing has settled. `Err`: a proof that does not verify, or no contract
/// at the registry address (no account, or one without code): a configuration
/// error, never "nothing settled". What is NOT checked here: that the address
/// holds the registry rather than another contract, and that `l1.state_root`
/// is a root of the right chain (the EEZL2 proxy on the L2 sits at the same
/// address by design). Both are the caller's to vouch for, as [`L1State`]
/// says: a contract that is not the registry reads as `Ok(None)` or as
/// whatever it keeps in that slot.
pub fn settled_l2_hash(
    l1: &L1State,
    registry: &Registry,
    account_proof: &[Vec<u8>],
    storage_proof: &[Vec<u8>],
) -> Result<Option<SettledHash>, CoreError> {
    let account = match verify_proof(&l1.state_root, &keccak256(&registry.address), account_proof) {
        ProofResult::Found(leaf) => decode_account_leaf(&leaf)?,
        ProofResult::Absent => {
            return Err(CoreError(
                "eez registry: no account at the registry address under this L1 state root".into(),
            ))
        }
        ProofResult::Invalid(reason) => {
            return Err(CoreError(format!(
                "eez registry: account proof invalid: {reason}"
            )))
        }
    };
    if account.code_hash == EMPTY_CODE_HASH {
        return Err(CoreError(
            "eez registry: the registry address holds no code under this L1 state root".into(),
        ));
    }
    if account.storage_root == EMPTY_TRIE_ROOT {
        return Ok(None);
    }
    let slot = registry.root_slot();
    let value = match verify_proof(&account.storage_root, &keccak256(&slot), storage_proof) {
        ProofResult::Found(leaf) => decode_storage_leaf(&leaf)?,
        ProofResult::Absent => return Ok(None),
        ProofResult::Invalid(reason) => {
            return Err(CoreError(format!(
                "eez registry: storage proof invalid: {reason}"
            )))
        }
    };
    // `decode_storage_leaf` refuses more than 32 bytes and a leading zero, and
    // gives the minimal big-endian form: zero is the empty value, and a hash
    // with leading zero bytes comes back shorter, so left-pad.
    if value.is_empty() {
        return Ok(None);
    }
    let mut hash = [0u8; 32];
    hash[32 - value.len()..].copy_from_slice(&value);
    Ok(Some(SettledHash {
        l1_slot: l1.slot,
        hash,
    }))
}

/// What a [`SettledBlock`] rests on beyond the L1's own anchor. Carried on
/// every settled block so that no later wiring can file its root under a
/// sync-committee label: a read anchored on it needs a `verifyMethod` of its
/// own (docs/ideas/gnosis-eez.md §2, §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustBasis {
    /// The registry's acceptance rule (the rollup's proof system; on RollupX
    /// an attester quorum), read through a verified L1 state.
    RegistrySettlement,
}

/// A settled L2 block, adopted by [`SettledAnchor::resolve_header`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettledBlock {
    /// The L1 beacon slot whose finalized state the registry was read at:
    /// what orders one settlement after another.
    pub l1_slot: u64,
    pub number: u64,
    pub hash: [u8; 32],
    pub state_root: [u8; 32],
    pub timestamp: u64,
    pub basis: TrustBasis,
}

/// What [`SettledAnchor::note_settled_hash`] did with a hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// The hash is pending now, waiting for its header.
    Pending,
    /// It is the settled block's hash, read again at a later slot: the slot moved.
    Refreshed,
    /// A read at a slot older than the settled block's or the pending hash's:
    /// nothing changed.
    Ignored,
    /// The read contradicts an earlier one; see [`SettledAnchor::alarm`].
    Alarmed,
}

/// What [`SettledAnchor::resolve_header`] did with a header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolve {
    /// The header hashes to the pending hash and is the settled block now.
    Adopted,
    /// No hash is pending, or this header is not the one pending: nothing changed.
    NotPending,
    /// The header's RLP does not hash to its own `hash` field, or does not
    /// decode: nothing changed.
    HashMismatch,
    /// The header IS the pending hash, but not above the settled block (a
    /// lower number, or the same number on another hash) although it was read
    /// at a later L1 slot: refused, and raised as an [`Alarm::Regressed`].
    Regressed { settled: u64, offered: u64 },
}

/// A contradiction between two reads of the registry. Finalized L1 state
/// neither disagrees with itself nor moves back, so each of these means a
/// wrong input (a root paired with the wrong slot, a non-finalized read) or a
/// registry that did what it must not. Sticky: the first one is kept. From
/// then on [`SettledAnchor::settled`] answers with the alarm instead of a
/// block; what an exposed integration does beyond that (park, re-anchor,
/// tell the user) is not decided, and the anchor stays dev-only until it is
/// (docs/ideas/gnosis-eez.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alarm {
    /// Two different hashes for one L1 slot.
    Conflict {
        l1_slot: u64,
        first: [u8; 32],
        second: [u8; 32],
    },
    /// A later read names the settled block again after a newer hash was noted.
    MovedBack {
        l1_slot: u64,
        settled: [u8; 32],
        dropped: [u8; 32],
    },
    /// A later read names a block not above the settled one.
    Regressed {
        l1_slot: u64,
        settled: u64,
        offered: u64,
    },
}

#[derive(Debug, Default)]
struct Inner {
    settled: Option<SettledBlock>,
    pending: Option<(u64, [u8; 32])>,
    alarm: Option<Alarm>,
}

impl Inner {
    fn raise(&mut self, alarm: Alarm) {
        self.alarm.get_or_insert(alarm);
    }
}

/// The settled-block anchor of one EEZ zone: the hash the registry names,
/// pending until a header for it is offered, then the settled block.
/// Thread-safe; fed by whoever reads the registry, read by whoever serves.
#[derive(Debug, Default)]
pub struct SettledAnchor {
    inner: Mutex<Inner>,
}

impl SettledAnchor {
    pub fn new() -> SettledAnchor {
        SettledAnchor::default()
    }

    /// The registry, read in the L1 finalized state of `noted.l1_slot()`,
    /// names `noted.hash()`.
    pub fn note_settled_hash(&self, noted: SettledHash) -> Note {
        let (slot, hash) = (noted.l1_slot, noted.hash);
        let mut inner = self.inner.lock().expect("settled anchor mutex");
        if let Some((pending_slot, pending)) = inner.pending {
            if pending_slot == slot && pending != hash {
                inner.raise(Alarm::Conflict {
                    l1_slot: slot,
                    first: pending,
                    second: hash,
                });
                return Note::Alarmed;
            }
        }
        if let Some(settled) = inner.settled {
            if settled.l1_slot == slot && settled.hash != hash {
                inner.raise(Alarm::Conflict {
                    l1_slot: slot,
                    first: settled.hash,
                    second: hash,
                });
                return Note::Alarmed;
            }
            if settled.hash == hash {
                if slot < settled.l1_slot {
                    return Note::Ignored;
                }
                if let Some((pending_slot, pending)) = inner.pending {
                    if pending_slot < slot {
                        inner.pending = None;
                        inner.raise(Alarm::MovedBack {
                            l1_slot: slot,
                            settled: hash,
                            dropped: pending,
                        });
                        return Note::Alarmed;
                    }
                }
                if let Some(settled) = inner.settled.as_mut() {
                    settled.l1_slot = slot;
                }
                return Note::Refreshed;
            }
            if slot < settled.l1_slot {
                return Note::Ignored;
            }
        }
        if inner
            .pending
            .is_some_and(|(pending_slot, _)| pending_slot > slot)
        {
            return Note::Ignored;
        }
        inner.pending = Some((slot, hash));
        Note::Pending
    }

    /// The hash waiting for its header, if any.
    pub fn pending_hash(&self) -> Option<[u8; 32]> {
        self.inner
            .lock()
            .expect("settled anchor mutex")
            .pending
            .map(|(_, h)| h)
    }

    /// Offer an L2 header fetched by the pending hash. Adopted only if the
    /// keccak of ITS RAW RLP is the pending hash (recomputed here, not taken
    /// from the decoder's `hash` field alone) and it is above the settled block.
    /// The adopted fields are decoded here from those same hashed bytes, not
    /// taken from the separately held `header.header`.
    pub fn resolve_header(&self, header: &VerifiedHeader) -> Resolve {
        let hash = keccak256(&header.raw_rlp);
        if hash != header.hash {
            return Resolve::HashMismatch;
        }
        let Ok(decoded) = BlockHeader::decode(&header.raw_rlp) else {
            return Resolve::HashMismatch;
        };
        let mut inner = self.inner.lock().expect("settled anchor mutex");
        let Some((l1_slot, pending)) = inner.pending else {
            return Resolve::NotPending;
        };
        if pending != hash {
            return Resolve::NotPending;
        }
        inner.pending = None;
        let number = decoded.number;
        if let Some(settled) = inner.settled {
            if number <= settled.number {
                inner.raise(Alarm::Regressed {
                    l1_slot,
                    settled: settled.number,
                    offered: number,
                });
                return Resolve::Regressed {
                    settled: settled.number,
                    offered: number,
                };
            }
        }
        inner.settled = Some(SettledBlock {
            l1_slot,
            number,
            hash,
            state_root: decoded.state_root,
            timestamp: decoded.timestamp,
            basis: TrustBasis::RegistrySettlement,
        });
        Resolve::Adopted
    }

    /// The settled block, once one was adopted, or the alarm that forbids
    /// resting on it: one lock, so a block is never handed out under an
    /// alarm raised before the answer.
    pub fn settled(&self) -> Result<Option<SettledBlock>, Alarm> {
        let inner = self.inner.lock().expect("settled anchor mutex");
        match inner.alarm {
            Some(alarm) => Err(alarm),
            None => Ok(inner.settled),
        }
    }

    /// The first contradiction seen, if any (sticky).
    pub fn alarm(&self) -> Option<Alarm> {
        self.inner.lock().expect("settled anchor mutex").alarm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myotis_core::header::BlockHeader;

    const VECTOR: &str =
        include_str!("../../../testdata/el/eez/registry-anchor-mainnet-26495425.txt");

    struct Vector {
        l1_block_number: u64,
        l1_block_hash: [u8; 32],
        l1_state_root: [u8; 32],
        l1_header_rlp: Vec<u8>,
        registry: Registry,
        root_slot: [u8; 32],
        account_proof: Vec<Vec<u8>>,
        storage_proof: Vec<Vec<u8>>,
        l2_block_hash: [u8; 32],
        l2_block_number: u64,
        l2_state_root: [u8; 32],
        l2_timestamp: u64,
        l2_header_rlp: Vec<u8>,
    }

    fn bytes(hex: &str) -> Vec<u8> {
        let hex = hex.trim().trim_start_matches("0x");
        assert!(hex.len().is_multiple_of(2), "odd hex length");
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
            .collect()
    }

    fn word<const N: usize>(hex: &str) -> [u8; N] {
        bytes(hex).try_into().expect("width")
    }

    fn vector() -> Vector {
        let mut one = std::collections::HashMap::new();
        let (mut account_proof, mut storage_proof) = (Vec::new(), Vec::new());
        for line in VECTOR
            .lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        {
            let (key, value) = line.split_once('=').expect("key=value");
            match key {
                "account_proof" => account_proof.push(bytes(value)),
                "storage_proof" => storage_proof.push(bytes(value)),
                _ => {
                    one.insert(key, value);
                }
            }
        }
        Vector {
            l1_block_number: one["l1_block_number"].parse().unwrap(),
            l1_block_hash: word(one["l1_block_hash"]),
            l1_state_root: word(one["l1_state_root"]),
            l1_header_rlp: bytes(one["l1_header_rlp"]),
            registry: Registry {
                address: word(one["registry"]),
                rollup_id: one["rollup_id"].parse().unwrap(),
            },
            root_slot: word(one["root_slot"]),
            account_proof,
            storage_proof,
            l2_block_hash: word(one["l2_block_hash"]),
            l2_block_number: one["l2_block_number"].parse().unwrap(),
            l2_state_root: word(one["l2_state_root"]),
            l2_timestamp: one["l2_timestamp"].parse().unwrap(),
            l2_header_rlp: bytes(one["l2_header_rlp"]),
        }
    }

    fn verified(raw: Vec<u8>) -> VerifiedHeader {
        VerifiedHeader {
            hash: keccak256(&raw),
            header: BlockHeader::decode(&raw).unwrap(),
            raw_rlp: raw,
        }
    }

    fn l2_header() -> VerifiedHeader {
        verified(vector().l2_header_rlp)
    }

    /// The settled block of an anchor with no alarm.
    fn block(anchor: &SettledAnchor) -> SettledBlock {
        anchor.settled().expect("no alarm").expect("a settled block")
    }

    /// The settled block an alarmed anchor still keeps (and no longer hands out).
    fn kept(anchor: &SettledAnchor) -> SettledBlock {
        anchor.inner.lock().unwrap().settled.expect("a settled block")
    }

    fn at(l1_slot: u64, hash: [u8; 32]) -> SettledHash {
        SettledHash { l1_slot, hash }
    }

    fn l1(v: &Vector) -> L1State {
        L1State {
            slot: 100,
            state_root: v.l1_state_root,
        }
    }

    /// A header whose RLP hashes to `hash` is out of reach in a test, so the
    /// "other block" cases offer the vector's header under a pending hash
    /// that is its own, after a settled block placed above or at it.
    fn settled_at(anchor: &SettledAnchor, l1_slot: u64, number: u64, hash: [u8; 32]) {
        anchor.inner.lock().unwrap().settled = Some(SettledBlock {
            l1_slot,
            number,
            hash,
            state_root: [0; 32],
            timestamp: 0,
            basis: TrustBasis::RegistrySettlement,
        });
    }

    #[test]
    fn root_slot_matches_independent_keccak() {
        // Computed off-line (pycryptodome keccak256 of the 64-byte preimage, + 1).
        assert_eq!(
            root_slot(2),
            word("679795a0195a1b76cdebb7c51d74e058aee92919b8c3389af86ef24535e8a28d")
        );
        assert_eq!(
            root_slot(1),
            word("e90b7bceb6e7df5418fb78d8ee546e97c83a08bbccc01a0644d599ccd2a7c2e1")
        );
        assert_eq!(vector().registry.root_slot(), vector().root_slot);
    }

    #[test]
    fn slot_offset_carries_and_wraps() {
        let mut word = [0xffu8; 32];
        word[0] = 0x01;
        let mut want = [0u8; 32];
        want[0] = 0x02;
        assert_eq!(add_be(word, 1), want);
        assert_eq!(add_be([0xff; 32], 1), [0; 32]);
        let mut low = [0u8; 32];
        low[31] = 0x41;
        let mut want = [0u8; 32];
        want[31] = 0x42;
        assert_eq!(add_be(low, 1), want);
    }

    #[test]
    fn mainnet_vector_proves_the_l2_hash_end_to_end() {
        let v = vector();
        // The L1 header is the block the vector names, and its state root is the root the proofs use.
        assert_eq!(keccak256(&v.l1_header_rlp), v.l1_block_hash);
        let l1_header = BlockHeader::decode(&v.l1_header_rlp).unwrap();
        assert_eq!(l1_header.number, v.l1_block_number);
        assert_eq!(l1_header.state_root, v.l1_state_root);
        let noted = settled_l2_hash(&l1(&v), &v.registry, &v.account_proof, &v.storage_proof)
            .unwrap()
            .expect("rollup 2 has settled");
        assert_eq!(noted, at(100, v.l2_block_hash));

        let anchor = SettledAnchor::new();
        assert_eq!(anchor.note_settled_hash(noted), Note::Pending);
        assert_eq!(anchor.pending_hash(), Some(v.l2_block_hash));
        let header = l2_header();
        assert_eq!(header.header.number, v.l2_block_number);
        assert_eq!(anchor.resolve_header(&header), Resolve::Adopted);
        assert_eq!(anchor.pending_hash(), None);
        assert_eq!(
            block(&anchor),
            SettledBlock {
                l1_slot: 100,
                number: v.l2_block_number,
                hash: v.l2_block_hash,
                state_root: v.l2_state_root,
                timestamp: v.l2_timestamp,
                basis: TrustBasis::RegistrySettlement,
            }
        );
        assert_eq!(anchor.alarm(), None);
    }

    #[test]
    fn a_wrong_state_root_proves_nothing() {
        let v = vector();
        let mut root = v.l1_state_root;
        root[31] ^= 1;
        let l1 = L1State {
            slot: 100,
            state_root: root,
        };
        assert!(settled_l2_hash(&l1, &v.registry, &v.account_proof, &v.storage_proof).is_err());
    }

    #[test]
    fn a_tampered_proof_node_proves_nothing() {
        let v = vector();
        for which in 0..2 {
            let (mut account, mut storage) = (v.account_proof.clone(), v.storage_proof.clone());
            let nodes = if which == 0 {
                &mut account
            } else {
                &mut storage
            };
            let last = nodes.len() - 1;
            let mid = nodes[last].len() / 2;
            nodes[last][mid] ^= 0x01;
            let got = settled_l2_hash(&l1(&v), &v.registry, &account, &storage);
            assert!(got.is_err(), "tampered {which}: {got:?}");
        }
    }

    #[test]
    fn another_rollup_id_is_not_read_from_this_proof() {
        // The storage proof is for rollup 2's slot; asked for rollup 7, it
        // must not yield rollup 2's hash.
        let v = vector();
        let other = Registry {
            rollup_id: 7,
            ..v.registry
        };
        let got = settled_l2_hash(&l1(&v), &other, &v.account_proof, &v.storage_proof);
        assert!(!matches!(got, Ok(Some(_))), "{got:?}");
    }

    #[test]
    fn a_wrong_registry_address_is_an_error_not_unsettled() {
        let v = vector();
        let other = Registry {
            address: [0x11; 20],
            ..v.registry
        };
        assert!(settled_l2_hash(&l1(&v), &other, &v.account_proof, &v.storage_proof).is_err());
    }

    #[test]
    fn a_header_whose_rlp_does_not_hash_to_its_hash_field_is_refused() {
        let v = vector();
        let anchor = SettledAnchor::new();
        anchor.note_settled_hash(at(1, v.l2_block_hash));
        let mut forged = l2_header();
        forged.raw_rlp[40] ^= 1;
        assert_eq!(anchor.resolve_header(&forged), Resolve::HashMismatch);
        assert_eq!(anchor.pending_hash(), Some(v.l2_block_hash));
        assert_eq!(anchor.settled(), Ok(None));
    }

    #[test]
    fn a_header_nobody_asked_for_changes_nothing() {
        let anchor = SettledAnchor::new();
        assert_eq!(anchor.resolve_header(&l2_header()), Resolve::NotPending);
        anchor.note_settled_hash(at(1, [0x22; 32]));
        assert_eq!(anchor.resolve_header(&l2_header()), Resolve::NotPending);
        assert_eq!(anchor.pending_hash(), Some([0x22; 32]));
        assert_eq!(anchor.settled(), Ok(None));
    }

    #[test]
    fn an_older_l1_read_is_ignored() {
        let v = vector();
        let anchor = SettledAnchor::new();
        anchor.note_settled_hash(at(10, v.l2_block_hash));
        assert_eq!(anchor.note_settled_hash(at(9, [0x33; 32])), Note::Ignored);
        assert_eq!(anchor.pending_hash(), Some(v.l2_block_hash));
        assert_eq!(anchor.resolve_header(&l2_header()), Resolve::Adopted);
        assert_eq!(anchor.note_settled_hash(at(9, [0x44; 32])), Note::Ignored);
        assert_eq!(
            anchor.note_settled_hash(at(9, v.l2_block_hash)),
            Note::Ignored
        );
        assert_eq!(block(&anchor).l1_slot, 10);
        assert_eq!(anchor.alarm(), None);
    }

    #[test]
    fn the_settled_hash_again_moves_its_slot_forward() {
        let v = vector();
        let anchor = SettledAnchor::new();
        anchor.note_settled_hash(at(10, v.l2_block_hash));
        anchor.resolve_header(&l2_header());
        assert_eq!(
            anchor.note_settled_hash(at(25, v.l2_block_hash)),
            Note::Refreshed
        );
        assert_eq!(block(&anchor).l1_slot, 25);
        assert_eq!(anchor.pending_hash(), None);
        assert_eq!(anchor.alarm(), None);
    }

    #[test]
    fn two_hashes_for_one_slot_raise_a_conflict() {
        let anchor = SettledAnchor::new();
        assert_eq!(anchor.note_settled_hash(at(100, [0xaa; 32])), Note::Pending);
        assert_eq!(anchor.note_settled_hash(at(100, [0xbb; 32])), Note::Alarmed);
        assert_eq!(anchor.pending_hash(), Some([0xaa; 32]));
        assert_eq!(
            anchor.alarm(),
            Some(Alarm::Conflict {
                l1_slot: 100,
                first: [0xaa; 32],
                second: [0xbb; 32]
            })
        );
        // Against the settled block's own slot too.
        let v = vector();
        let anchor = SettledAnchor::new();
        anchor.note_settled_hash(at(10, v.l2_block_hash));
        anchor.resolve_header(&l2_header());
        assert_eq!(anchor.note_settled_hash(at(10, [0xcc; 32])), Note::Alarmed);
        assert!(matches!(
            anchor.alarm(),
            Some(Alarm::Conflict { l1_slot: 10, .. })
        ));
    }

    #[test]
    fn the_settled_hash_after_a_newer_one_raises_moved_back() {
        let v = vector();
        let anchor = SettledAnchor::new();
        anchor.note_settled_hash(at(10, v.l2_block_hash));
        anchor.resolve_header(&l2_header());
        assert_eq!(anchor.note_settled_hash(at(11, [0xdd; 32])), Note::Pending);
        assert_eq!(
            anchor.note_settled_hash(at(12, v.l2_block_hash)),
            Note::Alarmed
        );
        assert_eq!(anchor.pending_hash(), None);
        assert_eq!(
            anchor.alarm(),
            Some(Alarm::MovedBack {
                l1_slot: 12,
                settled: v.l2_block_hash,
                dropped: [0xdd; 32]
            })
        );
    }

    #[test]
    fn a_settlement_that_moves_back_is_refused() {
        let v = vector();
        let anchor = SettledAnchor::new();
        settled_at(&anchor, 10, v.l2_block_number + 5, [0x55; 32]);
        anchor.note_settled_hash(at(11, v.l2_block_hash));
        assert_eq!(
            anchor.resolve_header(&l2_header()),
            Resolve::Regressed {
                settled: v.l2_block_number + 5,
                offered: v.l2_block_number
            }
        );
        assert_eq!(kept(&anchor).hash, [0x55; 32]);
        assert!(matches!(anchor.settled(), Err(Alarm::Regressed { .. })));
        assert_eq!(anchor.pending_hash(), None);
        assert_eq!(
            anchor.alarm(),
            Some(Alarm::Regressed {
                l1_slot: 11,
                settled: v.l2_block_number + 5,
                offered: v.l2_block_number
            })
        );
    }

    #[test]
    fn the_same_number_on_another_hash_is_refused() {
        let v = vector();
        let anchor = SettledAnchor::new();
        settled_at(&anchor, 10, v.l2_block_number, [0x66; 32]);
        anchor.note_settled_hash(at(11, v.l2_block_hash));
        assert_eq!(
            anchor.resolve_header(&l2_header()),
            Resolve::Regressed {
                settled: v.l2_block_number,
                offered: v.l2_block_number
            }
        );
        assert_eq!(kept(&anchor).hash, [0x66; 32]);
        assert!(anchor.settled().is_err());
        assert!(anchor.alarm().is_some());
    }

    #[test]
    fn the_first_alarm_is_kept() {
        let anchor = SettledAnchor::new();
        anchor.note_settled_hash(at(100, [0xaa; 32]));
        anchor.note_settled_hash(at(100, [0xbb; 32]));
        anchor.note_settled_hash(at(100, [0xcc; 32]));
        assert_eq!(
            anchor.alarm(),
            Some(Alarm::Conflict {
                l1_slot: 100,
                first: [0xaa; 32],
                second: [0xbb; 32]
            })
        );
    }

    #[test]
    fn a_later_settlement_replaces_an_earlier_one() {
        let v = vector();
        let anchor = SettledAnchor::new();
        settled_at(&anchor, 10, v.l2_block_number - 900, [0x77; 32]);
        assert_eq!(
            anchor.note_settled_hash(at(11, v.l2_block_hash)),
            Note::Pending
        );
        assert_eq!(anchor.resolve_header(&l2_header()), Resolve::Adopted);
        assert_eq!(block(&anchor).number, v.l2_block_number);
        assert_eq!(block(&anchor).l1_slot, 11);
        assert_eq!(anchor.alarm(), None);
    }

    /// A one-leaf trie holding `value` (already RLP) under `keccak(key)`: the
    /// leaf node, hex-prefixed with the full 64-nibble path (flag `0x20`), is
    /// the whole proof and its keccak the root.
    fn one_leaf(key: &[u8], value_rlp: Vec<u8>) -> ([u8; 32], Vec<Vec<u8>>) {
        use myotis_core::rlp::{encode, Item};
        let mut path = vec![0x20];
        path.extend_from_slice(&keccak256(key));
        let node = encode(&Item::List(vec![Item::Bytes(path), Item::Bytes(value_rlp)]));
        (keccak256(&node), vec![node])
    }

    fn account_rlp(storage_root: [u8; 32], code_hash: [u8; 32]) -> Vec<u8> {
        use myotis_core::rlp::{encode, Item};
        encode(&Item::List(vec![
            Item::Bytes(vec![1]),
            Item::Bytes(Vec::new()),
            Item::Bytes(storage_root.to_vec()),
            Item::Bytes(code_hash.to_vec()),
        ]))
    }

    fn storage_rlp(value: &[u8]) -> Vec<u8> {
        myotis_core::rlp::encode(&myotis_core::rlp::Item::Bytes(value.to_vec()))
    }

    const REGISTRY: Registry = Registry { address: [0x4b; 20], rollup_id: 2 };

    /// A state with the registry at `storage` (`None`: an empty storage trie)
    /// and code `code_hash`; returns the L1 state and both proofs.
    fn synthetic(
        storage: Option<(&[u8; 32], &[u8])>,
        code_hash: [u8; 32],
    ) -> (L1State, Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let (storage_root, storage_proof) = match storage {
            Some((slot, value)) => one_leaf(slot, storage_rlp(value)),
            None => (EMPTY_TRIE_ROOT, Vec::new()),
        };
        let (state_root, account_proof) = one_leaf(&REGISTRY.address, account_rlp(storage_root, code_hash));
        (L1State { slot: 7, state_root }, account_proof, storage_proof)
    }

    #[test]
    fn a_hash_with_a_leading_zero_byte_is_left_padded() {
        let mut hash = [0x5a; 32];
        hash[0] = 0;
        let slot = REGISTRY.root_slot();
        // The leaf keeps the minimal form: 31 bytes.
        let (l1, account, storage) = synthetic(Some((&slot, &hash[1..])), [0xc0; 32]);
        let got = settled_l2_hash(&l1, &REGISTRY, &account, &storage).unwrap();
        assert_eq!(got, Some(at(7, hash)));
    }

    #[test]
    fn an_absent_slot_is_unsettled() {
        // The storage trie holds another slot: an exclusion proof for ours.
        let other = root_slot(9);
        let (l1, account, storage) = synthetic(Some((&other, &[0x11; 32])), [0xc0; 32]);
        assert_eq!(settled_l2_hash(&l1, &REGISTRY, &account, &storage).unwrap(), None);
    }

    #[test]
    fn an_empty_storage_trie_is_unsettled() {
        let (l1, account, storage) = synthetic(None, [0xc0; 32]);
        assert_eq!(settled_l2_hash(&l1, &REGISTRY, &account, &storage).unwrap(), None);
    }

    #[test]
    fn a_registry_address_without_code_is_an_error() {
        let slot = REGISTRY.root_slot();
        let (l1, account, storage) = synthetic(Some((&slot, &[0x11; 32])), EMPTY_CODE_HASH);
        assert!(settled_l2_hash(&l1, &REGISTRY, &account, &storage).is_err());
        let (l1, account, storage) = synthetic(None, EMPTY_CODE_HASH);
        assert!(settled_l2_hash(&l1, &REGISTRY, &account, &storage).is_err());
    }

    #[test]
    fn an_alarm_withholds_the_settled_block() {
        let v = vector();
        let anchor = SettledAnchor::new();
        anchor.note_settled_hash(at(10, v.l2_block_hash));
        anchor.resolve_header(&l2_header());
        assert!(anchor.settled().is_ok());
        anchor.note_settled_hash(at(10, [0xee; 32]));
        let alarm = anchor.alarm().expect("conflict");
        assert_eq!(anchor.settled(), Err(alarm));
    }

    #[test]
    fn the_adopted_fields_come_from_the_hashed_bytes() {
        // A header whose decoded fields were altered after decoding: the
        // keccak still matches its raw RLP, and the anchor must adopt what
        // those bytes say, not the altered fields.
        let v = vector();
        let anchor = SettledAnchor::new();
        anchor.note_settled_hash(at(10, v.l2_block_hash));
        let mut header = l2_header();
        header.header.state_root = [0x99; 32];
        header.header.number += 1_000;
        assert_eq!(anchor.resolve_header(&header), Resolve::Adopted);
        assert_eq!(block(&anchor).state_root, v.l2_state_root);
        assert_eq!(block(&anchor).number, v.l2_block_number);
    }
}
