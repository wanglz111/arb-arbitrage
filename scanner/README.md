# Scanner

Rust scanner skeleton for event-driven triangle arbitrage discovery on Arbitrum.

## Current Scope

This first version does:

- track a small curated set of core Uniswap v3 pools
- bootstrap `token0/token1/fee/slot0/liquidity`
- subscribe to `Swap` logs over WebSocket when available
- backfill `Swap` logs over HTTP for tracked pools
- update in-memory pool state directly from decoded `Swap` events
- recompute only triangles affected by the changed pools
- print a rough `edge_bps` score from current spot prices and pool fees
- run a lightweight local in-tick simulation before spending exact quotes
- search a small local `amountIn` ladder, then refine around the best rung before choosing a size
- bootstrap nearest initialized tick bounds and use them in local boundary checks
- optionally exact-quote a small shortlist through `QuoterV2`
- limit exact quotes with per-block budget and per-ring dedupe
- precompute per-triangle executor route templates for fast calldata construction after exact quote
- generate executor calldata directly from local simulation when exact quote is disabled or skipped

This version does not do:

- local tick-by-tick swap simulation
- gas-aware filtering
- automatic execution

## Tracked Tokens

- `USDC`
- `USDT0`
- `WETH`
- `WBTC`
- `ARB`

## Tracked Pools

The current pool universe is hardcoded in [src/config.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/config.rs) and intentionally small.

## Environment

The scanner automatically loads:

- [../.env](/Users/edy/lucas/arb-arbitrage/.env)

That means the expected location for RPC config is the repo root, one level above `scanner/`.

- `HTTP_RPC_URL`
  - optional
  - defaults to `https://arb1.arbitrum.io/rpc`
- `LOG_RPC_URL`
  - optional
  - defaults to `HTTP_RPC_URL`
  - recommended to point at a cheap/public RPC for log polling
- `LOG_WS_URL`
  - optional
  - if set, the scanner listens to live `Swap` logs over WebSocket
  - HTTP backfill remains enabled even when WebSocket is used
- `SCANNER_POLL_MS`
  - optional
  - defaults to `1500`
- `SCANNER_MAX_LOG_BLOCK_RANGE`
  - optional
  - defaults to `10`
  - useful for free-tier providers that cap `eth_getLogs` block span
- `SCANNER_RPC_RETRY_MS`
  - optional
  - defaults to `2000`
  - retries latest-block fetches instead of exiting on transient RPC failures
- `SCANNER_MIN_EDGE_BPS`
  - optional
  - defaults to `0`
  - only considers candidates with `rough edge_bps >=` this threshold
- `SCANNER_MIN_LOCAL_EDGE_BPS`
  - optional
  - defaults to `0`
  - single-tick candidates must clear this threshold before exact quote
  - likely cross-tick candidates are still allowed through because the current crossing detector is conservative
- `SCANNER_ROUGH_SHORTLIST_SIZE`
  - optional
  - defaults to `5`
  - only the top locally-ranked candidates in each recompute window are considered for exact quotes
- `SCANNER_LOCAL_SIZE_BPS`
  - optional
  - defaults to `2500,5000,10000,20000,40000`
  - interpreted as basis-point multipliers over each token’s base `quote_amount_in`
  - example:
    - `2500` = `0.25x`
    - `10000` = `1.0x`
    - `40000` = `4.0x`
  - the scanner first simulates all configured coarse sizes
  - then it evaluates a few extra sizes around the best coarse rung, including boundary exploration near the smallest or largest rung
  - only the final selected size is sent to exact quote and executor calldata generation
- `SCANNER_EXACT_QUOTE_ENABLED`
  - optional
  - defaults to `false`
  - if `true`, the scanner exact-quotes shortlist candidates through Uniswap `QuoterV2`
- `SCANNER_MAX_EXACT_QUOTES_PER_BLOCK`
  - optional
  - defaults to `2`
  - hard budget for exact quotes in the same block
