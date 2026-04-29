# MEV Arbitrage Context

## 1. Address Analysis

Target address:

```text
0xaba6b8aa6a2ecf018980fbf3e8a48b7a8c6f8bc9
```

This address is not a contract. `eth_getCode` returns `0x`, so it is an EOA/searcher wallet.

The EOA controls and calls arbitrage executor contracts, including:

```text
0x84CA4772441bf5bA742910500305069E0D40b9F3
0x141Afa5ca33784fcAF873491a45f461f07C4c277
```

For the observed executor `0x84CA...b9F3`:

- `owner()` returns `0xaba6...8bc9`.
- It implements Balancer flash loan callback:

```solidity
receiveFlashLoan(address[], uint256[], uint256[], bytes)
```

- It calls Balancer Vault flash loan:

```solidity
flashLoan(address recipient, address[] tokens, uint256[] amounts, bytes userData)
```

Balancer Vault:

```text
0xBA12222222228d8Ba445958a75a0704d566BF2C8
```

## 2. What The Bot Does

The bot is doing flash-loan based MEV/arbitrage on Arbitrum.

Observed flow:

1. EOA calls executor with a short calldata selector, commonly `0x046b646c`.
2. Executor borrows WBTC from Balancer Vault.
3. Executor swaps through several AMM pools.
4. Executor ends with slightly more WBTC than borrowed.
5. Executor repays Balancer.
6. Executor sends the WBTC profit to the EOA.

Example transaction:

```text
0x575ccf730bfcc02682ff937c218cae1cff123f6b46fac1bbe823c660c8430a48
```

Observed path:

```text
WBTC -> USDt0 -> USDC -> cbBTC -> WBTC
```

In that transaction:

```text
Borrowed: 0.03481408 WBTC
Repaid:   0.03481408 WBTC
Profit:   0.00000255 WBTC
```

This was roughly around 0.19 USD at the observed price.

Another example:

```text
0x6e188015906854f3143e13e09bde94ad34121565689b3ad2f72ffcefe068bf0d
```

Observed result:

```text
Borrowed: 0.05153938 WBTC
Profit:   0.00000552 WBTC
```

The executor calldata was only 36 bytes in observed calls:

```text
selector + one uint256-like packed parameter
```

This strongly suggests the executor has hardcoded or compressed route logic, and the off-chain searcher mostly passes amount/flags.

## 3. Feasibility

Building something similar is feasible, especially if the scope is narrow.

Recommended first target:

```text
Rust local state engine
  -> fixed whitelist of pools
  -> fixed BTC/stable routes
  -> local quote simulation
  -> amount optimization
  -> eth_call confirmation
  -> send transaction to executor
```

Do not start with a universal MEV searcher. Start with a narrow fixed-route arbitrage system.

Suggested MVP scope:

```text
10-50 pools
WBTC / cbBTC / WETH / USDC / USDT0 / crvUSD routes
Balancer flash loan
Uniswap V3-style pools
Curve pools if needed
```

## 4. Required Algorithms

### Local State Maintenance

Maintain state locally from logs and occasional RPC reconciliation.

For Uniswap V2-style pools:

```text
reserve0
reserve1
fee
```

For Uniswap V3-style pools:

```text
slot0
current tick
sqrtPriceX96
current liquidity
tick bitmap
initialized ticks
liquidityNet per tick
fee tier
```

For Curve-style pools:

```text
balances
rates
amp
fee
admin fee if relevant
```

For Balancer:

```text
Use Vault mainly for flash loans.
Only maintain swap state if routing through Balancer pools.
```

### Quote Simulation

Needed quote algorithms:

```text
Uniswap V2: constant product formula
Uniswap V3: computeSwapStep across ticks
Curve: stableswap invariant / Newton method
```

Every candidate should be locally simulated first. Only promising candidates should be checked with `eth_call`.

### Path Search

For MVP, avoid full graph search.

Use fixed path enumeration:

```text
WBTC -> stable -> stable -> cbBTC -> WBTC
WBTC -> WETH -> stable -> crvUSD -> WBTC
```

For larger search:

```text
Use graph model with -log(rate) to find rough negative cycles.
Then run exact AMM simulation for candidate cycles.
```

### Input Amount Optimization

Profit is not linear because AMM price changes with trade size.

Practical approach:

