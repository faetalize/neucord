# Member requests the client makes and Neucord had not (#282): a name typed
# after `from:` in the search box is one op 8 (`query` as typed, `limit` 10,
# `presences` true) once per change of the text, and `@here` in the message
# box subscribes the channel's first member range (op 37 with [[0,99]]) once
# the typing rests. fake.log has both frames.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "search-open"
sleep 1
cmd "search-type from:al"
sleep 1
cmd "search-type from:alic"
sleep 2
cmd "shot $OUT/search.png"
cmd "search-open"
sleep 1
cmd "members-closed"
cmd "channel 602"
sleep 1
cmd "type hey @here"
sleep 2
cmd "quit"
sleep 1
