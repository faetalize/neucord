# #general with FAKE_ANSWERS=1: the line above apps' answers.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 3
cmd "shot $OUT/used-lines.png"
cmd "quit"
sleep 1
