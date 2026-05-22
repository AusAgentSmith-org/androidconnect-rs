#!/usr/bin/env bash
set -euo pipefail

REGISTRY="repo.indexarr.net/indexarr"
IMAGE="$REGISTRY/androidconnect-rs-ci"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Building CI image: $IMAGE"
docker build -t "$IMAGE:latest" "$SCRIPT_DIR"

echo "Pushing $IMAGE:latest"
docker push "$IMAGE:latest"
echo "Done."
