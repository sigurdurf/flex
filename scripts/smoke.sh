#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

port=18096
base="http://127.0.0.1:${port}"
workdir="$(mktemp -d)"
server_pid=""
admin_pass="smoke-$(od -An -N8 -tx1 /dev/urandom | tr -d ' \n')"

cleanup() {
  if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$workdir"
}
trap cleanup EXIT

fail() {
  echo "error: $*" >&2
  exit 1
}

# Fetch just the HTTP status code for a request.
status() {
  curl -s -o /dev/null -w '%{http_code}' "$@"
}

if ! command -v ffmpeg >/dev/null 2>&1; then
  fail "ffmpeg is required for the smoke test"
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

bin="target/debug/flex-server"

echo "== creating admin user =="
FLEX_CONFIG="${workdir}/flex.toml" "$bin" \
  user add admin --admin --password "${admin_pass}" \
  || fail "failed to create admin user"

echo "== starting flex-server on port ${port} =="
"$bin" --config "${workdir}/flex.toml" >"${workdir}/server.log" 2>&1 &
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
echo "== unauthenticated GET /api/v1/items (expect 401) =="
code="$(status "${base}/api/v1/items")"
echo "HTTP ${code}"
[[ "$code" == "401" ]] || fail "expected 401 for unauthenticated request, got ${code}"

echo
echo "== POST /api/v1/auth/login =="
login="$(curl -sf -X POST "${base}/api/v1/auth/login" \
  -H 'Content-Type: application/json' \
  -d "{\"username\":\"admin\",\"password\":\"${admin_pass}\",\"device_name\":\"smoke\"}")" \
  || fail "login failed"
token="$(printf '%s' "$login" \
  | python3 -c 'import sys, json; print(json.load(sys.stdin)["token"])')"
[[ -n "$token" ]] || fail "login did not return a token"
echo "obtained device token"

auth=(-H "Authorization: Bearer ${token}")

echo
echo "== POST /api/v1/scan =="
curl -sf "${auth[@]}" -X POST "${base}/api/v1/scan"; echo

echo
echo "== authenticated GET /api/v1/items (expect 200) =="
code="$(status "${auth[@]}" "${base}/api/v1/items")"
echo "HTTP ${code}"
[[ "$code" == "200" ]] || fail "expected 200 for authenticated request, got ${code}"

items="$(curl -sf "${auth[@]}" "${base}/api/v1/items")"
echo "$items"

first_id="$(printf '%s' "$items" \
  | python3 -c 'import sys, json; print(json.load(sys.stdin)["items"][0]["id"])' 2>/dev/null \
  || true)"
first_id="${first_id:-1}"

echo
echo "== GET /api/v1/items/${first_id}/stream (Range: bytes=0-99) =="
curl -s -D - -o /dev/null "${auth[@]}" -H "Range: bytes=0-99" \
  "${base}/api/v1/items/${first_id}/stream" | sed -n '1,15p'

echo
echo "== smoke test complete =="
