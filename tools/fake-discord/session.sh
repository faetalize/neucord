#!/bin/bash
# Runs inside `unshare -rn` and `dbus-run-session`: a fake Discord, a headless
# compositor and one Neucord, driven by the scenario named in $1.
set -u

OUT=$E2E/out/$RUN
BIN=${BIN:-$E2E/build/debug/neucord}
mkdir -p $OUT
export HOME=$E2E/home-$ACCOUNT
mkdir -p $HOME/data $HOME/state $HOME/config $HOME/cache
export XDG_DATA_HOME=$HOME/data XDG_STATE_HOME=$HOME/state XDG_CONFIG_HOME=$HOME/config XDG_CACHE_HOME=$HOME/cache
# In /tmp, not target/: a Wayland socket's path must fit in 108 bytes, and a deep checkout's doesn't.
export XDG_RUNTIME_DIR=$(mktemp -d /tmp/neucord-e2e.XXXXXX)
trap 'rm -rf "$XDG_RUNTIME_DIR"' EXIT
export FAKE_LOG=$OUT/fake.log E2E_CMD=$OUT/cmd
: > $FAKE_LOG; rm -f $E2E_CMD
ip link set lo up
python3 $HERE/fake_discord.py > $OUT/fake.out 2>&1 &
FAKE=$!
mutter --headless --wayland --no-x11 --virtual-monitor 1280x900 --wayland-display wayland-e2e > $OUT/mutter.log 2>&1 &
MUTTER=$!
for i in $(seq 50); do [ -S $XDG_RUNTIME_DIR/wayland-e2e ] && break; sleep 0.2; done
export WAYLAND_DISPLAY=wayland-e2e GDK_BACKEND=wayland
$BIN > $OUT/neucord.log 2>&1 &
APP=$!
cmd() {
  while [ -e $E2E_CMD ]; do sleep 0.1; done
  printf '%s\n' "$@" > $E2E_CMD.tmp && mv $E2E_CMD.tmp $E2E_CMD; sleep 1.0
}
waitfor() { # <kind> <count>
  for i in $(seq 100); do
    [ "$(grep -c "\"kind\": \"$1\"" $FAKE_LOG)" -ge "$2" ] && return 0; sleep 0.3
  done; echo "timeout waiting for $1 $2" >> $OUT/run.log; return 1
}
waitreq() { # <method> <path or its start> <count>
  for i in $(seq 100); do
    [ "$(grep -cF "\"method\": \"$1\", \"path\": \"$2" $FAKE_LOG)" -ge "$3" ] && return 0; sleep 0.3
  done; echo "timeout waiting for $1 $2 $3" >> $OUT/run.log; return 1
}
source $HERE/scenarios/$SCENARIO.sh
kill $APP $MUTTER $FAKE 2>/dev/null
wait 2>/dev/null
