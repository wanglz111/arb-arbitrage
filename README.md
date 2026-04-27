# arb-arbitrage

Local-first triangle arbitrage research and execution tooling for Arbitrum.

This repo is focused on one concrete goal: detect Uniswap v3 triangle opportunities fast enough to be useful in production, while keeping RPC usage low enough to run `24/7` on a real server. The current design favors off-chain local simulation for the hot path and keeps on-chain checks as the final safety layer.

## Goal

- discover triangle arbitrage opportunities on Arbitrum quickly
- minimize long-running RPC and Alchemy quota usage
- use Morpho flash loans as the preferred capital source
- prepare execution calldata fast enough for an eventual sub-`200ms` reaction path

## Current Status

Working pieces today:

- Rust scanner for event-driven triangle discovery
- in-memory tracked-pool state with live `Swap` updates and HTTP backfill
- rough scoring plus local coarse+refine size search
- fast local execution calldata construction
- Solidity flash-loan executor and fork smoke tests
- Docker image build and `docker compose` deployment path

What is not finished yet:

- full `Mint` / `Burn` event handling
- full multi-tick local simulation
- gas-aware filtering
- automatic live execution

## Quick Start

1. Copy the example env file and fill in your RPC endpoints.

```bash
cp .env.example .env
```

2. Run the scanner locally.

```bash
cargo run --manifest-path scanner/Cargo.toml
```

3. Run scanner checks.

```bash
cargo clippy --manifest-path scanner/Cargo.toml -- -D warnings
cargo test --manifest-path scanner/Cargo.toml
```

4. Run contract tests if you are working on the executor side.

```bash
forge test
```

5. Optional: let `git push` run local checks first.

```bash
git config --local core.hooksPath .githooks
```

The bundled `pre-push` hook runs:

- `cargo clippy --manifest-path scanner/Cargo.toml -- -D warnings`
- `cargo test --manifest-path scanner/Cargo.toml`
- `docker build --platform linux/amd64 -t arb-arbitrage:pre-push .`

Useful overrides:

- `SKIP_DOCKER_BUILD=1 git push`
- `git push --no-verify`

## Deployment

The repo includes:

- a Dockerfile for the Rust scanner
- a GitHub Actions workflow that builds and pushes a GHCR image
- a `docker-compose.yml` that runs the scanner from a server-side `.env`

Typical server flow:

```bash
cp .env.example .env
docker compose up -d
```

More details are in [DEPLOYMENT.md](/Users/edy/lucas/arb-arbitrage/DEPLOYMENT.md).

## Repo Layout

- [scanner/](/Users/edy/lucas/arb-arbitrage/scanner): Rust event-driven scanner
- [src/](/Users/edy/lucas/arb-arbitrage/src): Solidity executor contract
- [test/](/Users/edy/lucas/arb-arbitrage/test): Foundry fork and flash-loan tests
- [ARCHITECTURE.md](/Users/edy/lucas/arb-arbitrage/ARCHITECTURE.md): design notes
- [SESSION_CONTEXT.md](/Users/edy/lucas/arb-arbitrage/SESSION_CONTEXT.md): current working context and decisions
- [scanner/README.md](/Users/edy/lucas/arb-arbitrage/scanner/README.md): scanner-specific details

## Current Operating Direction

- hot path should not depend on exact quote RPC
- Alchemy should be treated as a scarce resource, with an effective target budget around `20M CUs / month`
- debug mode currently favors observation and summary output over automatic triggering

## Next Steps

1. Add `Mint` and `Burn` support to tracked pool state.
2. Improve local simulation quality across tick boundaries.
3. Add execution policy, gas policy, and transaction submission.
4. Validate more paths and sizing logic against historical and live data.
