# 下一步开发交接

更新时间：2026-04-29

这个文件是给下一轮开发看的中文摘要。继续开发时建议同时带上本文件和 `SESSION_CONTEXT.md`。

## 当前结论

1. 当前主执行资金源固定为 Morpho free flash loan。
   - Arbitrum Morpho singleton：`0x6c247b1F6182318877311737BaC0844bAa518F5e`
   - 不要在没有明确新指令的情况下切到 Balancer、Aave 或其他 flash loan provider。

2. `data/route-catalog.jsonl` 不是盈利记录。
   - 它只是路线目录，当前本地有 `724` 条。
   - 里面的 `sample_amount_in` 只是示例金额，不是最佳金额。
   - 里面的 `sample_execute_route_calldata` 只是示例 calldata，不代表有利润。

3. 真正要看盈利机会，应该看 `data/directional-candidates.jsonl`。
   - 当前本地这个文件是空的。
   - 只有当 scanner 发现本地或 exact quote 正向机会时，才会写入这里。
   - 记录里会包含 `block_number`、触发事件 `trigger`、local/exact 结果、执行 calldata。

4. 不 trigger、不发交易的情况下，当前代码能力已经可以做到：
   - 本地模拟路线；
   - 搜索最佳输入金额；
   - 生成 `execute(...)` 和 `executeRoute(...)` calldata；
   - 可选用 exact quote 或 deployed executor `eth_call` 做确认。

5. 但从当前本地记录看，还没有捕获到一条“利润大于 0 的 candidate calldata”。

## 推荐推进顺序

### 1. 先跑 scanner 找 candidate

先不部署、不发交易，只让 scanner 记录正向机会：

```bash
SCANNER_DEBUG_SUMMARY_ENABLED=true \
SCANNER_EXACT_QUOTE_ENABLED=false \
SCANNER_MIN_PROFIT_USD=1 \
cargo run --manifest-path scanner/Cargo.toml
```

观察输出：

```bash
tail -f data/directional-candidates.jsonl
```

如果长期没有记录，可以临时降低门槛，只为了观察系统是否有方向性输出：

```bash
SCANNER_DEBUG_SUMMARY_ENABLED=true \
SCANNER_EXACT_QUOTE_ENABLED=false \
SCANNER_MIN_PROFIT_USD=0 \
SCANNER_MIN_LOCAL_EDGE_BPS=0 \
cargo run --manifest-path scanner/Cargo.toml
```

判断一条记录是否值得继续看：

```bash
tail -1 data/directional-candidates.jsonl | jq '{
  block_number,
  trigger,
  triangle,
  fees,
  local,
  exact,
  execution
}'
```

重点看：

- `local.gross_profit_usd > 0`
- `local.edge_bps > 0`
- 如果开 exact quote，则看 `exact.profit_usd > 0`
- `execution.calldata` 或 `execution.route_calldata` 是否存在

### 2. 拿 candidate 写 Foundry fork test

拿到第一条正向记录后，不急着部署。先写 fork test 复现。

JSONL 里已经写：

- `block_number`
- `trigger.source`
- `trigger.event`
- `trigger.pool`
- `trigger.pool_address`
- `trigger.log_index`
- `execution.calldata`
- `execution.route_calldata`

Foundry 里按记录的 block fork：

```solidity
vm.createSelectFork(ARBITRUM_RPC_URL, blockNumber);
```

然后部署本地测试合约：

```solidity
RouteArb arb = new RouteArb(MORPHO, SWAP_ROUTER, profitRecipient);
```

直接 replay scanner 产出的 calldata：

```solidity
(bool ok, bytes memory ret) = address(arb).call(calldataFromJson);
require(ok, "candidate calldata reverted");
```

验证：

- 不 revert；
- Morpho 能收回本金；
- executor 不残留本金；
- `profitRecipient` 收到正利润。

注意：fork 到 `block_number` 是该 block 的最终状态；scanner 是在某个 log 后立即计算的中间状态。`trigger.log_index` 用来判断这个风险。如果同一个 block 后续还有相关池事件，fork 最终状态可能和 scanner 当时的中间状态不同。

### 3. fork test 通过后再部署合约

部署后先不发交易，只开 executor `eth_call`：

