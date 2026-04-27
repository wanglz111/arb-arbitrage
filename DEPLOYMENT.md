# Deployment

This repository is set up to build a scanner image on GitHub Actions and publish it to GHCR.

## Build Flow

- Push to `main` or `master`
- GitHub Actions runs:
  - `cargo clippy --manifest-path scanner/Cargo.toml -- -D warnings`
  - `cargo test --manifest-path scanner/Cargo.toml`
  - multi-arch Docker build
  - push to `ghcr.io/<owner>/<repo>:latest`

## Server Setup

1. Copy `.env.example` to `.env`.
2. Set `SCANNER_IMAGE=ghcr.io/<owner>/<repo>:latest`.
3. Fill in your RPC URLs and scanner env vars.
4. If the repository or package is private, run:

```bash
docker login ghcr.io
```

Use a GitHub token with at least `read:packages`.

5. Start the scanner:

```bash
docker compose up -d
```

Because `docker-compose.yml` uses `pull_policy: always`, re-running `docker compose up -d` will refresh the image before restart.

## Notes

- The Docker image only contains the Rust scanner binary.
- The local `.env` file is excluded from both git and Docker build context.
- This setup does not SSH-deploy into the server automatically. It prepares the image so the server can run it directly via Docker Compose.
