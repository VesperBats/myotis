# Gnosis Chain as an EEZ zone — what Myotis needs, and a development setup

Status: **plan, nothing built.** Written 2026-11-26 as part 1/3 of the
`feature/gnosis-eez` branch; parts 2/3 and 3/3 build steps 1 and 2 of §5
(dev-only, see §4), and the doc reaches `main` with the branch. The numbers were measured on 2026-11-24/25 on the live
RollupX network and will drift; re-measure before acting on them.

GnosisDAO voted to end Gnosis Chain's independent L1 and turn it into a zone of
the Ethereum Economic Zone (EEZ), a rollup settling on Ethereum mainnet. Reports
put the switch at the turn of 2026/2027 or in spring 2027. It is a hard fork that
keeps chain id 100, addresses, balances, contracts and xDAI as the gas token, and
it retires the validator set ([Sources](#sources)). No technical specification of
the migration is public yet. This document collects what follows for Myotis from
what IS public (the EEZ RollupX stack, its Nethermind and Erigon followers, and a
live Ethereum-backed RollupX network), and proposes a setup to develop against
before the Gnosis zone exists.

**Short answer:** after the fork, Gnosis has no sync committee, so the Gnosis
beacon light client stops being a source of truth at the fork. The anchor that
fits the trust model is the one [the Base evaluation](base-l2-evaluation.md)
arrives at for Base: the L2 block hash that the rollup's contract on mainnet
holds, read through Myotis's own verified mainnet state. On EEZ that is one
storage slot, it is already a block hash, and Myotis's Gloas-era
`ExecAnchor` already takes a block hash first and its header after. The hard
parts are elsewhere: the anchored block is 0.5–2 hours old, the state that old
is near the edge of what peers serve, and EEZ adds transaction types and EVM
rules.

## 1. The EEZ stack as seen from Myotis

The EEZ team runs RollupX networks today (`eez-association/eez-rollupX`). The
Ethereum-backed one, `rollupx.eez.dev`, is the closest thing to the Gnosis
zone that exists. Measured on it:

| | |
|---|---|
| L2 chain id / network id | 696991 / 696991 |
| L2 genesis | `0xb6bd210b7e824a1aabc4c1ae5b64f5643715441b332c7152e641a36ba01835b2`; fork id `7478b563:0` as the Nethermind follower announces it in its Status (CRC32 of genesis, no fork scheduled yet; a later fork changes it) |
| Block time | 2 s (`syncIntervalBlocks` 6: one Sync block per L1 slot) |
| Registry (L1, mainnet) | `0x4b2760f84b83d6b3829e30a7573b0172b6d6402c`, also the registry of rollup 1 (chain 696990) |
| Rollup id | 2 |
| Sequencer | reth (`reth/v2.11.0`), behind the public RPC |
| Followers | Nethermind EEZ build ([NethermindEth/nethermind#13867](https://github.com/NethermindEth/nethermind/pull/13867)), eth/68–72 and snap/1; Erigon's `canepat/eez` branch (no P2P on RollupX networks, no snap) |
| Header | 21 RLP fields; Myotis's decoder takes it |
| New transaction types | `0x45` native, `0x46` L1 history update, `0x76` EEZ system transaction |

[PR #768](https://github.com/VesperBats/myotis/pull/768) (`eez_probe`) showed
that the session Myotis ships speaks to the Nethermind follower: eth/71 and
snap/1 negotiate, and account, storage and bytecode proofs verify. On
2026-11-24 the same probe verified every proof against the hash the L1
registry holds (§2), from a self-hosted Nethermind follower.

## 2. The anchor: one storage slot on mainnet

The registry keeps `mapping(uint64 => RollupConfig) rollups` at storage slot 2,
with `RollupConfig = { address rollupContract; bytes32 root; uint256
etherBalance }`. `root` is the **L2 block hash** of the latest settled block,
not a state root: on 2026-11-24 it was `0xfdbd7139…eae10a` at L1 finalized,
the hash of L2 block #1,170,840, which the sequencer also reported as its
`finalized` block. The slot is

```
keccak256(uint256(rollupId) ‖ uint256(2)) + 1
  = 0x679795a0195a1b76cdebb7c51d74e058aee92919b8c3389af86ef24535e8a28d   (rollup 2)
```

The chain of checks, with nothing taken on a peer's word:

1. The mainnet light client gives a sync-committee-verified finalized
   execution block and its state root (today's anchor, unchanged).
2. A snap account proof of the registry and a storage proof of the slot,
   verified against that state root, give the L2 block hash.
3. An L2 peer serves the header by hash; its keccak must be that hash. This
   is the shape of the Gloas path (`ExecAnchor::note_finalized_hash`, then
   `resolve_header`), but not its semantics: that API orders finalities by
   beacon slot and files every resolved root as proven under a sync-committee
   signature. An L2 anchor has no beacon slot of its own (the L1 finalized
   slot it was read at orders it) and rests on the registry, so its reads
   need their own `verifyMethod` and status flavour, never the label of a
   sync-committee anchor.
4. The header's `stateRoot` anchors the L2's snap proofs, and its
   `parentHash` chain verifies older L2 blocks as today.

**What this trusts that today's anchors do not:** the registry's acceptance
rule. A root lands through `postAndVerifyBatch` and the rollup's proof system;
on RollupX that is an attester quorum (2-of-3 in the EEZ team's own posts), not
a ZK proof. Myotis would carry the assumption that the contract only accepts
correct roots, which is the same kind of assumption the Base evaluation rates
in its §3.2. This is a new trust anchor and needs the owner's decision (§4).

## 3. What changes for Myotis at the fork

1. **Anchor.** As §2. Gnosis becomes dependent on the mainnet chain in the
   same process: it is only as synced as mainnet is, as the Base evaluation's
   §7 describes for Base.
2. **Freshness.** The anchored block is settled, not current. Measured
   2026-11-24/25: L1-finalized root 3,700 blocks (~2 h) and 1,636 blocks
   (55 min) behind the L2 head. The engine's freshness guards assume a
   current head and would refuse or degrade every fee read on a
   settled-only profile: the fee cache calls data older than 75 s or 5
   blocks stale (`el/feecache.rs`), fee history goes stale after 64 × 12 s
   (`myotis-engine/src/host.rs`). They need a per-network scale or a
   settled-only meaning. A `parentHash` walk only reaches blocks OLDER
   than the anchor, so a block newer than it cannot be verified from it at
   all. The options:
   - serve reads at the settled block only (honest, 0.5–2 h old, and the
     status screen's "verified head age" says so);
   - verify the sequencer's signature on newer heads. Erigon's rollup0 design
     gossips EIP-712 `SignedHead`s over libp2p gossipsub
     (`/eez/<chainId>/<rollupId>/heads/0`); RollupX sequencers sign no heads.
     Which one the Gnosis zone gets is open (§6). A signed head rests on the
     sequencer key, a weaker evidence rung like the Base evaluation's §4.
3. **Snap serving window.** Nethermind's EEZ overlay serves 16,384 blocks
   back by default, but from outside the public bootnode served less (#768,
   2026-11-11): a root 5,120 blocks back but not 6,144 at 12:15 UTC, and
   2,048 but not 4,096 at 13:55 UTC, so the window is neither a fixed depth
   nor a fixed block. A settled root measured 1,636–3,700 blocks back; at the
   worst of both measurements that is outside the window already, before L1
   finality and fetch time are added. A Nethermind peer answers such a root
   with Disconnect reason 16, not an empty proof. A Disconnect alone does not
   prove an unknown root (the peer may drop us for reasons of its own), so it
   needs a confirming check, e.g. a shallower root the same peer still
   serves, before it counts as "unknown root" rather than a peer fault.
4. **Header-chain bound.** 8192 blocks are 4.5 h at 2 s; enough to walk back
   from a settled anchor, as long as the Gnosis zone keeps a block time of 2 s
   or more.
5. **Wire.** Chain id and network id stay 100; the fork id changes at the
   fork. The new transaction types need decoders and must enter the
   `transactionsRoot` and `receiptsRoot` rebuilds, the log index and fee reads.
6. **EVM.** `eth_call` and `eth_estimateGas` need the EEZ rules in revm: the
   system address and system transactions, the FeeVault predeploy
   (`0xeeee…0078`, base fees go there), the L1 block-hash history predeploy
   (`0xeeee…0077`), and whatever of Gnosis's own contracts survives the fork.
   Rust engine only: Besu has no EEZ semantics, so the Java engine drops out
   as it does for Tor and for Base.
7. **Fork boundary.** Blocks before the fork stay verifiable from the last
   Gnosis checkpoint the sync committee signed; blocks after it through L1.
   A Gnosis profile needs both and a switch between them.
8. **Discovery.** The RollupX network has one published discv4 seed and no
   DNS list. Crawls from it (#768 on 2026-11-11, again 2026-11-24, 170–270
   nodes dialled each) found no other node of the chain; nodes answering with
   another network id and the same genesis turned out to echo whatever genesis
   they are sent. Myotis has to take EEZ peers from configured enodes until the
   Gnosis zone publishes its own.

## 4. Dev-only until the owner decides

Owner's decision, 2026-11-26: the registry anchor of §2 is built and tested in
the Rust engine but **exposed to no host and no wallet**. The trust rule in
`CLAUDE.md` is not changed by this branch. When an EEZ network is to be
exposed, the rule change (a new entry under *Trust*, with scope and limits in
`.claude/agent/data-sources.md`) goes to the owner as its own before/after
question.

## 5. The setup and the order of work

**A. The live protocol target.** A Nethermind EEZ follower of chain 696991 on
the developer's own machine, bootstrapped over snap
(`eez-rollupX/docs/runbooks/nethermind-snap.md`) with a local mainnet node as
its L1. Myotis dials it as a static peer. It is the real protocol on the real
L1, which is what steps 1–4 below need. Two things to know when setting one up:

- No deployment bundle is published for 696991 (the repository's
  `deployments/` only has 696990). The genesis can be rebuilt: the predeploy
  set of eez-rollupX's `genesis.json` with code, nonce, balance and storage read
  at block 0 from the public L2 RPC reproduces the published state root, the
  header fields come from block 0, and `config.rollupX.l1Reference` is L1 block
  26,292,941 (`0x2f185f74…70a6`: its timestamp is genesis − 12 s and its
  `prevRandao` is the L2 genesis `mixHash`). The config fields are not
  covered by the genesis hash, but a wrong one halts the follower against L1
  rather than letting it follow a wrong chain.
- Nethermind's L1 rules loader takes a geth-format mainnet genesis
  (`geth dumpgenesis --mainnet`) only after removing the null header keys and
  `gasUsed`, adding the `0x` to the alloc keys and writing the terminal total
  difficulty as an integer.

The follower's JSON-RPC is a development oracle (compare `eth_call` and gas
estimates against it), never a data source (`CLAUDE.md` §Data sources).

**B. A controllable devnet.** eez-rollupX's Kurtosis package (private PoS L1,
composer, prover, follower; `testing/kurtosis/`). It is the only way to provoke
what the live network will not do on request: a settlement stall, an L1 reorg
(the package ships an L1-fault workload), a Gnosis-shaped L2 genesis. Its cost
for Myotis is a network profile for a private beacon chain (checkpoint, fork
versions), which no other network needs. A cheaper intermediate is the EEZ
team's Chiado-backed RollupX network (699169), whose L1 Myotis already
verifies since #724. Whether that network is still running was not checked.

**C. The migration itself.** Chain 100's state continuing across the hard fork
cannot be faked locally (the state is large and the migration tooling is not
public). The likely first rehearsal is Chiado, and Myotis's Chiado profile
(#724, added with this in mind) is where to test it.

**Order of work:**

1. **Registry anchor (dev-only).** Slot derivation, the L1 account and storage
   proof down to the L2 block hash, and the hand-off of that hash, with the
   L1 slot it was read at, to the L2 chain's anchor under its own trust
   label; tested on a captured mainnet proof.
2. **The 696991 dev profile** (Rust engine, not in any host catalogue) and
   golden vectors captured from a follower: headers, bodies carrying
   `0x45`/`0x46`/`0x76`, receipts, with their roots.
3. Decoders and root rebuilds for the EEZ transaction types.
4. The EEZ rules in revm, checked against the development oracle.
5. The freshness policy: settled-only first, signed heads if the Gnosis zone
   has them; the fee and fee-history staleness guards per network.
6. Peers from configured enodes; the serving window (reason 16 confirmed
   against a shallower root before it reads as "unknown root").
7. A Gnosis profile that switches anchors at the fork.

## 6. Open questions for the Gnosis / EEZ teams

- rollup0 (signed heads) or RollupX for the Gnosis zone?
- Block time and settlement cadence, and thus the age of the anchored block.
- Will the zone's nodes serve snap on public discv4, and from which seeds?
- The fork block or time, the registry address and the rollup id.
- Will there be a Chiado rehearsal, and when?

## Sources

- [GnosisDAO approves Gnosis Chain for the EEZ](https://cointelegraph.com/news/gnosisdao-approves-gnosis-chain-ethereum-economic-zone), Cointelegraph.
- [Gnosis Chain to cease L1 operations in 2027](https://www.kucoin.com/news/flash/gnosis-chain-to-end-l1-operations-in-2027-becomes-first-general-purpose-zone-in-eez), KuCoin News (ChainCatcher).
- [Gnosis approves plan to transition Chain into an Ethereum-based rollup](https://www.bitget.com/amp/news/detail/12560605703117), Bitget News.
- [Building One Ethereum at Gnosis Chain](https://www.gnosis.io/blog/building-one-ethereum-at-gnosis-chain), Gnosis blog.
- [eez-association/eez-rollupX](https://github.com/eez-association/eez-rollupX): runbooks `nethermind-follower.md`, `nethermind-snap.md`; `testing/kurtosis/`.
- [NethermindEth/nethermind#13867](https://github.com/NethermindEth/nethermind/pull/13867), the EEZ plugin.
- Erigon, branch `canepat/eez`: `docs/site/docs/fundamentals/eez-follower.md`, `eez/protocol/rollups.go`.
- [eez-association/eez-rollup-dashboard](https://github.com/eez-association/eez-rollup-dashboard) `src/lib/treasury/rollupx-mainnet-network.json` (registry, rollup id, L1 reference of 696991).