```bash
SCANNER_EXECUTION_CALL_ENABLED=true \
SCANNER_EXECUTOR_ADDRESS=<deployed RouteArb> \
SCANNER_EXECUTOR_CALLER=<owner/caller> \
SCANNER_EXECUTION_CALL_MODE=direct \
SCANNER_DEBUG_SUMMARY_ENABLED=true \
cargo run --manifest-path scanner/Cargo.toml
```

如果使用预注册 route：

1. 先从 `data/route-catalog.jsonl` 里拿 `set_route_calldata`；
2. owner 调 `setRoute(...)` 预加载路线；
3. scanner 使用：

```bash
SCANNER_EXECUTION_CALL_MODE=route
```

### 4. 最后才考虑真实发交易

不考虑 gas 时，失败最大亏损主要是 gas。但自动发交易前仍建议补：

- 最低 profit buffer；
- max gas / max fee cap；
- nonce 管理；
- 同一机会去重；
- 失败后冷却时间；
- deployed executor 的 owner 权限检查。

## 当前能做的测试

基础检查：

```bash
cargo test --manifest-path scanner/Cargo.toml
cargo clippy --manifest-path scanner/Cargo.toml -- -D warnings
forge test
```

合约侧已经覆盖：

- `RouteArb` route 存储；
- closed path 校验；
- 3-5 hop 校验；
- owner-only route 写入；
- mock Morpho flash loan + mock router 完整执行；
- 本地无 fork 时，fork smoke test 会跳过 Arbitrum 地址无代码的情况。

scanner 侧已经覆盖：

- path encoding；
- execution calldata encoding；
- route calldata encoding；
- amountOutMinimum 不低于 amountIn；
- local multi-tick simulation；
- Mint/Burn 更新 initialized ticks 和 active liquidity；
- bounded ternary sizing。

## 关于 Uniswap V3 精确模拟

当前本地模拟是 `f64` 近似模型，已经支持跨已加载 initialized ticks，但还不是 Uniswap V3 的整数级精确结果。

后续要做的“精确模拟”指的是把 scanner 的本地 swap math 改到接近或等价于 Uniswap V3 合约逻辑：

- 实现或移植 `TickMath`：
  - `getSqrtRatioAtTick`
  - 必要时支持 `getTickAtSqrtRatio`
- 实现或移植 `SqrtPriceMath`：
  - `getNextSqrtPriceFromInput`
  - token0/token1 两个方向的 amount delta
- 实现或移植 `SwapMath.computeSwapStep`：
  - 每一步按 fee、liquidity、sqrtPriceLimit、remaining amount 计算；
  - 正确处理 rounding up/down；
  - 正确处理 fee 从 input 扣除。
- 实现 `FullMath.mulDiv` / `mulDivRoundingUp` 等价逻辑：
  - Rust 里需要用大整数，避免 `u128/u256` 溢出和浮点误差。
- tick crossing 要完全按 V3 规则更新 liquidity：
  - zero-for-one 和 one-for-zero 方向的 `liquidityNet` 符号不同；
  - crossing 后更新当前 tick 和 liquidity。
- 本地状态需要有足够的 initialized ticks：
  - 当前是围绕当前 tick 扫 tick bitmap；
  - 如果最佳金额会跨更多 tick，需要扩大加载范围或按需 lazy load。

完成后，目标是：

- local simulation 和 `QuoterV2` 在同一 block、同一 path、同一 amount 下输出接近一致；
- 对于不需要 exact quote 的热路径，local result 仍可直接生成可靠 calldata；
- exact quote 只作为低频确认或调试工具，不作为热路径依赖。

## 当前跟踪的 token

| Token | Address | Decimals | 默认基准金额 |
|---|---|---:|---:|
| USDC | `0xaf88d065e77c8cC2239327C5EDb3A432268e5831` | 6 | 1000 USDC |
| USDT0 | `0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9` | 6 | 1000 USDT0 |
| WETH | `0x82aF49447D8a07e3bd95BD0d56f35241523fBab1` | 18 | 0.5 WETH |
| WBTC | `0x2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f` | 8 | 0.02 WBTC |
| cbBTC | `0xcbB7C0000aB88B473b1f5aFd9ef808440eed33Bf` | 8 | 0.02 cbBTC |
| ARB | `0x912CE59144191C1204E64559FE8253a0e49E6548` | 18 | 1000 ARB |

注意：上面的“默认基准金额”只是 scanner sizing 的 base amount。实际最佳金额会由 coarse ladder + refinement + bounded ternary 搜索决定。

## 当前跟踪的 Uniswap V3 池子

