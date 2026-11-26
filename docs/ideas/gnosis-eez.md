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
beacon light client stops being a source of truth at the fork. The candidate
anchor is the one [the Base evaluation](base-l2-evaluation.md) arrives at for
Base: the L2 block hash that the rollup's contract on mainnet holds, read
through Myotis's own verified mainnet state. On EEZ that is one storage slot,
it is already a block hash, and Myotis's Gloas-era `ExecAnchor` already takes a
block hash first and its header after. Two things have to be kept apart: the
READ is verified end to end (sync committee → mainnet state root → storage
proof → keccak of the L2 header), but the VALUE is only as good as the
registry's acceptance rule, which is outside today's trust rule (§2, §4). On
top of that, the anchored block was 55 min and about 2 h old in the two samples taken, the state that old is near
the edge of what peers serve, and EEZ adds transaction types and EVM rules.

## 1. The EEZ stack as seen from Myotis

The EEZ team runs RollupX networks today (`eez-association/eez-rollupX`). The
Ethereum-backed one, `rollupx.eez.dev`, is the closest thing to the Gnosis
zone that exists. Measured on it:

| | |
|---|---|
| L2 chain id / network id | 696991 / 696991 |
| L2 genesis | `0xb6bd210b7e824a1aabc4c1ae5b64f5643715441b332c7152e641a36ba01835b2`; fork id `7478b563:0` as the Nethermind follower announces it in its Status (CRC32 of genesis, no fork scheduled yet; a later fork changes it) |
| Block time | 2 s (`syncIntervalBlocks` 6: one Sync block per L1 slot) |
| Registry (L1, mainnet) | `0x4b2760f84b83d6b3829e30a7573b0172b6d6402c`, also the registry of rollup 1 (chain 696990). The EEZL2 proxy on L2 sits at the same address by design (eez-rollupX `deployments/ethereum-696990/README.md`: both managers at one address on their chains), so an address alone does not say which chain a proof is for |
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
correct roots. The Base evaluation rates that kind of assumption in its §3.2:
only a ZK proof verified on L1 fits the rule; an attested proposal is "no — a
new, documented exception at best". This is a new trust anchor and needs the
owner's decision (§4).

## 3. What changes for Myotis at the fork

1. **Anchor.** As §2. Gnosis becomes dependent on the mainnet chain in the
   same process: it is only as synced as mainnet is, as the Base evaluation's
   §7 describes for Base.
