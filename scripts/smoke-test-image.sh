#!/usr/bin/env bash
# Starts the container image the way the README tells operators to, and checks
# that it serves the UI, /metrics and the git endpoint as a non-root user with
# its default config written to the /data volume.
#
# Usage: scripts/smoke-test-image.sh <image>
# Set CONTAINER_ENGINE=podman to use podman instead of docker.
set -euo pipefail

image=${1:?usage: $0 <image>}
engine=${CONTAINER_ENGINE:-docker}
name=git-archiver-smoke-$$
volume=$name-data
port=${SMOKE_PORT:-18080}
base=http://127.0.0.1:$port
work=$(mktemp -d)

cleanup() {
  status=$?
  if [ "$status" -ne 0 ]; then
    echo "--- container logs ---" >&2
    "$engine" logs "$name" >&2 || true
  fi
  "$engine" rm -f "$name" >/dev/null 2>&1 || true
  "$engine" volume rm -f "$volume" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

"$engine" volume create "$volume" >/dev/null
"$engine" run -d --name "$name" -v "$volume:/data" -p "127.0.0.1:$port:8080" "$image" >/dev/null

for _ in $(seq 1 50); do
  curl -fsS "$base/" -o "$work/list.html" 2>/dev/null && break
  sleep 0.2
done
grep -q "Git Archiver" "$work/list.html" || fail "the UI isn't served on port 8080"
echo "ok: UI served on 8080"

curl -fsS "$base/metrics" | grep -q "^# TYPE archiver_" || fail "/metrics isn't served"
echo "ok: /metrics served"

"$engine" exec "$name" test -f /data/config.toml || fail "no default config in /data"
echo "ok: default config written to /data/config.toml"

# shellcheck disable=SC2016 # awk and the container shell expand these, not this one
uid=$("$engine" exec "$name" awk '/^Uid:/ { print $2 }' /proc/1/status)
[ "$uid" != 0 ] || fail "the app runs as root"
echo "ok: the app runs as uid $uid"

# An archive is a plain bare repo under /data/repos; put one there by hand and
# clone it through the git endpoint, which needs `git http-backend` in the image.
# shellcheck disable=SC2016
"$engine" exec "$name" sh -euc '
  archive=/data/repos/example.com/smoke/test.git
  git init -q --bare "$archive"
  git init -q /tmp/work
  git -C /tmp/work -c user.name=smoke -c user.email=smoke@example.com \
    commit -q --allow-empty -m "smoke test"
  git -C /tmp/work push -q "$archive" HEAD:refs/heads/main
  git -C "$archive" symbolic-ref HEAD refs/heads/main
'
git clone -q "$base/git/example.com/smoke/test.git" "$work/clone" || fail "cloning from the git endpoint failed"
[ "$(git -C "$work/clone" log -1 --format=%s)" = "smoke test" ] || fail "the clone has the wrong history"
echo "ok: archive cloned over the git endpoint"

echo "all smoke tests passed"
