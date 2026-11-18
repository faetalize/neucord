# Forward (#254): Forward in a message's menu, and the box it opens. It lists the places we opened last (not where the
# message is from), a search finds more, a place is picked with a click and put back with another, at most five, and
# a word typed with it follows as a message of its own. A forward is a message with `message_reference` of type 1,
# and `forward_only` when one file goes alone, from the viewer.
waitfor identify 1
sleep 3
cmd "channel 201"
sleep 1
cmd "channel 604"
sleep 1
cmd "channel 600"
sleep 2
cmd "forward-menu line_5_of $OUT/menu.png"
sleep 2
cmd "forward-open the newest picture"
sleep 2
cmd "forward-rows"
cmd "forward-state"
cmd "shot $OUT/forward.png"
cmd "forward-search hidden"
sleep 1
cmd "forward-rows"
cmd "forward-search ideas"
sleep 1
cmd "forward-rows"
cmd "forward-search "
sleep 1
cmd "forward-pick 0"
sleep 1
cmd "forward-comment @silent look at this"
cmd "forward-state"
cmd "shot $OUT/forward-picked.png"
cmd "forward-send"
waitreq POST /api/v9/channels/604/messages 2
sleep 2
cmd "shot $OUT/forwarded.png"
cmd "channel 600"
sleep 2
cmd "forward-viewer the newest picture"
sleep 2
cmd "forward-viewer-click"
sleep 2
cmd "forward-rows"
cmd "forward-pick 1"
cmd "forward-state"
cmd "shot $OUT/viewer-forward.png"
cmd "forward-send"
waitreq POST /api/v9/channels/201/messages 1
sleep 2
cmd "quit"
sleep 1
