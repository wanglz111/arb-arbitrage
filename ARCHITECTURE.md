# Arbitrum Uniswap V3 Triangle Arbitrage

## 1. Quick Scan

Data sources used in this pass:

- Chain discovery: Uniswap V3 factory `PoolCreated` logs on Arbitrum
- Valuation snapshot: GeckoTerminal public API for Uniswap V3 Arbitrum pools
- Execution cost anchor: current Arbitrum gas price from RPC

Snapshot notes:

- Latest scanned block: `456762564`
- Total Uniswap V3 pools on Arbitrum: `25,737`
- Fee tier distribution:
  - `0.01% (100)`: `2,539`
  - `0.05% (500)`: `2,020`
  - `0.30% (3000)`: `7,120`
  - `1.00% (10000)`: `14,058`

Important observation:

- Pool count is dominated by `1%`, but deep executable liquidity is dominated by `0.05%`.
- For triangle arbitrage, deep low-fee majors matter much more than raw pool count.

Representative deep pools from the snapshot:

| Pool | Fee | Reserve USD | 24h Volume USD |
|---|---:|---:|---:|
| USDC / WETH | 0.05% | 56.48m | 54.57m |
| WBTC / WETH | 0.05% | 49.37m | 23.07m |
| WBTC / USDT | 0.05% | 13.04m | 9.98m |
| WETH / USDT | 0.05% | 12.57m | 9.22m |
| USDC / WBTC | 0.05% | 8.73m | 5.99m |
| USDC / WETH | 0.30% | 7.96m | 0.81m |
| USDC / USDT | 0.01% | 2.56m | 0.90m |
| ARB / WETH | 0.05% | 1.84m | 5.61m |
| ARB / USDC | 0.30% | 0.71m | 0.13m |

Tokens with strongest presence across sampled deep pools:

- `WETH`
- `WBTC`
- `USDC`
- `USDT`
- `ARB`

## 2. Is This Feasible?

Yes, but only in an event-driven local-state model.

Polling a website or periodically requoting all paths is too slow. On Arbitrum, obvious triangle dislocations are short-lived and usually disappear within blocks or even inside the same block race.

The right framing is not:

- "find a big pool that moved"

The right framing is:

- "maintain a local graph of executable pools"
- "when one pool state changes, only recompute cycles touching that pool"
- "reject paths whose edge depth cannot support notional above fees + gas + safety margin"

Also important:

- Pure Uniswap-internal triangle opportunities do exist, especially across different fee tiers and after large directional swaps.
- But majors are very efficient. The cleanest opportunities are usually tiny and very competitive.
- So the system must optimize latency, filtering quality, and notional sizing, not just path enumeration.

## 3. Best Initial Triangle Set

Start with a small high-quality token universe instead of all 25k pools.

Recommended initial core:

- `USDC`
- `USDT`
- `WETH`
- `WBTC`
- `ARB`

Best initial triangles:

1. `USDC -> WETH -> WBTC -> USDC`
   Total fee baseline: `0.05% + 0.05% + 0.05% = 0.15%`

2. `USDT -> WETH -> WBTC -> USDT`
   Total fee baseline: `0.15%`

3. `USDC -> WETH -> USDT -> USDC`
   Total fee baseline: `0.05% + 0.05% + 0.01% = 0.11%`

4. `USDC -> WETH -> ARB -> USDC`
   Total fee baseline: `0.05% + 0.05% + 0.30% = 0.40%`

Interpretation:

- Triangles among `USDC/USDT/WETH/WBTC` are the most realistic for execution quality.
- ARB triangles are useful, but the fee burden is materially higher because the strong `ARB/USDC` pool in this snapshot is `0.30%`.

## 4. Pool Admission Rules

Do not put every discovered pool into the active arbitrage graph.

I would split pools into three layers:

### A. Discovery layer

- All pools discovered from factory logs
- Used for metadata completeness only

### B. Watch layer

- Pools whose quote asset is in a trusted base set:
  - `USDC`, `USDT`, `WETH`, `WBTC`, `ARB`
- Pools with acceptable fee tier:
  - Prefer `100`, `500`, `3000`
- Pools with estimated reserve above threshold:
  - Example: `reserveUsd >= 250k`

### C. Execution layer

- Pools that also pass executable-depth thresholds for your target trade size
- Example:
  - For a `10k` notional trade, estimated price impact per hop should stay below a fixed cap
  - Total expected profit after fees must exceed:
    - gas cost
    - L1 data fee
    - slippage buffer
    - revert risk buffer

