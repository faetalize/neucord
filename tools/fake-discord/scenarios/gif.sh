# A GIF in #general (needs FAKE_GIF=1) is decoded off the main loop and
# animated: its two frames, red then blue, show in turn across shots a
# second apart.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 4
for i in 1 2 3 4 5 6; do cmd "shot $OUT/gif$i.png"; done
cmd "quit"
sleep 1
