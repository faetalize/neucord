# The forum's post search (#331), with FAKE_FORUM=1: nothing is asked while
# typing goes on, 350 ms after it pauses one request asks `name` (and `tag`
# and `tag_setting` once tags filter), the posts found replace the list,
# and emptying the box brings it back.
waitfor identify 1
sleep 3
cmd "channel 605"
sleep 2
cmd "shot $OUT/forum.png"
cmd "forum-search pl"
cmd "forum-search pla"
cmd "forum-search plan"
sleep 1
cmd "shot $OUT/forum-searching.png"
waitreq GET /api/v9/channels/605/threads/search?name=plan 1
sleep 1
cmd "shot $OUT/forum-found.png"
cmd "forum-search nothing"
sleep 2
cmd "shot $OUT/forum-none.png"
cmd "forum-search"
sleep 1
cmd "shot $OUT/forum-back.png"
cmd "quit"
sleep 1