## 5. Architecture

### 5.1 Components

1. `pool-discovery`
   - Backfill factory `PoolCreated`
   - Store:
     - pool address
     - token0/token1
     - fee tier
     - tick spacing
     - created block

2. `token-registry`
   - Store token metadata:
     - symbol
     - decimals
     - trusted/stable/base flags

3. `pool-state-indexer`
   - Maintain latest state per active pool:
     - `slot0`
     - `liquidity`
     - token balances
     - last updated block
   - Subscribe to:
     - `Swap`
     - `Mint`
     - `Burn`
     - `Collect` optional

4. `price-anchor`
   - Maintain local USD estimates for tokens
   - Prefer deriving from trusted onchain anchors:
     - USDC
     - USDT
     - WETH
     - WBTC
   - External APIs are acceptable for research, but not as production truth

5. `graph-builder`
   - Build adjacency by token
   - One edge per pool
   - Edge attributes:
     - fee
     - current mid price
     - liquidity proxy
     - trusted/untrusted score

6. `candidate-engine`
   - On each pool update, only enumerate triangles touching either token in that pool
   - Avoid global recomputation

7. `quote-engine`
   - Two-stage model:
     - Stage 1: fast approximate screening
     - Stage 2: exact quote validation

8. `executor`
   - Submit only if:
     - expected net profit > threshold
     - quote freshness is inside block budget
     - path remains valid after exact quote

### 5.2 Why Two-Stage Quote Matters

If you try to exactly simulate every triangle on every swap event, the system will drown.

Use:

- Stage 1 approximate filter:
  - current spot from `sqrtPriceX96`
  - fee-adjusted edge return
  - rough slippage estimate from liquidity / reserve proxy

- Stage 2 exact validator:
  - staticcall Quoter V2, or
  - your own local V3 swap simulator with initialized ticks

Production recommendation:

- Start with Quoter/staticcall for correctness
- Replace with local simulation once candidate volume is high enough

## 6. Trigger Model

Your intuition is correct: big pool changes often create opportunities.

But the trigger should not be "TVL changed a lot".

Better triggers:

1. `Swap` event on a watched pool
2. Price move exceeds threshold vs previous local state
3. Moved pool belongs to a token triplet in the execution graph
4. Approximate cycle edge product crosses profitability boundary

Example trigger:

- Pool `USDC/WETH 0.05%` receives a large swap
- Recompute only triangles touching `USDC` or `WETH`
- Validate:
  - `USDC-WETH-WBTC-USDC`
  - `USDC-WETH-USDT-USDC`
  - `USDC-WETH-ARB-USDC`

## 7. Profitability Gate

Current RPC gas price snapshot:

- Arbitrum gas price: about `0.020082 gwei`
- WETH price snapshot: about `$2395.42`

This L2 gas price alone is cheap, but actual tx cost on Arbitrum also includes L1 data posting cost. So production profitability must use:

`expected_net = output_usd - input_usd - pool_fees - l2_execution_fee - l1_data_fee - slippage_buffer - failure_buffer`

I would not use "greater than gas fee" as the only condition.

Use:

- `expected_net >= max(absolute_floor, relative_floor * notional)`

Suggested research defaults:

- absolute floor: `$5`
- relative floor: `0.05%` to `0.10%` of notional

For example:

- on `10,000 USDC` notional
- require at least `$10` expected net before execution

This is much safer than a pure gas-floor gate.

## 8. Minimal Viable Build

### Phase 1

- Universe limited to:
  - `USDC`, `USDT`, `WETH`, `WBTC`, `ARB`
- Only watched fee tiers:
  - `100`, `500`, `3000`
- Use external reserve USD only for research dashboards
- Use local RPC state for live pricing
- Use Quoter/staticcall for exact path validation

### Phase 2

- Add local tick map and full V3 simulation
- Expand token universe gradually
- Add cross-fee same-pair arbitrage
- Add cross-DEX expansion if the Uniswap-only opportunity set is too thin

## 9. Practical Conclusion

This is doable.

But the profitable version is not "monitor TVL and then brute force triangles".

It is:

- locally maintain a filtered pool graph
- react to swaps in real time
- only recompute affected triangles
- gate by executable depth and net profit, not by raw pool count

If you want, next I can turn this into a concrete repo skeleton:

- `indexer/`
- `graph/`
- `quote/`
- `executor/`
- `config/tokens.ts`
- `config/pools.ts`

and start with the `USDC/USDT/WETH/WBTC/ARB` triangle engine first.
