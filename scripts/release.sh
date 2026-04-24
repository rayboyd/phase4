#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-}"

if [[ -z "$VERSION" ]]; then
    echo "Usage: $0 <version>"
    echo "  e.g. $0 0.0.1"
    echo "  e.g. $0 0.0.1-rc.1"
    exit 1
fi

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$ ]]; then
    echo "Error: invalid version format '$VERSION'"
    echo "Expected: <major>.<minor>.<patch> or <major>.<minor>.<patch>-rc.<n>"
    exit 1
fi

TAG="v${VERSION}"

BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [[ "$BRANCH" != "main" ]]; then
    echo "Error: must be on main (currently on '$BRANCH')"
    exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Error: working tree is dirty, commit or stash changes first"
    exit 1
fi

if git tag --list | grep -q "^${TAG}$"; then
    echo "Error: tag '$TAG' already exists"
    exit 1
fi

cargo set-version "${VERSION}"

git add Cargo.toml Cargo.lock
git commit -m "chore(release): bump version to ${VERSION}"
git tag "${TAG}"

echo ""
echo "Version bumped to ${VERSION} and tagged ${TAG}."
echo "Push with:"
echo ""
echo "  git push origin main && git push origin ${TAG}"
