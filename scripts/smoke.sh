#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

port=18096
base="http://127.0.0.1:${port}"
workdir="$(mktemp -d)"
server_pid=""

cleanup() {
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$workdir"
}
trap cleanup EXIT

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "error: ffmpeg is required for the smoke test" >&2
  exit 1
fi

media="${workdir}/media"
mkdir -p "$media"

echo "== generating sample media in ${media} =="
for n in one two; do
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc=size=320x240:rate=15" -t 2 -pix_fmt yuv420p \
    "${media}/sample-${n}.mp4"
  echo "  created sample-${n}.mp4"
done

cat > "${workdir}/flex.toml" <<EOF
host = "127.0.0.1"
port = ${port}
database = "${workdir}/flex.db"

[[libraries]]
name = "Smoke"
path = "${media}"
kind = "movies"

extensions = ["mp4"]
EOF

echo "== building flex-server =="
cargo build -p flex-server

echo "== starting flex-server on port ${port} =="
target/debug/flex-server --config "${workdir}/flex.toml" >"${workdir}/server.log" 2>&1 &
server_pid=$!

ready=""
for _ in $(seq 1 60); do
  if curl -sf "${base}/api/v1/health" >/dev/null 2>&1; then
    ready=1
    break
  fi
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "error: server exited early; log follows:" >&2
    cat "${workdir}/server.log" >&2
    exit 1
  fi
  sleep 1
done

if [[ -z "$ready" ]]; then
  echo "error: server did not become healthy in time; log follows:" >&2
  cat "${workdir}/server.log" >&2
  exit 1
fi

echo
echo "== GET /api/v1/health =="
curl -sf "${base}/api/v1/health"; echo

echo
echo "== POST /api/v1/scan =="
curl -sf -X POST "${base}/api/v1/scan"; echo

echo
echo "== GET /api/v1/items =="
items="$(curl -sf "${base}/api/v1/items")"
echo "$items"

first_id="$(printf '%s' "$items" \
  | python3 -c 'import sys, json; print(json.load(sys.stdin)["items"][0]["id"])' 2>/dev/null \
  || true)"
first_id="${first_id:-1}"

echo
echo "== GET /api/v1/items/${first_id}/stream (Range: bytes=0-99) =="
curl -s -D - -o /dev/null -H "Range: bytes=0-99" \
  "${base}/api/v1/items/${first_id}/stream" | sed -n '1,15p'

echo
echo "== smoke test complete =="
