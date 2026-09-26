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
# The sha-tagged image is also pinned as HIVEMIND_IMAGE in the compose
# project's env file (default: `.env` next to the first compose file). That
# file is what anything else reconciling the cell with `docker compose up -d`
# — a systemd timer, a cron job, you next week — reads, so the update
# survives it instead of being recreated back onto `:latest`
# (hivemind-2mgj). The pin is written before the restart, so no reconciler
# tick can slip in between, and it is kept only if the new container comes
# up healthy: on any failure the env file is put back exactly as it was, and
# a reconciler rolls the cell back to the previous image.
#
# NOT wired to run automatically on every merge: that's an explicit
# non-goal (see hivemind-zdsh.7's DO/NOT). This is the mechanism a
# scheduled dog order or a human invokes deliberately.
#
# Environment:
#   HIVEMIND_COMPOSE_FILE  compose file(s), colon-separated like COMPOSE_FILE
#                          (default: docker-compose.yml). Pass every file the
#                          cell is reconciled with, override files included.
#   HIVEMIND_ENV_FILE      env file to pin HIVEMIND_IMAGE in and hand to
#                          compose (default: `.env` next to the first compose
#                          file, which is where compose looks by itself).

ref="${1:-origin/master}"
compose_files="${HIVEMIND_COMPOSE_FILE:-docker-compose.yml}"
first_compose_file="${compose_files%%:*}"
env_file="${HIVEMIND_ENV_FILE:-$(dirname "$first_compose_file")/.env}"
port="${HIVEMIND_PORT:-8080}"
health_url="${HIVEMIND_HEALTH_URL:-http://localhost:${port}/v1/health}"
version_url="${HIVEMIND_VERSION_URL:-http://localhost:${port}/v1/version}"
health_retries="${HIVEMIND_HEALTH_RETRIES:-30}"

# The positional parameters become the `-f` list from here on; $ref is read.
set --
old_ifs="$IFS"
IFS=:
for compose_file in $compose_files; do
  [ -n "$compose_file" ] || continue
  set -- "$@" -f "$compose_file"
done
IFS="$old_ifs"

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

case "$ref" in
  origin/*) git fetch origin --quiet ;;
esac

full_sha="$(git rev-parse "$ref")"
short_sha="$(git rev-parse --short=12 "$ref")"
image="hivemind:${short_sha}"

# The env file as it was before this run, so a failed update can put it back
# byte for byte; and whether it existed at all, so a file this script created
# is removed again rather than left behind empty.
env_backup="$(mktemp)"
env_new="$(mktemp)"
env_existed=0
[ ! -f "$env_file" ] || env_existed=1
pinned=0
verified=0

restore_env_file() {
  if [ "$env_existed" = 1 ]; then
    cat "$env_backup" > "$env_file"
  else
    rm -f "$env_file"
  fi
}

finish() {
  status=$?
  trap - EXIT
  if [ "$pinned" = 1 ] && [ "$verified" != 1 ]; then
    restore_env_file
    echo "restored ${env_file} to its state before this update." >&2
  fi
  rm -f "$env_backup" "$env_new"
  exit "$status"
}
trap finish EXIT
trap 'exit 1' HUP INT TERM

echo "Building ${image} from ${ref} (${full_sha})..."
docker build --build-arg "GIT_SHA=${full_sha}" -t "$image" -f Dockerfile "$repo_root"

echo "Pinning HIVEMIND_IMAGE=${image} in ${env_file}..."
[ "$env_existed" != 1 ] || cp "$env_file" "$env_backup"
awk -v line="HIVEMIND_IMAGE=${image}" '
  /^[ \t]*(export[ \t]+)?HIVEMIND_IMAGE[ \t]*=/ { if (!seen) print line; seen = 1; next }
  { print }
  END { if (!seen) print line }
' "$env_backup" > "$env_new"
pinned=1
# Written in place, not renamed over: an existing env file keeps its mode
# and owner (the umask only shapes a file created here, which holds the
# cell's secrets and so starts owner-only).
(umask 077 && cat "$env_new" > "$env_file")

echo "Restarting cell with ${image}..."
HIVEMIND_IMAGE="$image" docker compose "$@" --env-file "$env_file" up -d --no-deps hivemind

echo "Waiting for ${health_url} to report healthy..."
attempt=0
until curl -fsS "$health_url" >/dev/null 2>&1; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge "$health_retries" ]; then
    echo "cell did not become healthy after ${health_retries} attempts." >&2
    echo "leaving the container running for inspection — it was NOT rolled back;" >&2
    echo "anything reconciling with 'docker compose up -d' will move it back to the previous image." >&2
    echo "check logs with: docker compose $* --env-file ${env_file} logs hivemind" >&2
    exit 1
  fi
  sleep 2
done

deployed_version="$(curl -fsS "$version_url")"
echo "Cell healthy."
echo "  built:    ${short_sha}"
echo "  deployed: ${deployed_version}"

case "$deployed_version" in
  *"$short_sha"*) verified=1 ;;
  *)
    echo "WARNING: /v1/version does not mention ${short_sha} — the running" >&2
    echo "container may not be the one just built. Investigate before trusting it." >&2
    exit 1
    ;;
esac

echo "  pinned:   HIVEMIND_IMAGE=${image} in ${env_file}"
