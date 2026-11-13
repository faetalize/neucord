#!/bin/bash
# Builds a scratch copy of this checkout pointed at the loopback fake, then
# runs one scenario in its own network namespace and D-Bus session, with a
# headless compositor. Everything lands under target/fake-discord.
#
#   tools/fake-discord/run.sh <scenario> [run-name]
#
# Environment: FAKE_VARIANTS, FAKE_REIDENTIFY, FAKE_NEW_TOKEN,
# FAKE_INTERACTION, FAKE_MODAL_SUBMIT, FAKE_NSFW (see fake_discord.py), ACCOUNT (a name;
# runs that share it share the app's data dirs, as a relaunch does), E2E_TOKEN.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
export E2E=$ROOT/target/fake-discord
export SCENARIO=$1 RUN=${2:-$1} ACCOUNT=${ACCOUNT:-default} HERE
export FAKE_VARIANTS=${FAKE_VARIANTS:-} FAKE_REIDENTIFY=${FAKE_REIDENTIFY:-} FAKE_NEW_TOKEN=${FAKE_NEW_TOKEN:-} E2E_TOKEN=${E2E_TOKEN:-}
export FAKE_INTERACTION=${FAKE_INTERACTION:-} FAKE_MODAL_SUBMIT=${FAKE_MODAL_SUBMIT:-} FAKE_NSFW=${FAKE_NSFW:-}
[ -f "$HERE/scenarios/$SCENARIO.sh" ] || { echo "no scenario $SCENARIO" >&2; exit 2; }
mkdir -p "$E2E/src"
rsync -a --delete --exclude target --exclude .git "$ROOT/" "$E2E/src/"
python3 "$HERE/patch.py" "$E2E/src"
(cd "$E2E/src" && CARGO_TARGET_DIR="$E2E/build" cargo build 2>&1 | tail -3)
exec unshare -rn sh -c 'exec dbus-run-session -- "$HERE/session.sh"'
