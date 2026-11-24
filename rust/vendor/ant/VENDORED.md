# ant, vendored

Upstream: https://github.com/freedom-hq/ant
Commit: b138f6bc074d8c28149017d4e7b1e80db8c67d4d
License: MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`, copied with the crates)

The embedded Swarm node behind `rust/swarm` (crate `myotis-swarm`, the reader of the addr1
address index; `.claude/agent/data-sources.md`, the Swarm carve-out). ant is not published
on crates.io. As a git dependency it put git sources into the lock, so every build on a
fresh machine fetched the repository: a third-party repository's availability became part
of every build, with no registry checksum. Vendored instead (owner's decision, 2026-11-20):
no build fetches anything from GitHub, and moving to another ant commit is a diff in this
repository, reviewed like code.

## What is here

- `crates/`: the seven crates `rust/swarm` builds (`ant-chain`, `ant-control`, `ant-crypto`,
  `ant-node`, `ant-p2p`, `ant-postage`, `ant-retrieval`), byte for byte as upstream has them
  at the commit above, with the patches below applied.
- `Cargo.toml`: ant's workspace manifest, which the crates inherit their edition, lints and
  dependency versions from. Its one change: `members` lists the seven crates instead of all
  of ant's (the others are not copied, and Cargo refuses a missing member). Its
  `[profile.release]` has no effect here: profiles come from the root of the build
  (`rust/swarm`, later the engine).
- `contracts/`: `BzzSwapHelper.bin`, which `ant-chain` embeds (`include_str!` in
  `src/tx.rs`), and its source `BzzSwapHelper.sol`.
- `LICENSE-MIT`, `LICENSE-APACHE`: upstream's.

## Patches

The copy is upstream's at the commit above plus exactly these patches, applied in order by
`scripts/vendor_ant.sh` from `rust/vendor/ant-patches/` (owner's decision, 2026-11-24: "Patch
the vendored copy in Myotis now, and send the same changes upstream"). Each is the change of
an upstream PR, rebased onto the commit above and limited to the vendored files. **A patch
goes when upstream merges its PR**: the next re-vendor to a commit that contains it drops the
patch file and its entry here (the script stops on a patch that no longer applies).

- `ant-patches/0001-outbound-only-node.patch`, freedom-hq/ant#269, upstream PR
  https://github.com/freedom-hq/ant/pull/276. `NodeConfig::outbound_only` /
  `RunConfig::outbound_only` (default off): no listener, no UPnP behaviour (so no port
  mapping on the user's router), no external address registered from a listener or from a
  peer's `observed_addr`, and configured external addresses refused. Why: a fetch-only
  reader inside a wallet should be neither reachable from outside nor announce its public
  address. `rust/swarm` turns it on. The PR's row in ant's `docs/ffi-parity-audit.md` is
  not vendored, so the patch leaves it out.
- `ant-patches/0002-dnsaddr-feature.patch`, freedom-hq/ant#270, upstream PR
  https://github.com/freedom-hq/ant/pull/277. ant-p2p's `dnsaddr` feature (default on,
  forwarded by ant-node): hickory-resolver (for `/dnsaddr` bootnodes) and libp2p's DNS
  transport are built only with it; without it a bootnode named by DNS is refused at start
  (`RunError::DnsBootnode`). Why: hickory 0.25 has advisories fixed only in 0.26, which
  `myotis-net` keeps out of its own graph, and on Android ant sent the lookup to
  Cloudflare's resolver. `rust/swarm` builds with the feature off (it takes ant-p2p and
  ant-node with `default-features = false`) and pins Swarm's mainnet bootnodes as IP
  multiaddrs (`MAINNET_BOOTNODES` in `rust/swarm/src/lib.rs`): no DNS query at run time.
  The PR also turns the feature on explicitly for `ant-ffi` and adds CI steps; those files
  are not vendored, so the patch leaves them out.
- `ant-patches/0003-public-dials-only.patch`, upstream PR
  https://github.com/freedom-hq/ant/pull/278. `NodeConfig::public_dials_only` /
  `RunConfig::public_dials_only` (default off): only globally routable underlays are
  dialed, never a private one, not even inside the host's own subnets (the same-LAN
  exception of ant's underlay filter, `LocalSubnets`), and the host's interfaces are not
  read for it. Why: a hive entry is signed only by the peer it describes, so any Swarm peer
  could have the reader open TCP connections to hosts on the user's LAN, on a port it picks
  (review of VesperBats/myotis#920). `rust/swarm` turns it on for Swarm mainnet (a `dev`
  network lives on a LAN and keeps `allow_private_dials`). The PR's row in ant's
  `docs/ffi-parity-audit.md` is not vendored, so the patch leaves it out.

The third open item, freedom-hq/ant#271 (a panic on peer input aborts the host under
`panic = "abort"`), has no patch: accepted for now (owner's decision, 2026-11-24), the issue
open (`rust/swarm/src/lib.rs`).

Not copied: ant's other crates (`ant-ffi`, `ant-gateway`, `antctl`, `antd`, `antop`),
`xtask`, its `Cargo.lock` (the lock that counts is `rust/swarm/Cargo.lock`), and its docs
and scripts.

## Rules

- **Never edit these files by hand.** Myotis carries no fork of ant: a fix goes upstream,
  and until upstream merges it, it is a patch in `rust/vendor/ant-patches/` (named and
  explained above, with its upstream PR), never an edit of the copy. `scripts/vendor_ant.sh
  --check` finds any change that is neither upstream's nor a listed patch's.
- **Moving to another commit:** `scripts/vendor_ant.sh <commit>`, then let Cargo update
  `rust/swarm/Cargo.lock` (`cargo update --manifest-path rust/swarm/Cargo.toml -p ant-node`,
  plus what the new crates need), in one PR whose description links upstream's compare view
  of the two commits. The diff under `rust/vendor/ant` is ant's change: review it as code
  that runs inside the engine.
- **The commit's age:** like every hand-moved pin (`.github/dependabot.yml`, the inventory
  at its top and the 7-day cooldown), a commit vendored here is at least 7 days old; a
  security fix is the exception, as it is for Dependabot.
- **The commit's origin:** `scripts/vendor_ant.sh` takes only a commit on upstream's
  default branch. GitHub serves a fork's commits through the parent's URL too, so a SHA
  alone does not say the code is upstream's.
- **Checking the copy:** `scripts/vendor_ant.sh --check` fetches upstream at the commit
  above, applies the patch series, and compares the result with what git tracks, file
  modes included; it fails on unstaged edits of the copy or the series, and on a patch this
  file does not name. Builds never fetch it; CI runs it when the copy, the patches or the
  script change (`.github/workflows/vendored-ant.yml`).
- **Agent instruction files** (`CLAUDE.md`, `AGENTS.md`, `.claude/`, `.mcp.json`, ...) in a
  vendored crate are refused by the script: Claude Code would load a nested `CLAUDE.md`.
  An update that brings one is decided about in its PR.
- These crates are not Myotis code: `cargo fmt` and clippy of the Myotis workspaces do not
  cover them, and their warnings are upstream's. As path dependencies they are built
  without the `--cap-lints allow` Cargo gives registry and git dependencies, so a toolchain
  bump can surface warnings in them, and a lint that is deny by default would fail the build
  in files that must not be edited: the fix then goes upstream, or the toolchain waits.

## Watching upstream

ant is on no registry, so no advisory database (RustSec, GitHub advisories, Dependabot
alerts) can name it, as a git pin or as a copy. Its registry dependencies stay covered
through `rust/swarm/Cargo.lock`; ant's own code, which parses what Swarm peers send, is
not. A security fix in ant reaches Myotis only when someone moves this copy. Of the three
open items `rust/swarm/src/lib.rs` lists, freedom-hq/ant#269 and #270 are fixed here by the
patches above until upstream merges them, and #271 is accepted for now. (The third patch,
for LAN dials, has no issue upstream; its PR is #278.)

Who watches it (owner's decision, 2026-11-24): `.github/workflows/ant-upstream-watch.yml`,
daily (`scripts/ant_upstream_watch.sh`). It compares upstream's default branch with the
commit above and reports, in one issue labelled `ant-upstream` and assigned to the owner,
the commits since it (those whose message reads like a security fix singled out), the
tags and releases on them, upstream's published GitHub security advisories, and the state
of the three issues above. It comments only when one of those facts changes (a tag, a
commit whose message reads like a security fix, an advisory, one of the three issues);
other upstream commits only refresh the issue's description. The owner, acting on the
issue, decides whether the copy moves and whether a release follows; the workflow never
re-vendors and never closes the issue. The issues' states also say when a patch can go:
upstream's merge of #276 or #277 closes #269 or #270. #278 closes no issue and the watch
does not follow its state; its merge shows only among the upstream commits the issue lists.

## The SBOM

A path crate has no `source` in a lock, which `scripts/sbom.py` otherwise reads as one of
Myotis's own crates. Since the engine links `myotis-swarm` (its `swarm` feature, part 5 of
`feature/swarm-addr1`'s address-history plan), these crates are in `rust/Cargo.lock`, and
`sbom.py` lists every crate of a tree under `rust/vendor` as its upstream at the commit this
file names (`pkg:cargo` with a `vcs_url`, property `myotis:vendored`), and names each patch
of the tree's series on its crates (property `myotis:patched-by`, one per patch file), so
the SBOM does not claim the bytes are upstream's alone. A vendored tree without its
`VENDORED.md` is an error, so none is left out silently; `rust/vendor/ant-patches` is the
series of `rust/vendor/ant`, not a tree.
