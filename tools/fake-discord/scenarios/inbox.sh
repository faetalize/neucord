# The inbox's Mentions tab (#229): the button opens a popover with the first
# 25 mentions (`GET /users/@me/mentions?limit=25&roles=true&everyone=true`),
# the next page asks `before` the oldest one, and marking one done sends
# `DELETE /users/@me/mentions/<id>`. fake.log has the three requests.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "inbox-open"
waitreq GET "/api/v9/users/@me/mentions?limit=25&roles=true&everyone=true" 1
sleep 1
cmd "shot-popover inbox_button $OUT/inbox.png"
cmd "inbox-more"
waitreq GET "/api/v9/users/@me/mentions?before=" 1
sleep 1
cmd "inbox-done"
waitreq DELETE "/api/v9/users/@me/mentions/" 1
sleep 1
cmd "shot-popover inbox_button $OUT/inbox-done.png"
cmd "quit"
sleep 1
