# Arbitrum Triangle Arbitrage Session Context

Last updated: 2026-04-27

## Objective

Build a local-first triangle arbitrage system on Arbitrum with these priorities:

1. Detect opportunities on Uniswap v3 quickly.
2. Minimize RPC usage, especially Alchemy usage.
3. Use Morpho flash loans as the preferred capital source.
4. Start with a scanner-first architecture before optimizing execution.

## Constraints

- Chain: Arbitrum
- Preferred DEX: Uniswap v3
- Preferred flash liquidity: Morpho free flash loans
- User does not provide principal
- Early stage goal: run end-to-end locally without revert
- Later stage goal: event-driven scanner with low RPC cost, deployed on a server and running `24/7`
- User Alchemy allowance is `30M CUs / month`, but scanner optimization should treat `20M CUs / month` as the operating budget target
- Execution path target is now stricter:
  - after a relevant event arrives, the system should be able to derive execution calldata and submit the transaction within roughly `200ms`
- User’s eventual gas tolerance target: around `5 USD`, but current scanner design phase does not enforce gas filtering yet

## What Has Been Verified

### 1. Uniswap v3 pool scan on Arbitrum

Existing files:

- [scan_uniswap_v3_arb.mjs](/Users/edy/lucas/arb-arbitrage/scan_uniswap_v3_arb.mjs)
- [scan-output.json](/Users/edy/lucas/arb-arbitrage/scan-output.json)
- [ARCHITECTURE.md](/Users/edy/lucas/arb-arbitrage/ARCHITECTURE.md)

Confirmed earlier:

- Total Uniswap v3 pools found on Arbitrum: `25,737`
- Fee distribution:
  - `100`: `2,539`
  - `500`: `2,020`
  - `3000`: `7,120`
  - `10000`: `14,058`

Deep core pools identified earlier include:

- `USDC/WETH 0.05%`
- `WBTC/WETH 0.05%`
- `WETH/USDT0 0.05%`
- `WBTC/USDT0 0.05%`
- `USDC/WBTC 0.05%`
- `USDC/USDT0 0.01%`
- `ARB/WETH 0.05%`
- `ARB/USDC 0.3%`

### 2. Morpho flash loans on Arbitrum

Morpho singleton verified:

- `0x6c247b1F6182318877311737BaC0844bAa518F5e`

Foundry tests already written and passed:

- [test/MorphoFlashLoan.t.sol](/Users/edy/lucas/arb-arbitrage/test/MorphoFlashLoan.t.sol)

Verified on Arbitrum fork that Morpho flash loans work for these tokens at tested sizes:

- `USDC`
- `USDT0`
- `WETH`
- `WBTC`
- `wstETH`
- `weETH`

This confirms Morpho is not just theoretically available; fork execution succeeded.

### 3. Local flash-loan + swap smoke execution

Existing files:

- [src/TriangleArb.sol](/Users/edy/lucas/arb-arbitrage/src/TriangleArb.sol)
- [test/TriangleArbFork.t.sol](/Users/edy/lucas/arb-arbitrage/test/TriangleArbFork.t.sol)

Current behavior:

- `TriangleArb.sol` performs:
  - `Morpho.flashLoan`
  - Uniswap v3 multihop `exactInput`
  - repay Morpho in the callback

Fork smoke tests passed for these routes:

- `USDC-WETH-WBTC-USDC`
- `USDC-WETH-USDT0-USDC`
- `USDT0-WETH-WBTC-USDT0`
- `USDC-WETH-ARB-USDC`

Important note:

- This smoke harness can top up local deficits inside the fork test to avoid revert.
- That is test-only behavior and is not valid execution logic for production.

### 4. Basic quote scanner

Existing file:

- [scan_triangles.mjs](/Users/edy/lucas/arb-arbitrage/scan_triangles.mjs)

Current behavior:

- Quotes a small fixed set of known triangle paths
- Uses Uniswap `QuoterV2`
- Good for research and verification
- Not suitable for real scanner architecture because it is RPC-heavy

Current observation from recent runs:

