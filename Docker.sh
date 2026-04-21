#!/usr/bin/env bash
set -euo pipefail

# Run the full CI suite inside Docker.

IMAGE="phase4-build"
docker build -t "$IMAGE" -f Dockerfile .

docker run --rm \
  -v "$(pwd)":/workspace \
  -v cargo-registry:/usr/local/cargo/registry \
  -v cargo-git:/usr/local/cargo/git \
  "$IMAGE" bash -c "
    set -e
    cargo ci-fmt
    cargo ci-clippy
    cargo ci-check
    cargo ci-test
    cargo audit
    cargo deny check
    cargo build --release
  "
