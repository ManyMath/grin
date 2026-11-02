#!/usr/bin/env bash
# Source this before `cargo update`, `cargo generate-lockfile`, or any other
# command that has to fetch the grin-wallet fork while it is not yet pushed
# to ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git.
#
# cargo keeps naming the cypherstack URL (that is what Cargo.lock records);
# git rewrites it to the local clone. Once Cargo.lock exists and the pinned
# rev is in ~/.cargo/git/db, ordinary builds need none of this. After the
# fork is pushed, this file is a no-op you can stop sourcing.
#
# Usage:  source /home/user/src/grin/tool/local-fork-env.sh
GRIN_WALLET_LOCAL=${GRIN_WALLET_LOCAL:-/home/user/src/grin-wallet}
export CARGO_NET_GIT_FETCH_WITH_CLI=true
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0="url.file://${GRIN_WALLET_LOCAL}.insteadOf"
export GIT_CONFIG_VALUE_0="ssh://git@git.cypherstack.com:222/sneurlax/grin-wallet.git"
# Hard guard: any attempt to really use ssh fails instead of connecting.
export GIT_SSH_COMMAND=/bin/false