- The tested core routes were gross negative at the sampled sizes
- This is expected and does not invalidate the architecture
- It means fixed-path polling is not enough; scanner must be event-driven

## Current Design Decision

The scanner should be:

- event-driven
- in-memory
- pool-state based
- selective about which pools it tracks
- selective about when it uses on-chain quoting

It should not do:

- full-path brute force on every loop
- frequent `Quoter` calls for every path and size
- full-chain polling through Alchemy

## Current Scanner Status

Recent scanner work has already moved the design toward a local-first hot path:

- live `Swap` events and HTTP backfill both update in-memory pool state directly
- candidate rescoring is incremental and limited to affected triangles only
- local coarse+refine size search is now preferred over exact quote for the hot path
- executor calldata can already be assembled from local simulation without waiting for `Quoter`

Current observation mode requirements:

- treat Alchemy as a scarce resource and keep long-running operation within an effective `20M CUs / month` budget target
- use exact quote only as an optional verification path, not as a hot-path dependency
- keep execution untriggered while debugging
- in debug mode, emit periodic summary statistics for locally profitable candidates instead of noisy per-candidate logs

## Target Scanner Architecture

### Phase 1: Universe Builder

Build a narrow active universe first.

Initial token set:

- `USDC`
- `USDT0`
- `WETH`
- `WBTC`
- `ARB`

Initial fee tiers:

- `100`
- `500`
- `3000`

Initial pool filter:

- high reserve / deep liquidity pools only
- start from tens of pools, not hundreds or thousands

Outputs:

- active pool list
- token metadata
- triangle combinations
- reverse mapping from `pool -> affected triangles`

### Phase 2: State Loader

At startup, batch-load state for active pools.

Pool state to keep in memory:

- `token0`
- `token1`
- `fee`
- `slot0`
- `liquidity`
- `lastUpdatedBlock`

Implementation rule:

- use batch loading / multicall where possible

### Phase 3: Log Watcher

Continuously watch only tracked pools for:

- `Swap`
- `Mint`
- `Burn`

RPC split strategy:

- cheap/public RPC for log streaming
- Alchemy for fallback reads, startup snapshot, and occasional precision queries

This is the main Alchemy-saving design choice.

### Phase 4: Candidate Engine

When one pool changes:

1. update that pool’s in-memory state
2. find triangles that depend on that pool
3. recompute only those triangles
4. reject obviously bad candidates locally

This avoids full graph rescans.

### Phase 5: Quote / Precision Filter

Use a two-stage filter:

1. local rough pricing from pool state
2. exact quote only for candidates that survive rough screening

Short-term:

- exact quote can use Uniswap `QuoterV2`

Long-term:

- replace most `Quoter` usage with a local swap simulator

### Phase 6: Executor Integration

Only after scanner is stable:

- feed scanner candidates into execution logic
- use Morpho flash loans as the first execution path
- then add real profitability / slippage / gas constraints

## Language Decision

Current recommendation:

- `Rust` for the long-running scanner
- keep `Node.js` for one-off scanning and research scripts
- keep `Solidity + Foundry` for on-chain execution and fork tests

Reason:

- the scanner needs concurrency, stable long-running behavior, in-memory state, and low-latency event handling
- Rust is a better long-term fit than continuing with a script-only Node scanner

## Execution Plan

### Step 1

Create a Rust scanner skeleton:

- `scanner/` project or equivalent crate layout
- config loading
- static core token/pool definitions

### Step 2

Implement startup state loading:

- connect RPC
- fetch tracked pool metadata/state
- hold active state in memory

### Step 3

Implement log watching:

- subscribe or poll `Swap` logs only for tracked pools
- update pool state incrementally

### Step 4

Implement triangle dependency graph:

- build `pool -> triangle ids`
- recompute only affected triangles

### Step 5

Implement rough candidate scoring:

- infer spot prices from current state
- estimate whether triangle product can exceed fees
- print candidate events

### Step 6

Add exact quote refinement:

- only quote shortlisted candidates
- avoid quote spam

### Step 7

