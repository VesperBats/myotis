# The bundled checkpoint tables

The hash of every 1024th block per network, a trust anchor: the rule is in
`.claude/agent/data-sources.md` (Trust, "The bundled checkpoint table"), the design in
`docs/ideas/deep-block-verification.md`, the loader in `../src/checkpoints.rs`.

| File | What |
|---|---|
| `mainnet-1024.hashes` | 32 bytes per entry; entry *i* is the hash of block *i*·1024, from block 0 up to the last multiple of 1024 at or below the top |
| `mainnet-1024.hashes.json` | the build's sidecar: entries, last entry, top block and hash, genesis hash, keccak256, source client, cross-check |
| `gnosis-1024.hashes` | the same for Gnosis Chain (chain 100) |
| `gnosis-1024.hashes.json` | its sidecar |

The loader does not read the sidecars. It checks each table against its pin in
`../src/checkpoints.rs` (`MAINNET_PIN`, `GNOSIS_PIN`): the keccak256, the number of entries,
entry 0 against the genesis hash, and the top. A table that does not match is refused.

## The mainnet table

- **Built** 2026-11-20 by eth2swarm's `ethswarm checkpoint-table` (VesperBats/eth2swarm#32)
  from Erigon 3.8.1. The tool hashed every header itself and linked each one by `parentHash`
  down to the genesis hash `0xd4e56740…8fa3`.
- **Top, as first built:** block 26,457,537 (`0x229a7a21…85fc`), the source node's finalized block. It was
  cross-checked against Geth 1.17.7: 203 entries and the top block were equal. The owner ruled
  on 2026-11-20 that this suffices for the first table. The running node's gap walk, from its
  beacon-attested finalized block down to the top, checks the top again.
- **Why every entry counts as checked:** the walk linked each header to its parent, from
  the top all the way down to the genesis hash. So every entry sits on one hash chain whose
  two ends are checked: the genesis hash, and the cross-checked top. An entry off that chain
  would have broken the walk. The 203 sampled entries are a check of the tool, not the whole
  check.
