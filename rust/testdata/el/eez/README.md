# EEZ registry anchor vectors

Used by `myotis-net/src/el/eez.rs` (dev-only, docs/ideas/gnosis-eez.md §2, §4).

`registry-anchor-mainnet-26495425.txt` holds three things:

- one Ethereum mainnet block header, as raw RLP;
- `eth_getProof` of the EEZ registry `0x4b2760f84b83d6b3829e30a7573b0172b6d6402c` at that block, for the slot of `rollups[2].root`, which is `keccak256(uint256(2) ‖ uint256(2)) + 1`;
- the raw header of the L2 block that slot names: RollupX chain 696991, block 1,217,082.

Captured 2026-11-26:

- The L1 header and proof came from an Erigon 3 mainnet archive node (`debug_getRawHeader`, and `eth_getProof` at the head block). That Erigon refused proofs at older blocks with a witness-root mismatch, so the vector is at a head block, not a finalized one. A state root is a state root either way.
- The L2 header came from the RollupX sequencer's public RPC (`https://rollupx.eez.dev/rpc/l2`, `debug_getRawHeader` by hash).

Both sources were used only to capture bytes. The tests check every one of them:

- keccak of the L1 header is the pinned hash;
- the proofs verify against that header's state root;
- keccak of the L2 header is the hash the proof yields.

A wrong byte fails the test; it cannot pass. The `CLAUDE.md` rule that a local client over HTTP is for debugging only is about where the engine gets its data. These are test inputs that are checked before use.

To recapture, against any mainnet node that serves proofs:

```sh
B=$(cast block-number)                      # or eth_blockNumber
cast rpc debug_getRawHeader $(printf 0x%x $B)
cast proof 0x4b2760f84b83d6b3829e30a7573b0172b6d6402c \
  0x679795a0195a1b76cdebb7c51d74e058aee92919b8c3389af86ef24535e8a28d -B $B
cast rpc debug_getRawHeader <storageProof[0].value> --rpc-url https://rollupx.eez.dev/rpc/l2
```