Connect to executor:

- translate candidate into `TriangleArb.execute(...)`
- optimize the executor ABI and calldata construction for low-latency submission
- keep execution disabled until scanner quality is acceptable

## Immediate Next Task

Build the executor handoff path around the scanner:

- make calldata construction a first-class scanner output
- keep per-event execution preparation cheap enough to fit a `~200ms` reaction budget
- continue reducing dependence on exact quotes so the long-running process stays within the `20M CU / month` operating target

After this execution-handoff pass, the most natural next build steps are:

- improve the new multi-size local search beyond its current discrete ladder
- extend the local simulator beyond the current single-tick approximation
- then add `Mint` / `Burn` handling before attempting broader pool coverage

## Context Log

### 2026-04-27

- Confirmed Uniswap v3 pool universe shape on Arbitrum and identified likely core pools.
- Confirmed Morpho free flash loans work on Arbitrum fork for major relevant tokens.
- Built and tested a local `Morpho -> Uniswap v3 -> repay` triangle arb smoke path.
- Confirmed current fixed known routes are not profitable at sampled sizes.
- Decided scanner must be event-driven and selective, not quote-driven polling.
- Decided Rust is the preferred implementation language for the scanner process.
- Installed Rust toolchain locally with `rustup`, plus `rustfmt` and `clippy`.
- Created [scanner/README.md](/Users/edy/lucas/arb-arbitrage/scanner/README.md) and a new Rust crate at [scanner](/Users/edy/lucas/arb-arbitrage/scanner).
- Added the first scanner modules:
  - [scanner/src/config.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/config.rs)
  - [scanner/src/state.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/state.rs)
  - [scanner/src/watcher.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/watcher.rs)
  - [scanner/src/graph.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/graph.rs)
  - [scanner/src/main.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/main.rs)
- Updated the scanner to auto-load the repo root [`.env`](/Users/edy/lucas/arb-arbitrage/.env), which is the parent directory of `scanner/`.
- Upgraded the scanner watcher architecture to `WebSocket live logs + HTTP backfill`.
- Added scanner config support for:
  - `LOG_WS_URL`
  - `SCANNER_WS_RECONNECT_MS`
  - `SCANNER_MAX_LOG_BLOCK_RANGE`
- Current watcher behavior:
  - if `LOG_WS_URL` is set, the scanner subscribes to live `Swap` logs over WebSocket
  - regardless of WebSocket usage, HTTP backfill remains active to cover missed blocks and reconnect gaps
  - `eth_getLogs` backfill is chunked to small block ranges to stay compatible with free-tier providers
- Optimized refresh RPC usage:
  - bootstrap still loads full static + dynamic pool state
  - post-bootstrap pool refresh now reads only `liquidity()` and `slot0()`
  - repeated `token0()/token1()/fee()` calls were removed from steady-state refresh
- Added duplicate suppression:
  - if the same pool was already processed at the same or newer block, refresh and rescoring are skipped
  - this reduces duplicate work when both WebSocket live logs and HTTP backfill observe the same event
- Added candidate print filtering and ring dedupe:
  - scanner now supports `SCANNER_MIN_EDGE_BPS`
  - only candidates with `edge_bps >= threshold` are printed
  - the same directional triangle ring is canonicalized and printed once instead of repeating under different starting tokens
  - candidate logs now include the three pool fee tiers
- Current scanner behavior:
  - tracks a small curated set of core pools
  - bootstraps `slot0 + liquidity + static metadata`
  - polls `Swap` logs only for tracked pools
  - refreshes changed pool state
  - rescans only affected triangles
  - prints rough `edge_bps` based on spot price and fees
- Validation completed:
  - `cargo check`
  - `cargo fmt`
  - `cargo clippy -- -D warnings`
