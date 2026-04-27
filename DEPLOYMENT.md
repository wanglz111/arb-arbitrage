# Deployment

This repository is set up to build a scanner image on GitHub Actions, smoke-test
it, and publish it to GHCR.

## Build Flow

- Push to `main` or `master`
- GitHub Actions runs:
  - `cargo clippy --manifest-path scanner/Cargo.toml -- -D warnings`
  - `cargo test --manifest-path scanner/Cargo.toml`
  - a container smoke test that verifies the scanner starts and stays alive
  - Docker build and push to GHCR
- Published image refs include:
  - `ghcr.io/<owner>/<repo>:latest` on the default branch
  - `ghcr.io/<owner>/<repo>:<branch>`
  - `ghcr.io/<owner>/<repo>:sha-<full-commit-sha>`
- Each successful non-PR run also publishes:
  - a workflow summary with the exact immutable tag and digest
  - a `scanner-image.env` artifact containing ready-to-copy `SCANNER_IMAGE=...` values

## Server Setup

1. Copy `.env.example` to `.env`.
2. Fill in your RPC URLs and scanner env vars.
3. Set `SCANNER_IMAGE` in `.env` to the immutable tag or digest from the latest successful `docker-image` workflow.

Example:

```bash
SCANNER_IMAGE=ghcr.io/wanglz111/arb-arbitrage:sha-<full-commit-sha>
```

or

```bash
SCANNER_IMAGE=ghcr.io/wanglz111/arb-arbitrage@sha256:<image-digest>
```

If `SCANNER_IMAGE` is omitted, `docker-compose.yml` falls back to `ghcr.io/wanglz111/arb-arbitrage:latest`, which is convenient for ad hoc use but not ideal for production rollouts.
4. If the repository or package is private, run:

```bash
docker login ghcr.io
```

Use a GitHub token with at least `read:packages`.

5. Start the scanner:

```bash
docker compose up -d
```

Because `docker-compose.yml` uses `pull_policy: always`, re-running `docker compose up -d` will refresh the configured image before restart.

## Recommended Rollout Flow

1. Push the scanner change to `main` or `master`.
2. Wait for the `docker-image` workflow to succeed.
3. Copy the immutable `SCANNER_IMAGE=...` value from the workflow summary or `scanner-image.env` artifact into the server `.env`.
4. Run `docker compose up -d`.
5. Verify the container image and startup logs:

```bash
docker ps -a
docker compose logs --tail=50 scanner
```

## Notes

- The Docker image only contains the Rust scanner binary.
- The local `.env` file is excluded from both git and Docker build context.
- This setup does not SSH-deploy into the server automatically. It prepares the image so the server can run it directly via Docker Compose.
