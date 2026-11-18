# Focus Mode (#236): turned on for 4 s, it silences and then ends by itself
# (the log says on, then off); for ever it stays; the status menu shows it.
# Run with FAKE_NSFW=dms=1, which puts the settings in READY.
waitfor identify 1
sleep 3
cmd "focus-state"
cmd "focus-mode 4000"
cmd "focus-state"
sleep 6
cmd "focus-state"
cmd "focus-mode forever"
cmd "focus-state"
cmd "status-menu $OUT/menu.png"
sleep 1
cmd "quit"
sleep 1