- Runtime smoke test was not fully waited through because first `cargo run` triggers a longer full binary build, but the codebase is currently type-clean and lint-clean.
- Added a hybrid shortlist + exact quote layer to the Rust scanner:
  - new config supports:
    - `QUOTE_RPC_URL`
    - `SCANNER_ROUGH_SHORTLIST_SIZE`
    - `SCANNER_EXACT_QUOTE_ENABLED`
    - `SCANNER_MAX_EXACT_QUOTES_PER_BLOCK`
  - scanner now:
    - dedupes canonical triangle rings before quoting
    - limits exact quotes per block
    - can split exact quote traffic onto a separate RPC
  - exact quote logic lives in:
    - [scanner/src/quote.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/quote.rs)
- Reworked scanner state updates to decode Uniswap v3 `Swap` events directly:
  - live and backfill watchers now decode:
    - `sqrtPriceX96`
    - `liquidity`
    - `tick`
    - `block_number`
    - `log_index`
  - scanner applies swap updates in-memory using event order instead of refreshing `slot0()` / `liquidity()` by RPC after every swap
  - this makes logs semantically more accurate and reduces repeated RPC reads
  - core files:
    - [scanner/src/watcher.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/watcher.rs)
    - [scanner/src/state.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/state.rs)
    - [scanner/src/main.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/main.rs)
- Added the first lightweight local swap simulator:
  - new file:
    - [scanner/src/simulate.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/simulate.rs)
  - current model:
    - approximates Uniswap v3 swaps within the current tick using `sqrtPriceX96`, `liquidity`, and fee
    - computes `local_edge_bps` before deciding whether a candidate deserves exact quote budget
  - new config:
    - `SCANNER_MIN_LOCAL_EDGE_BPS`
  - candidate logs now expose:
    - `rough_edge_bps`
    - `local_edge_bps`
    - `local_amount_in`
    - `local_amount_out`
- Improved scanner log semantics:
  - candidate `triangle` labels now match the actual simulated `start_token` rotation
  - duplicate live/backfill events that lose the `block_number + log_index` race are now logged as `skipped stale event`
- Extended the lightweight local simulator toward cross-tick awareness:
  - pool bootstrap now loads `tickSpacing()` and keeps it in memory
  - pool bootstrap now also loads nearest initialized tick bounds from `tickBitmap`
  - local simulation now uses nearest initialized tick bounds instead of only raw spacing boundaries when estimating per-leg headroom
  - candidate logs now expose:
    - `local_crosses_tick`
    - `crossed_tick_legs`
    - `max_headroom_ratio`
  - current interpretation:
    - `local_crosses_tick` is still a conservative local warning, but it is now based on nearest initialized tick bounds discovered at bootstrap
    - recent runtime samples showed `local_edge_bps` can still match `QuoterV2` very closely even on samples flagged as crossing
  - current usage:
    - keep using `local_edge_bps` for ranking
    - use `local_crosses_tick` and `max_headroom_ratio` as diagnostics until full multi-tick initialized-liquidity simulation is added
- Validation completed for these newer scanner changes:
  - `cargo fmt`
  - `cargo check`
  - `cargo clippy -- -D warnings`
- Shifted the optimization baseline for production deployment:
  - assume the scanner will run on a server `24/7`
  - treat `20M Alchemy CUs / month` as the design budget even though the account has `30M`
  - prioritize architectures that keep exact-quote usage rare
- Reworked the executor ABI for faster scanner-side calldata generation:
  - [src/TriangleArb.sol](/Users/edy/lucas/arb-arbitrage/src/TriangleArb.sol) now exposes:
    - `execute(uint256 loanAmount, uint256 amountOutMinimum, bytes path)`
  - `loanToken` is no longer passed separately:
    - it is derived from the first token in the Uniswap v3 multihop `path`
  - the contract now validates that the path closes back to the same token
  - router + Morpho approvals are now initialized lazily per token instead of reset/reapproved on every execution
