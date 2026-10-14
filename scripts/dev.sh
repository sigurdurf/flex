#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if [[ ! -f flex.toml ]]; then
  echo "flex.toml not found; copying config/flex.example.toml"
  cp config/flex.example.toml flex.toml
fi

exec cargo run -p flex-server -- --config flex.toml
