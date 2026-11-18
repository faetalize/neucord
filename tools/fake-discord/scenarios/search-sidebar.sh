# Search is a sidebar (#284): it takes the member list's place, results open
# their message and leave it open, going to another place in the server keeps
# it, leaving the server (or the button, or Escape) closes it and the member
# list is back.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "search-state"
cmd "search-open"
sleep 1
cmd "search-state"
cmd "shot $OUT/search-empty.png"
cmd "search-type line 3"
cmd "search-enter"
waitreq GET "/api/v9/guilds/500/messages/search" 1
sleep 2
cmd "shot $OUT/search-results.png"
cmd "search-result 0"
sleep 2
cmd "search-state"
cmd "shot $OUT/search-opened.png"
cmd "channel 602"
sleep 1
cmd "search-state"
cmd "home"
sleep 1
cmd "search-state"
cmd "guild 500"
cmd "channel 600"
sleep 1
cmd "search-open"
sleep 1
cmd "search-open"
sleep 1
cmd "search-state"
cmd "shot $OUT/search-closed.png"
cmd "quit"
sleep 1