```text
golden-section search
ternary search
bounded binary/scan hybrid
```

For mixed V3/Curve routes, numerical optimization is more practical than closed-form formulas.

### Executor Calldata

First version can be simple:

```solidity
execute(uint256 amountIn, bytes calldata route)
```

After stable operation, compress calldata:

```text
selector + packed amount + route id / flags
```

The observed bot likely uses hardcoded route IDs or packed route flags because external calldata is very short.

## 5. RPC And CU Cost

If local simulation is implemented correctly, RPC is not the main cost.

RPC is still needed for:

```text
initial pool state loading
log subscriptions
periodic state reconciliation
eth_call confirmation
nonce/gas queries
sendRawTransaction
receipt checks
getLogs backfill after downtime
```

With local state, free RPC or Alchemy Free is enough for MVP and small-scale testing.

Paid RPC becomes useful for:

```text
stability
lower latency
higher rate limits
reliable WebSocket
large getLogs backfills
redundant transaction submission
```

Observed Alchemy CU rough costs:

```text
eth_call:              26 CU
eth_getLogs:           60 CU
eth_sendRawTransaction:40 CU
eth_getReceipt:        20 CU
WS/log event:          roughly bandwidth-based, often around 40 CU
```

Approximate PAYG price:

```text
$0.45 per 1M CU
```

Example cost:

```text
100,000 eth_call/day * 26 CU = 2.6M CU/day
2.6M CU/day * $0.45 / 1M = about $1.17/day
```

Bad architecture example:

```text
50 pools * 1 eth_call/sec * 86400 sec * 26 CU
= 112M CU/day
= about $50/day
```

Good architecture:

```text
logs / sequencer feed
  -> local state update
  -> local route simulation
  -> only profitable candidates get eth_call
  -> only confirmed opportunities get sent
```

Conclusion:

```text
Free RPC is enough for MVP if local simulation is real.
Paid RPC can pay for itself after the strategy has positive expectancy.
RPC cost is usually smaller than failed transaction cost and latency cost.
```

## 6. Profit Threshold

Observed profits can be around 0.2-0.5 USD equivalent per trade.

This can be viable on Arbitrum because gas is cheap, but the real net formula is:

```text
net_profit =
  expected_profit
  - gas_cost
  - failed_tx_expected_cost
  - priority / Timeboost / private path cost
  - slippage buffer
```

For development, start with higher threshold:

```text
>= 1 USD expected net profit
```

After failure rate and latency are under control, lower threshold:

```text
0.5 USD
```

Do not optimize for 0.2 USD opportunities until the execution system is proven.

## 7. Main Risks

Key risks:

```text
state drift
missed logs
wrong V3 tick math
wrong Curve invariant
stale eth_call result
transaction lands too late
competitors take the same opportunity
failed transactions consume gas
Timeboost / ordering disadvantage
executor calldata bug
token decimal / fee-on-transfer edge cases
```

On Arbitrum, transaction ordering and latency matter. Timeboost or other priority paths may become more important than RPC cost for small-profit opportunities.

## 8. Recommended Build Plan

Phase 1: Offline replay

```text
Take known profitable txs
Reconstruct pool states around those blocks
Implement quote simulator
Verify local simulation matches on-chain results
```

Phase 2: Narrow live watcher

```text
Subscribe to logs for 10-30 pools
Maintain local state
Enumerate fixed routes
Print profitable candidates only
No real transactions yet
```

Phase 3: eth_call validation

```text
Deploy simple executor
For each candidate, run eth_call
Compare local expected profit vs executor result
Track false positive rate
```

Phase 4: small live execution

```text
Set high profit threshold
Send low frequency transactions
Track landed/failed/reverted/missed opportunities
```

Phase 5: optimize

```text
compress calldata
reduce gas
add fallback RPC
add private/priority transaction path
lower profit threshold
expand pool whitelist
```

## 9. Practical Conclusion

The realistic path is not to build a universal searcher first.

Build a narrow Rust searcher:

```text
fixed routes
local state
local V3/Curve math
small executor
eth_call confirmation
careful failure accounting
```

This is a manageable project.

The hard parts are:

```text
accurate local state
correct AMM math
low false positive rate
fast execution
low failed transaction rate
```

RPC cost should not block the MVP. Free RPC is enough to begin if the hot path is local simulation.