2. **Freshness.** The anchored block is settled, not current. Measured
   2026-11-24/25, with the registry read in the L1-finalized state (so L1
   finality is inside these figures): 3,700 blocks (~2 h) and 1,636 blocks
   (55 min) behind the L2 head. A `parentHash` walk only reaches blocks OLDER
   than the anchor, so a block newer than it cannot be verified from it at
   all. The options:
   - **Serve reads at the settled block only.** Three things are then owed,
     none of which exists:
     - *Which block tag it answers.* A wallet asks for `latest`; answering it
       from a block 55 min–2 h behind the head is a well-formed answer to another
       question, which `CLAUDE.md` §Trust rules out (APPLIED or REFUSED). The
       Go engine's precedent is to read only at its finalized anchor and
       refuse `latest` (its `eth_blockNumber` is an open owner decision,
       `.claude/agent/architecture.md`). So either `latest`/`pending` are
       refused and the settled block is served under `finalized` only, or
       "may `latest` mean the settled block, labelled" goes to the owner. That
       decides whether settled-only is a usable wallet profile at all.
     - *An age.* `verifiedHeadAgeMs` (Rust engine) is the time since the
       optimistic head's NUMBER last changed on this node, not the block's age
       (`docs/how-it-works/readiness-and-verified-head-age.md`). On a
       settled-only head it reads about 0 s right after each root lands, for a
       block already 55 min–2 h old, and passes the 45 s warm threshold
       (`ui/…/Readiness.kt`) between roots, so the strip would sit on "not
       ready to transact" nearly always. It needs an age from the anchored
       block's own timestamp and a readiness rule for it: `:myotis-api`
       surface.
     - *What a fee read means.* Nothing in the fee path looks at the anchored
       head's age: the fee cache's `Fresh` has no age bound for the current
       top (`el/feecache.rs`; its `STALE_MAX_*` only bound how long a value
       for an OLDER top is still served), and `FEE_HISTORY_STALE_MAX`
       (`myotis-engine/src/host.rs`) only applies when a build fails. So
       `eth_gasPrice`, `eth_maxPriorityFeePerGas` and `eth_feeHistory` would
       serve a "next block" fee computed 1,636–3,700 blocks ago, as exact. The
       one guard that compares the head with the wall clock, the pre-broadcast
       `send_check::head_is_fresh`, would refuse every send. Whether fee reads
       are refused or served with their age is a decision, not a constant.
   - **Verify the sequencer's signature on newer heads.** Erigon's rollup0
     design gossips EIP-712 `SignedHead`s over libp2p gossipsub
     (`/eez/<chainId>/<rollupId>/heads/0`); RollupX sequencers sign no heads.
     Which one the Gnosis zone gets is open (§6). A signed head rests on one
     operator's key: "peer trusted" under `CLAUDE.md` §Trust, as the Base
     evaluation rates the same evidence (its §2 table, §4 rung 3, and §8
     decision 3: showing it at all, even labelled, is the owner's call). It
     needs its own owner decision; the decision of §4 does not cover it.
3. **Snap serving window.** Nethermind's EEZ overlay serves 16,384 blocks
   back by default, but from outside the public bootnode served less (#768,
   2026-11-11): a root 5,120 blocks back but not 6,144 at 12:15 UTC, and
   2,048 but not 4,096 at 13:55 UTC, so the window is neither a fixed depth
   nor a fixed block. A settled root measured 1,636–3,700 blocks back (L1
   finality included, item 2). At the worst sweep the limit lay somewhere
   between 2,048 and 4,096, so 3,700 may already have been outside it; the
   measurements do not settle that, and the time to fetch adds to it. A
   Nethermind peer answers such a root
   with Disconnect reason 16, not an empty proof. A Disconnect alone does not
   prove an unknown root (the peer may drop us for reasons of its own), so it
   needs a confirming check, e.g. a shallower root the same peer still
   serves, before it counts as "unknown root" rather than a peer fault. The
   Disconnect ends the session, so that check comes first on the same
   session, or after a reconnect and the pause Nethermind's recent-IP filter
   needs (#768).
4. **Header-chain bound.** What the bound has to cover is the distance from
   a newer head down to the anchor it links to (nothing above the anchor
   verifies from it, item 2): 8192 blocks are 4.5 h at 2 s, more than the
   measured settlement lag, as long as the Gnosis zone keeps a block time of
   2 s or more. Below the anchor, older blocks link down as today.
5. **Wire.** Chain id and network id stay 100; the fork id changes at the
   fork. The new transaction types need decoders and must enter the
   `transactionsRoot` and `receiptsRoot` rebuilds, the log index and fee reads.
6. **EVM.** `eth_call` and `eth_estimateGas` need the EEZ rules in revm: the
   system address and system transactions, the FeeVault predeploy
   (`0xeeee…0078`, base fees go there), the L1 block-hash history predeploy
   (`0xeeee…0077`), and whatever of Gnosis's own contracts survives the fork.
   Rust engine only: Besu has no EEZ semantics, so the Java engine drops out
   as it does for Tor and for Base.
7. **Fork boundary.** After the fork the Gnosis light client stops advancing,
   and the weak-subjectivity gate parks it: Gnosis's bound is 3 periods
   (`ChainConfig::gnosis()`, about 34 h), so every node is in `STALE_ANCHOR`
   within two days of the last signed period, and rightly so, since a
   retired validator set can sign a competing history. So the last signed
   Gnosis checkpoint cannot stay a live anchor. A pre-fork block can rest on
   one of two things:
   - a value the release pins: a bundled checkpoint table for chain 100,
     whose top is at or below the fork (today the table is mainnet only,
     `myotis_core::checkpoints::for_chain`, and it changes only in a release
     PR); the read keeps the table's trust;
   - a `parentHash` walk down from a post-fork registry anchor, if the chain
     links across the fork; the read then carries the registry's trust
     label, not the sync committee's.
   Which one is a decision for the Gnosis profile (step 7 of §5).
8. **Discovery.** The RollupX network has one published discv4 seed and no
   DNS list. Crawls from it (two in #768 on 2026-11-11, three on 2026-11-24;
   99–269 nodes dialled each) found no other node of the chain; nodes answering with
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

**The gate.** "Not listed" is not a gate: in the Rust engine a network
resolves by name inside the engine (`resolve_config` →
`catalog::canonical_network_name` → `config_for`), whatever `availableNetworks`
lists, and the engine library is also published for other wallets. And the
final `feature/gnosis-eez` → `main` PR puts this branch into a `main` that must
be releasable at every commit. So, for every part of this branch:

- the registry anchor and the 696991 profile are reachable only from unit and
  integration tests and from `examples/` (as `eez_probe` is);
- never through `config_for` or `catalog`, the UniFFI surface, the C ABI or
  `myotis-rpcd`;
- no host module and no Kotlin/Java code refers to them.

A part that needs more than that (a cargo feature, a host flag) asks the owner
first.

The settled-only freshness policy (§3 item 2: block tags, the age, fee reads)
and sequencer-signed heads are separate decisions, not covered by this one.

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
2. **The 696991 dev profile** (Rust engine, behind the gate of §4) and
   golden vectors captured from a follower: headers, bodies carrying
   `0x45`/`0x46`/`0x76`, receipts, with their roots.
3. Decoders and root rebuilds for the EEZ transaction types.
4. The EEZ rules in revm, checked against the development oracle.
5. The freshness policy, as owner decisions first (§3 item 2): which tags a
   settled-only profile answers, its age and readiness rule, what a fee read
   means; signed heads only on their own trust decision.
6. Peers from configured enodes; the serving window (reason 16 confirmed
   against a shallower root, on the same session or after the reconnect
   pause, before it reads as "unknown root").
7. A Gnosis profile that switches anchors at the fork, including what a
   pre-fork read rests on (§3 item 7); if it is a release-pinned table, a
   bundled checkpoint table for chain 100 is part of this step.

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
