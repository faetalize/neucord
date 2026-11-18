# The pinned messages of #general (#226): the button opens a popover with the
# newest pin first, two to a page (`GET .../messages/pins?limit=25`), and
# Load more messages asks `before` the last one's time, as JavaScript writes
# it. fake.log has both requests.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "pins-open"
waitreq GET "/api/v9/channels/600/messages/pins?limit=25" 1
sleep 1
cmd "shot-popover pins_button $OUT/pins.png"
cmd "pins-more"
waitreq GET "/api/v9/channels/600/messages/pins?limit=25&before=2026-01-02T10%3A07%3A00.123Z" 1
sleep 1
cmd "shot-popover pins_button $OUT/pins-more.png"
cmd "quit"
sleep 1