- **Size (as first built):** 25,838 entries, 826,816 bytes. keccak256 `0xccf5ad88…03d0`.
- **Extended for v0.1.21** (2026-11-22, release step 6) by `ethswarm checkpoint-table --extend`
  (VesperBats/eth2swarm#33; eth2swarm at `2707995`) from the nuc's Erigon 3.8.1, cross-checked
  against Geth 1.17.7 on zbox: 203 entries and the top block were equal. The walk started at the
  old last entry, reproduced it, passed through the old top (block 26,457,537) and went up to the
  new top: block **26,467,996** (`0x2a207fd0…01f6`), the source node's finalized block. The old
  826,816 bytes are kept byte for byte (`cmp -n`). Now 25,848 entries, 827,136 bytes, keccak256
  `0x2e243c40…2778`.
- **Extended for v0.1.22** (2026-11-23, release step 6) the same way: the same `ethswarm` build
  (eth2swarm at `2707995`), the nuc's Erigon 3.8.1, cross-checked against Geth 1.17.7 on zbox
  (203 entries and the top block equal). The walk started at the old last entry (block
  26,467,328), passed through the old top (block 26,467,996) and went up to the new top: block
  **26,476,643** (`0xd9871937…eec2`), the source node's finalized block. The old 827,136 bytes
  are kept byte for byte (`cmp -n`). Now 25,857 entries, 827,424 bytes, keccak256
  `0xe6cbcf47…013f`.
- **Extended for v0.1.23** (2026-11-24, release step 6) the same way: the same `ethswarm` build
  (eth2swarm at `2707995`), the nuc's Erigon 3.8.1, cross-checked against Geth 1.17.7 on zbox
  (203 entries and the top block equal). The walk started at the old last entry (block
  26,476,544), passed through the old top (block 26,476,643) and went up to the new top: block
  **26,485,995** (`0x3d84b431…a6a5`), the source node's finalized block. The old 827,424 bytes
  are kept byte for byte (`cmp -n`). Now 25,866 entries, 827,712 bytes, keccak256
  `0xcb318629…523a`.
- **Extended for v0.1.24** (2026-11-25, release step 6) the same way: the same `ethswarm` build
  (eth2swarm at `2707995`), the nuc's Erigon 3.8.1, cross-checked against Geth 1.17.7 on zbox
  (203 entries and the top block equal). The walk started at the old last entry (block
  26,485,760), passed through the old top (block 26,485,995) and went up to the new top: block
  **26,491,578** (`0x3a2aa218…313a`), the source node's finalized block. The old 827,712 bytes
  are kept byte for byte (`cmp -n`). Now 25,871 entries, 827,872 bytes, keccak256
  `0x010a1eb4…f9be`.

## The Gnosis Chain table

The owner's decision of 2026-11-25 extends D1 to Gnosis Chain (chain 100): a table built from
the owner's own Gnosis archive node, pinned like mainnet's.

- **Built** 2026-11-26 by `ethswarm checkpoint-table` at eth2swarm `4936aea`, from Erigon
  3.8.1 (`--chain=gnosis`, archive). That commit teaches the tool the AuRa headers of Gnosis
  Chain before its merge (block 25,349,536 and below): their RLP carries the AuRa step and
  signature where `mixHash` and `nonce` stand. The tool hashed every header itself (the hash
  had to equal the node's) and linked each one by `parentHash` down to the genesis hash
  `0x4f1dd231…9756`.
- **Top:** block **49,469,393** (`0x432535a0…90e0`), passed as `--top`/`--top-hash` (the
  sidecar can only say "the caller"). What it rests on:
  - It was the finalized block of Myotis's own Gnosis light client on 2026-11-26:
    `myotis-rpcd --network gnosis` built from Myotis `ae4dcd2d` (on `main`), its execution
    block checked against a sync-committee-signed finalized header.
  - That light client was anchored with `--checkpoint-root 0x6bed064e…4a0e --checkpoint-slot
    31328048`, not the release's embedded Gnosis checkpoint (slot 31,321,840). Two operators,
    gnosischain.com (`rpc-gbc`) and publicnode.com, report that root as the finalized,
    canonical block of slot 31,328,048.
  - The beacon block that carries the top is slot **31,331,072** (an epoch boundary), root
    `0x9278d150de40dd53569f4df80f04f97ad5b7e25934509aa4908d01d543829768`. Both operators
    report it finalized, with execution payload block 49,469,393 and hash `0x432535a0…90e0`.
    The pair can be checked against the beacon chain later, without an execution node.
  - The source node's finalized block was the same block with the same hash. That adds
    little: an execution node's finalized block is what its consensus client tells it.
  - No second Gnosis execution client was at hand, so the 200-entry cross-check of mainnet's
    builds was not run. Four entries (blocks 1024, 25,349,120, 25,350,144 and 45,000,704)
    were hashed from the node's raw headers by a keccak apart from the tool, and the test
    `gnosis_known_entries` pins them. They come from the same node, so this checks the tool,
    not the source.
- **Size:** 48,310 entries, 1,545,920 bytes (about 1.55 MB in every app
  that embeds the Rust engine). keccak256 `0xaf23cdad…45eb`.
- **What it serves:** blocks after the merge only (`first_served(100)`, block 25,349,537 up).
  The engine's header decoder does not read AuRa headers, so a deep read of an earlier block
  keeps the refusal it had without a table. The entries below the merge are there because the
  format starts at genesis, and they are checked by the same walk.
- **Above the top** the running node walks the gap down from its attested finalized block,
  as on mainnet. Gnosis Chain makes a block every 5 s, so the gap grows by about 17,000 blocks
  (17 walks of 1024 headers) a day until the next release extends the table.

## Changing a table

A table changes only in a release PR that names its build: the tool, the source node, the
top, and the cross-check. Each release extends it (`.claude/agent/releases.md`, step 6):
replace the file and the sidecar, and move the pin in `../src/checkpoints.rs` in the same
commit: `MAINNET_PIN`, plus the new pin appended to `MAINNET_SHIPPED` (for Gnosis Chain,
`GNOSIS_PIN` and `GNOSIS_SHIPPED`). How a release extends the Gnosis table, and what its top
rests on without a second Gnosis client, is rule text that awaits the owner's sign-off
(`.claude/agent/releases.md`, step 6, names mainnet's only). The new table's bytes
up to the old last entry must equal the old table's, so a release only appends. The test
`every_shipped_table_is_a_prefix` checks that in CI, since GitHub shows a changed table only
as "Binary files differ". A running node never writes here.
A table is committed, never generated at build time, so release outputs stay byte-identical.
Even so, a PR into `main` that changes one needs a green `reproducible builds` run
(`.claude/agent/reproducibility.md`), because the table is embedded in the engine.
