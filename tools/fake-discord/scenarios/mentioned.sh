# Messages that ping us stand out (FAKE_PING=1): line 36 pings us, line 34
# @everyone.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 3
cmd "shot $OUT/mentioned.png"
cmd "quit"
sleep 1