每条执行路线是 3-5 hop closed route，所以一次 swap 会经过 3 到 5 个 Uniswap V3 池子。当前 route catalog 总共有 `724` 条路线，都是从下面这些池子组合出来的。

| 池子 | Address | Fee | 观察内容 |
|---|---|---:|---|
| USDC/WETH 0.05% | `0xc6962004f452be9203591991d15f6b388e09e8d0` | 500 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| WBTC/WETH 0.05% | `0x2f5e87c9312fa29aed5c179e456625d79015299c` | 500 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| WBTC/USDT0 0.05% | `0x5969efdde3cf5c0d9a88ae51e47d721096a97203` | 500 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| WETH/USDT0 0.05% | `0x641c00a822e8b671738d32a431a4fb6074e5c79d` | 500 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| USDC/WBTC 0.05% | `0x0e4831319a50228b9e450861297ab92dee15b44f` | 500 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| USDC/WETH 0.3% | `0xc473e2aee3441bf9240be85eb122abb059a3b57c` | 3000 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| USDC/USDT0 0.01% | `0xbe3ad6a5669dc0b8b12febc03608860c31e2eef6` | 100 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| WBTC/cbBTC 0.01% | `0x9B42809aaaE8d088eE01FE637E948784730F0386` | 100 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| WBTC/cbBTC 0.05% | `0xE9f9F89bf71548Fefc9b70453B785515B3B98e45` | 500 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| USDC/cbBTC 0.01% | `0x78d218D8549D5AB2E25fB7166219baBb3E9446C5` | 100 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| USDT0/cbBTC 0.01% | `0x56dBe966Ea9A9Ce3C449724D00F5DC619f74762D` | 100 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| WETH/cbBTC 0.05% | `0xb48B15861f9c5b513690fAD7240d741cb40798dE` | 500 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| ARB/WETH 0.05% | `0xc6f780497a95e246eb9449f5e4770916dcd6396a` | 500 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |
| ARB/USDC 0.3% | `0xaebdca1bc8d89177ebe2308d62af5e74885dccc3` | 3000 | Swap / Mint / Burn, slot0, liquidity, tickBitmap/ticks |

需要观察的事件和状态：

- `Swap`：
  - 更新 `sqrtPriceX96`
  - 更新 active `liquidity`
  - 更新 current `tick`
  - 记录 `block_number` 和 `log_index`
- `Mint` / `Burn`：
  - 更新 `tickLower/tickUpper` 的 `liquidityNet`
  - 如果当前 tick 在 range 内，更新 active liquidity
  - 重新计算当前 tick 附近 initialized tick 边界
- 启动时链上读取：
  - `token0`
  - `token1`
  - `fee`
  - `tickSpacing`
  - `slot0`
  - `liquidity`
  - `tickBitmap`
  - `ticks`

## 例子：route 13 经过哪些池子

`route-catalog.jsonl` 里的 route 13：

```text
USDT0 -> WBTC -> USDC -> USDT0
fees: 500 / 500 / 100
```

它经过 3 个池子：

1. `WBTC/USDT0 0.05%`
   - address：`0x5969efdde3cf5c0d9a88ae51e47d721096a97203`
   - 方向：`USDT0 -> WBTC`
2. `USDC/WBTC 0.05%`
   - address：`0x0e4831319a50228b9e450861297ab92dee15b44f`
   - 方向：`WBTC -> USDC`
3. `USDC/USDT0 0.01%`
   - address：`0xbe3ad6a5669dc0b8b12febc03608860c31e2eef6`
   - 方向：`USDC -> USDT0`

这个 route catalog 记录本身不代表盈利。只有当 scanner 对这条 route 在某个 block 的某个 trigger 后写入 `directional-candidates.jsonl`，才说明当时本地判断它有正向机会。

## 下一步最推荐做的事

1. 长跑 scanner，让 `directional-candidates.jsonl` 先出现第一条正向记录。
2. 拿这条记录的 `block_number`、`trigger`、`execution.calldata` 写 Foundry fork test。
3. 如果 fork test 能正利润不 revert，再考虑部署 `RouteArb`。
4. 部署后先开 `SCANNER_EXECUTION_CALL_ENABLED=true`，只做 `eth_call` 模拟。
5. 多次 `eth_call` 通过后，再进入真实发交易阶段。
6. 并行推进 Uniswap V3 精确整数模拟，减少 local 和 exact quote 的偏差。
