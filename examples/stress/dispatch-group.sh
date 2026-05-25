#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CLI="${DEPLOY_CLI:-$ROOT/server/target/release/deploy-cli}"

cd "$ROOT"
exec "$CLI" stress dispatch --kind validators \
  --meta role=validators \
  --meta nomad_group=1 \
  "$@"
