#!/usr/bin/env sh
set -eu

# scripts/cell-update.sh [ref]
#
# Updates a self-hosted HiveMind cell (docker-compose.yml) to a given git
# ref by building the server image from source and tagging it with the
# commit sha — never `:latest`, which only moves on a tagged release (see
# .github/workflows/release.yml) and can silently lag master for weeks
# (hivemind-zdsh.7). Health-checks the new container before declaring
# success and prints the sha `/v1/version` reports back, so a caller can
# confirm the cell is actually running what it was just built from.
#
# On failure the previous container is left running — this script never
# tears down a healthy cell to replace it with an unhealthy one — and the
# previously-tagged image is still on disk, so a manual rollback is just
# `HIVEMIND_IMAGE=hivemind:<previous sha> docker compose up -d --no-deps hivemind`.
#
# NOT wired to run automatically on every merge: that's an explicit
# non-goal (see hivemind-zdsh.7's DO/NOT). This is the mechanism a
# scheduled dog order or a human invokes deliberately.

ref="${1:-origin/master}"
compose_file="${HIVEMIND_COMPOSE_FILE:-docker-compose.yml}"
port="${HIVEMIND_PORT:-8080}"
health_url="${HIVEMIND_HEALTH_URL:-http://localhost:${port}/v1/health}"
version_url="${HIVEMIND_VERSION_URL:-http://localhost:${port}/v1/version}"
health_retries="${HIVEMIND_HEALTH_RETRIES:-30}"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

case "$ref" in
  origin/*) git fetch origin --quiet ;;
esac

full_sha="$(git rev-parse "$ref")"
short_sha="$(git rev-parse --short=12 "$ref")"
image="hivemind:${short_sha}"

echo "Building ${image} from ${ref} (${full_sha})..."
docker build --build-arg "GIT_SHA=${full_sha}" -t "$image" -f Dockerfile "$repo_root"

echo "Restarting cell with ${image}..."
HIVEMIND_IMAGE="$image" docker compose -f "$compose_file" up -d --no-deps hivemind

echo "Waiting for ${health_url} to report healthy..."
attempt=0
until curl -fsS "$health_url" >/dev/null 2>&1; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge "$health_retries" ]; then
    echo "cell did not become healthy after ${health_retries} attempts." >&2
    echo "leaving the container running for inspection — it was NOT rolled back." >&2
    echo "check logs with: docker compose -f ${compose_file} logs hivemind" >&2
    exit 1
  fi
  sleep 2
done

deployed_version="$(curl -fsS "$version_url")"
echo "Cell healthy."
echo "  built:    ${short_sha}"
echo "  deployed: ${deployed_version}"

case "$deployed_version" in
  *"$short_sha"*) : ;;
  *)
    echo "WARNING: /v1/version does not mention ${short_sha} — the running" >&2
    echo "container may not be the one just built. Investigate before trusting it." >&2
    exit 1
    ;;
esac
