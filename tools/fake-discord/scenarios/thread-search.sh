# The thread browser's name search (#322): Enter asks, one request carrying
# `name` and `tag_setting=match_some` and nothing else; the results replace
# the lists; emptying the box brings the lists back.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "threads"
sleep 2
cmd "thread-rows"
cmd "thread-search plans"
waitreq GET /api/v9/channels/600/threads/search?name=plans 1
sleep 1
cmd "thread-rows"
cmd "thread-search"
sleep 1
cmd "thread-rows"
cmd "quit"
sleep 1
