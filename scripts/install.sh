#!/usr/bin/env bash
set -euo pipefail

# Builds a release binary from the current working tree and installs it to
# ~/.cargo/bin. The installed binary is identified by the short git hash
# embedded at compile time, so no version bump is required between formal
# releases. Intended for local dogfooding during active development.

BINARY="phase4"
INSTALL_DIR="${HOME}/.cargo/bin"

echo "Building ${BINARY} (release)"
cargo build --release

INSTALLED_PATH="${INSTALL_DIR}/${BINARY}"
cp "target/release/${BINARY}" "${INSTALLED_PATH}"

echo ""
echo "Installed to ${INSTALLED_PATH}"
echo ""
"${INSTALLED_PATH}" --version
