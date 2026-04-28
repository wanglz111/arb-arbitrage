# Deployment

This repository no longer builds or publishes scanner images through GitHub Actions.
Image publication is intentionally local and explicit through `.githooks/pre-commit`.

## Local Build Flow

Enable the repository hooks once:

```bash
git config --local core.hooksPath .githooks
```

Before each commit, the `pre-commit` hook:

- runs scanner clippy with `-D warnings`
- runs scanner tests
- builds the Docker image from the staged git tree
- smoke-tests the image
- pushes GHCR tags:
  - `ghcr.io/<owner>/<repo>:tree-<staged-tree-hash>`
  - `ghcr.io/<owner>/<repo>:<branch>`
  - `ghcr.io/<owner>/<repo>:latest`
- writes `scanner-image.env` with the immutable `SCANNER_IMAGE=...` tag

The hook infers the image name from the GitHub `origin` remote. Override it when needed:

```bash
GHCR_IMAGE=ghcr.io/<owner>/<repo> git commit
```

If GHCR auth is missing, log in first:

```bash
docker login ghcr.io
```

Use a GitHub token with `write:packages`.

For local-only commits that must not publish an image:

```bash
ARB_SKIP_GHCR_PUBLISH=1 git commit
```

## Server Setup

1. Copy `.env.example` to `.env`.
2. Fill in RPC URLs and scanner env vars.
3. Copy `SCANNER_IMAGE=...` from local `scanner-image.env` into the server `.env`.

Example:

```bash
SCANNER_IMAGE=ghcr.io/wanglz111/arb-arbitrage:tree-<staged-tree-hash>
```

4. Start or refresh the scanner:

```bash
docker compose up -d
```

Because `docker-compose.yml` uses `pull_policy: always`, re-running `docker compose up -d` refreshes the configured image before restart.

## Notes

- The Docker image only contains the Rust scanner binary.
- The local `.env` file is excluded from git and Docker build context.
- `scanner-image.env` is generated locally and ignored by git.
- `docker-compose.yml` mounts `./data` into `/app/data` so quiet-mode directional JSONL survives container restarts.
- The staged tree hash is used because `pre-commit` runs before a final commit SHA exists.
