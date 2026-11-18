# The viewer's video (#255): Stats for Nerds over it, and View Details in the More menu.
# Run with FAKE_VIDEO=<a .webm>.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 3
cmd "forward-viewer made-up clip"
sleep 3
cmd "viewer-stats on"
sleep 1
cmd "viewer-shot $OUT/viewer-stats.png"
sleep 1
cmd "viewer-shot $OUT/viewer-stats.png"
cmd "viewer-more $OUT/viewer-details.png"
sleep 3
cmd "quit"
sleep 1