- `SCANNER_EXECUTION_SLIPPAGE_BPS`
  - optional
  - defaults to `25`
  - when an exact quote is available, the scanner derives `amountOutMinimum` for the executor by keeping this many basis points of output as slippage buffer
  - the final `amountOutMinimum` is never allowed to fall below `amountIn`, so the prepared calldata cannot knowingly accept a swap that would fail flash-loan repayment
- `SCANNER_LOG_EXECUTION_CALLDATA`
  - optional
  - defaults to `false`
  - if `true`, exact-quoted candidates also log the prepared `TriangleArb.execute(...)` calldata as hex
- `SCANNER_DEBUG_SUMMARY_ENABLED`
  - optional
  - defaults to `false`
  - if `true`, candidate-level `affected triangle rescored` logs are suppressed
  - instead, the scanner emits one periodic debug summary for locally profitable candidates only
- `SCANNER_DEBUG_SUMMARY_INTERVAL_SECS`
  - optional
  - defaults to `300`
  - controls the debug-summary window size in seconds
- `QUOTE_RPC_URL`
  - optional
  - defaults to `HTTP_RPC_URL`
  - recommended to point at a separate RPC if you want to isolate quote traffic from state reads
- `SCANNER_START_FROM_LATEST`
  - optional
  - defaults to `true`
  - if `true`, start watching from the current latest block forward

## Commands

Build checks:

```bash
cargo check
cargo fmt
cargo clippy -- -D warnings
```

Run:

```bash
RUST_LOG=scanner=info cargo run
```

Use a custom RPC:

```bash
HTTP_RPC_URL='https://arb-mainnet.g.alchemy.com/v2/...' RUST_LOG=scanner=info cargo run
```

Recommended split:

```bash
HTTP_RPC_URL='https://arb-mainnet.g.alchemy.com/v2/...'
LOG_RPC_URL='https://arb1.arbitrum.io/rpc'
LOG_WS_URL='wss://arb-mainnet.g.alchemy.com/v2/...'
RUST_LOG=scanner=info cargo run
```

## Module Layout

- [src/main.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/main.rs): startup, bootstrap, live-event loop, backfill loop
- [src/config.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/config.rs): token and pool universe
- [src/execute.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/execute.rs): precomputed route templates plus fast executor calldata assembly
- [src/quote.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/quote.rs): exact quote path encoding and `QuoterV2` calls
- [src/path.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/path.rs): shared Uniswap v3 multihop path packing
- [src/simulate.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/simulate.rs): lightweight local swap simulation using nearest initialized tick bounds
- [src/state.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/state.rs): on-chain state loading, tick spacing, and initialized tick boundary bootstrap
- [src/watcher.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/watcher.rs): tracked-pool HTTP backfill plus live WebSocket watcher
- [src/graph.rs](/Users/edy/lucas/arb-arbitrage/scanner/src/graph.rs): triangle graph and affected-path rescoring

Candidate logs now also show:

- `local_size_bps`
- `local_gross_profit`
- `local_search_samples`
- `local_refinement_samples`
- `execution_source`

Execution source meanings:

- `local`: calldata was prepared from the local coarse+refine simulator only
- `exact`: calldata was prepared from an exact `QuoterV2` result
- `none`: no execution plan could be built

When `SCANNER_DEBUG_SUMMARY_ENABLED=true`, the scanner switches to summary output for candidate observations. Each summary window logs:

- `positive_candidates`
- `max_local_edge_bps`
- `max_local_gross_profit`
- `most_common_triangle`

## Next Steps

1. Add `Mint` and `Burn` handling.
2. Add richer shortlist controls such as cooldowns, token-specific budgets, and rough-score deltas.
3. Replace most exact quotes with a deeper local simulator to stay within long-running RPC/CU budgets.
4. Improve best-size search beyond the current discrete ladder, for example with denser ladders or local refinement around the best rung.
5. Submit the prepared executor calldata through a real transaction sender with nonce/gas policy tuned for sub-200ms reaction time.
