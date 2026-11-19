# The composer card beside the account panel, the same height and on the same
# bottom edge, with who is typing above the card (FAKE_TYPING=1).
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 3
cmd "shot $OUT/bottom.png"
cmd "quit"
sleep 1
