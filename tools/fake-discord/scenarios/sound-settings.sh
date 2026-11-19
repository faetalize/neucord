# The Sounds group of the Neucord settings page (#298): the new message switch
# under the master switch (off and fixed when sounds are off, kept in between),
# and under Focus Mode (off, fixed, with its reason). Run with FAKE_NSFW=dms=1,
# which puts the settings in READY.
waitfor identify 1
sleep 3
cmd "settings neucord"
sleep 2
cmd "sound-state New message"
cmd "shot $OUT/on.png"
cmd "sound-switch New message off"
cmd "sound-state New message"
cmd "sound-switch Notification sounds off"
cmd "sound-state New message"
cmd "shot $OUT/all-off.png"
cmd "sound-switch Notification sounds on"
cmd "sound-state New message"
cmd "focus-mode forever"
sleep 1
cmd "sound-state New message"
cmd "shot $OUT/focus.png"
cmd "focus-mode off"
sleep 1
cmd "sound-state New message"
cmd "quit"
sleep 1
