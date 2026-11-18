# A live stage in the sidebar (#341): its topic, its speakers, and the count
# of the audience. Run with FAKE_STAGE=1.
waitfor identify 1
sleep 3
cmd "guild 500"
sleep 2
cmd "shot $OUT/stage.png"
cmd "quit"
sleep 1
