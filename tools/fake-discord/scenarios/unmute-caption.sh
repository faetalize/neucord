# Unmute's caption (#245): run with FAKE_MUTED=1. The server's menu shows "Unmute Server" with
# "Muted until …" under it.
waitfor identify 1
sleep 3
cmd "guild 500"
sleep 2
cmd "guild-menu 500 $OUT/guild-menu.png"
sleep 2
cmd "quit"
sleep 1
