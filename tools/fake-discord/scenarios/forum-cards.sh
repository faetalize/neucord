# Post cards with FAKE_FORUM=list or grid: the post made since we read the
# forum says New, every post offers the forum's reaction, and a video shows
# its still.
waitfor identify 1
sleep 3
cmd "channel 605"
waitreq POST /api/v9/channels/605/post-data 1
sleep 2
cmd "shot $OUT/cards.png"
cmd "quit"
sleep 1