- Added a fast scanner-side executor calldata builder:
  - new files:
    - [scanner/src/path.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/path.rs)
    - [scanner/src/execute.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/execute.rs)
  - current approach:
    - precompute the packed Uniswap v3 `path` for every tracked triangle at startup
    - precompute a static `TriangleArb.execute(uint256,uint256,bytes)` calldata template per triangle
    - when an exact quote is available, only patch the `amountIn` and `amountOutMinimum` words
    - `amountOutMinimum` is clamped so it never drops below `amountIn`, which prevents the prepared payload from knowingly allowing a non-repayable flash-loan path
  - new scanner config supports:
    - `SCANNER_EXECUTION_SLIPPAGE_BPS`
    - `SCANNER_LOG_EXECUTION_CALLDATA`
  - exact-quoted candidate logs now include executor-preparation fields such as:
    - `execution_amount_out_minimum`
    - `execution_path_bytes`
    - `execution_calldata_bytes`
- Validation completed for the executor-calldata changes:
  - `cargo fmt`
  - `cargo check`
  - `cargo clippy -- -D warnings`
  - `cargo test`
  - `forge build`
- Added the first best-size search pass to the scanner:
  - new config:
    - `SCANNER_LOCAL_SIZE_BPS`
  - default local size ladder:
    - `2500,5000,10000,20000,40000`
    - interpreted as `0.25x, 0.5x, 1x, 2x, 4x` of each token’s base `quote_amount_in`
  - current behavior:
    - the scanner first locally simulates all configured coarse size rungs per affected triangle
    - then it runs a small local refinement pass around the best rung, including boundary exploration near the smallest or largest sampled size
    - it keeps the refined best local size before spending exact quote budget
    - exact quote and execution calldata now both use that selected `amountIn`
  - candidate logs now expose:
    - `local_size_bps`
    - `local_gross_profit`
    - `local_search_samples`
    - `local_refinement_samples`
  - important limitation:
    - this is now a coarse-plus-local-refinement search, but still not a continuous optimizer across all possible input sizes
- Removed the requirement that execution planning wait for exact quote RPC:
  - execution calldata can now be generated directly from the local coarse+refine simulator
  - exact quote is still available as an optional precision overlay, but it is no longer required for producing an execution payload
  - candidate logs now expose:
    - `execution_source = local | exact | none`
  - intended usage:
    - keep `SCANNER_EXACT_QUOTE_ENABLED=false` on the fast hot path
    - treat exact quote as optional diagnostics or a slower confirmation path
- Validation completed for the best-size changes:
  - `cargo fmt`
  - `cargo clippy -- -D warnings`
  - `cargo test`
- Re-evaluated the `20M Alchemy CU / month` target against the current endpoint layout:
  - with `SCANNER_POLL_MS=1500`, if `HTTP_RPC_URL` points at Alchemy, the scanner spends about `57,600` `eth_blockNumber` calls per day
  - at Alchemy's published `eth_blockNumber = 10 CU`, that is about `576k CU / day`, or about `17.3M CU / month`, before counting any exact quotes or WebSocket traffic
  - current `.env` also points `LOG_WS_URL` at Alchemy, and Alchemy bills WebSocket subscriptions by delivered bandwidth, so live swap streaming on active Arbitrum pools is not compatible with the `20M CU / month` target
  - conclusion:
    - the current code architecture is directionally correct
    - the current RPC placement is not quota-safe if Alchemy carries live polling or live WebSocket logs
    - Alchemy should be reserved for startup snapshots and rare precision calls, not the hot path
- Re-evaluated historical backtesting readiness:
  - a first-pass replay over historical `Swap` logs is buildable with the current code structure
  - but a trustworthy historical backtest is not ready yet because the scanner still does not replay `Mint` / `Burn`
  - also, the current exact-quote path does not yet evaluate historical state at arbitrary past blocks
  - conclusion:
    - current backtesting can be made approximate quickly
    - current backtesting cannot yet be treated as high-confidence ground truth
- Current validation gap:
  - the updated [test/TriangleArbFork.t.sol](/Users/edy/lucas/arb-arbitrage/test/TriangleArbFork.t.sol) compiles against the new executor ABI
  - a real fork smoke rerun was attempted, but this environment failed before execution because Foundry panicked while trying to use the configured RPC
  - so the post-refactor Solidity path is compile-verified but not yet re-smoke-tested against a live Arbitrum fork in this session
