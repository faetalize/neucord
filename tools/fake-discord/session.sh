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
# The compositor: `mutter --headless` by default; COMPOSITOR=gnome-shell is GNOME's whole shell, headless, and
# COMPOSITOR=niri runs niri (what `niri` is on this machine) nested in a headless mutter, with the stock
# configuration, and COMPOSITOR=host uses one that is running already. `cshot <png>` shoots what the compositor shows, windows and their frames included, where it can.
COMPOSITOR=${COMPOSITOR:-mutter}
case $COMPOSITOR in
  mutter)
    mutter --headless --wayland --no-x11 --virtual-monitor 1280x900 --wayland-display wayland-e2e > $OUT/compositor.log 2>&1 &
    COMP=$!
    SOCKET=wayland-e2e
    ;;
  gnome-shell)
    XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=wayland gnome-shell --headless --wayland --no-x11 --virtual-monitor 1280x900 \
      --wayland-display wayland-e2e > $OUT/compositor.log 2>&1 &
    COMP=$!
    SOCKET=wayland-e2e
    ;;
  niri)
    mutter --headless --wayland --no-x11 --virtual-monitor 1280x900 --wayland-display wayland-parent > $OUT/parent.log 2>&1 &
    PARENT=$!
    for i in $(seq 50); do [ -S $XDG_RUNTIME_DIR/wayland-parent ] && break; sleep 0.2; done
    mkdir -p $XDG_CONFIG_HOME/bento && echo 'hotkey-overlay { skip-at-startup; }' > $XDG_CONFIG_HOME/bento/config.kdl
    WAYLAND_DISPLAY=wayland-parent niri > $OUT/compositor.log 2>&1 &
    COMP=$!
    SOCKET=
    for i in $(seq 50); do
      SOCKET=$(cd $XDG_RUNTIME_DIR && ls -d wayland-[0-9]* 2>/dev/null | grep -v lock | head -1)
      [ -n "$SOCKET" ] && break; sleep 0.2
    done
    export NIRI_SOCKET=$(ls $XDG_RUNTIME_DIR/*.sock 2>/dev/null | head -1)
    ;;
  host)
    # A compositor started by hand (sway, KWin, the desktop you are on): HOST_DISPLAY is the path of its Wayland socket.
    # Only the D-Bus session is the run's own, so a notification goes to no server of yours.
    COMP=
    SOCKET=${HOST_DISPLAY:?COMPOSITOR=host needs HOST_DISPLAY, the path of a Wayland socket}
    ;;
  *) echo "no compositor $COMPOSITOR" >&2; exit 2 ;;
esac
for i in $(seq 50); do [ -S $XDG_RUNTIME_DIR/$SOCKET ] || [ -S "$SOCKET" ] && break; sleep 0.2; done
export WAYLAND_DISPLAY=$SOCKET GDK_BACKEND=wayland
cshot() {
  case $COMPOSITOR in
    niri|host) grim "$1" >> $OUT/run.log 2>&1 ;;
    gnome-shell) gdbus call --session -d org.gnome.Shell.Screenshot -o /org/gnome/Shell/Screenshot \
      -m org.gnome.Shell.Screenshot.Screenshot false false "$1" >> $OUT/run.log 2>&1 ;;
    *) echo "cshot: no screenshot of $COMPOSITOR" >> $OUT/run.log ;;
  esac
}
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
kill $APP $COMP ${PARENT:-} $FAKE 2>/dev/null
wait 2>/dev/null
