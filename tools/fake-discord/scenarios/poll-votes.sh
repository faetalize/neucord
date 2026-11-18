# Who voted on a poll (#314), with FAKE_POLL=1: the poll's vote count opens
# the list. Its first answer has 130 voters: a page of 100 (`limit=100&type=2`),
# then, scrolled to the end, the next after the last id. A look at the empty
# answer too.
POLL=$(python3 -c "print(((1767225600000 - 1420070400000 + 1000 + 60 * 100) << 22) | 60)")
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "shot $OUT/poll.png"
cmd "poll-votes $POLL 1"
waitreq GET /api/v9/channels/600/polls 1
sleep 1
cmd "shot $OUT/poll-votes.png"
cmd "poll-votes $POLL 3"
sleep 1
cmd "shot $OUT/poll-votes-empty.png"
cmd "quit"
sleep 1
