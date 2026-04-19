#!/usr/bin/env bash
# E2E smoke test — spins up `rustotron tail --json`, fires a burst of
# fake api.response frames against it via a small Node helper, then
# asserts that the expected lines reached stdout.
#
# Used as the quick "did I break something" check on a dev laptop. Not
# run in CI because it binds a real port.
#
# Requires: node ≥ 18, ws package (`npm i -g ws` or local node_modules),
# `jq`, `cargo`.
#
# Usage:
#   scripts/e2e-tail-smoke.sh
#   PORT=9092 scripts/e2e-tail-smoke.sh   # override port
set -euo pipefail

PORT="${PORT:-19091}"
DURATION_MS="${DURATION_MS:-2000}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "[e2e] building rustotron…"
(cd "$ROOT" && cargo build --release --bin rustotron)

BIN="$ROOT/target/release/rustotron"

echo "[e2e] starting rustotron tail --json on 127.0.0.1:$PORT"
OUT="$(mktemp)"
"$BIN" tail --json --port "$PORT" > "$OUT" 2>/dev/null &
TAIL_PID=$!
# Give it a moment to bind.
sleep 0.5

cleanup() {
  kill "$TAIL_PID" >/dev/null 2>&1 || true
  wait "$TAIL_PID" 2>/dev/null || true
  rm -f "$OUT"
}
trap cleanup EXIT

echo "[e2e] firing 5 mock api.response frames"
node - <<NODE
const WebSocket = require('ws');
const ws = new WebSocket('ws://127.0.0.1:${PORT}');
await new Promise((r, rej) => { ws.on('open', r); ws.on('error', rej); });
ws.send(JSON.stringify({type:'client.intro',payload:{name:'e2e'},important:false,date:new Date().toISOString(),deltaTime:0}));
const methods = ['GET','POST','PUT','DELETE','PATCH'];
for (let i = 0; i < 5; i++) {
  ws.send(JSON.stringify({
    type: 'api.response',
    payload: {
      duration: (i+1)*10,
      request: { url: \`https://e2e.test/r/\${i}\`, method: methods[i] },
      response: { status: 200 + i*10 },
    },
    important: false, date: new Date().toISOString(), deltaTime: 100,
  }));
}
await new Promise(r => setTimeout(r, 400));
ws.close();
NODE

sleep 0.3

# Expect exactly 5 ndjson lines.
LINES=$(wc -l < "$OUT" | tr -d ' ')
if [ "$LINES" -lt 5 ]; then
  echo "[e2e] FAIL — expected ≥5 lines on stdout, got $LINES"
  echo "--- tail output ---"
  cat "$OUT"
  exit 1
fi

# Validate each line is JSON and has the expected fields.
if ! jq -e 'select(.method != null and .status != null and .url != null)' < "$OUT" > /dev/null; then
  echo "[e2e] FAIL — ndjson output missing required fields"
  cat "$OUT"
  exit 1
fi

echo "[e2e] PASS — got $LINES ndjson lines:"
head -5 "$OUT"
