#!/usr/bin/env bash
# Runs packages/grin/test/regtest_test.dart against a private Grin chain:
# the node's usernet mode with its test miner, a grin-wallet CLI wallet that
# owns the mined coins and runs a foreign API listener, and a small SOCKS5
# proxy. The node and the CLI are built from the fork's own sources.
#
#   tool/regtest.sh [path-to-grin-wallet-fork-clone]   (default: /home/user/src/grin-wallet)
#
# Everything lives under $GRIN_REGTEST_DIR (default: a temp dir) and is torn
# down at exit. Needs cargo, python3, curl and a Dart SDK on PATH.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd -P)
fork=${1:-/home/user/src/grin-wallet}
work=${GRIN_REGTEST_DIR:-$(mktemp -d)}
export GIT_SSH_COMMAND=/bin/false
mkdir -p "$work/node" "$work/walletA"

echo "== building grin (node) and grin-wallet (cli) from $fork"
(cd "$fork/grin" && cargo build -q --release --bin grin --target-dir "$work/target-node")
(cd "$fork" && cargo build -q --release --bin grin-wallet --target-dir "$work/target-wallet")
node=$work/target-node/release/grin
cli=$work/target-wallet/release/grin-wallet

pids=()
cleanup() { for p in "${pids[@]:-}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

echo "== miner wallet and listener (foreign API on 127.0.0.1:23415)"
(cd "$work" && "$cli" --usernet -t "$work/walletA" -p regtest init >/dev/null 2>&1)
(cd "$work/walletA" && "$cli" --usernet -t "$work/walletA" -p regtest listen >"$work/walletA-listen.log" 2>&1) &
pids+=($!)

echo "== usernet node with the test miner (foreign API on 127.0.0.1:23413)"
(cd "$work/node" && "$node" --usernet server config >/dev/null)
sed -i.bak \
  -e "s|^db_root = .*|db_root = \"$work/node/chain_data\"|" \
  -e 's|^run_tui = true|run_tui = false|' \
  -e 's|^run_test_miner = false|run_test_miner = true|' \
  -e 's|^#test_miner_wallet_url = .*|test_miner_wallet_url = "http://127.0.0.1:23415"|' \
  -e 's|^log_to_stdout = true|log_to_stdout = false|' \
  -e "s|^log_file_path = .*|log_file_path = \"$work/node/grin-server.log\"|" \
  "$work/node/grin-server.toml"
(cd "$work/node" && "$node" --usernet server run >"$work/node/stdout.log" 2>&1) &
pids+=($!)

echo "== SOCKS5 proxy on 127.0.0.1:1080"
python3 -I "$root/tool/socks5_test_proxy.py" 1080 >"$work/socks5.log" 2>&1 &
pids+=($!)

tip() {
  curl -s -m 5 -X POST http://127.0.0.1:23413/v2/foreign -H 'content-type: application/json' \
    -d '{"jsonrpc":"2.0","method":"get_tip","id":1,"params":[]}' \
    | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["result"]["Ok"]["height"])' 2>/dev/null || echo 0
}
echo "== waiting for 20 mined blocks"
for _ in $(seq 1 120); do [ "$(tip)" -ge 20 ] && break; sleep 2; done
echo "tip: $(tip)"

echo "== regtest_test.dart"
cd "$root/packages/grin"
GRIN_REGTEST_NODE_URL=http://127.0.0.1:23413 \
GRIN_REGTEST_PEER_URL=http://127.0.0.1:23415 \
GRIN_REGTEST_CLI=$cli \
GRIN_REGTEST_CLI_DIR=$work/walletA \
GRIN_REGTEST_CLI_PASSWORD=regtest \
GRIN_REGTEST_SOCKS=127.0.0.1:1080 \
dart test test/regtest_test.dart
echo "proxy connections: $(grep -c CONNECT "$work/socks5.log" || true)"
