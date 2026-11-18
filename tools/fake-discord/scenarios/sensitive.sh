# The sensitive media filter (#480): run with FAKE_SENSITIVE=1 FAKE_NSFW=allowed=0, so #general covers the pictures
# Discord's scan flagged and the ones it has not looked at yet, and leaves the clean one and the embed's text.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 3
cmd "shot $OUT/sensitive.png"
cmd "quit"
sleep 1
