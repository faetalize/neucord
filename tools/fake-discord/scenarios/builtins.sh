# The built-in commands (#292), run with FAKE_OWNER=1 so the moderation ones
# show: /shrug as the box's message, /nick, /ban and /timeout (their bodies
# and X-Audit-Log-Reason are in fake.log), and Clyde's lines in the shot.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "type /shrug "
sleep 1
cmd "line"
cmd "type hi"
cmd "key Return"
waitreq POST /api/v9/channels/600/messages 1
cmd "type /nick "
sleep 1
cmd "type Fakey"
cmd "key Return"
waitreq PATCH /api/v9/guilds/500/members/@me/nick 1
cmd "type /ban "
sleep 1
cmd "type bob"
sleep 1
cmd "pick 0"
cmd "line"
cmd "type Prev"
sleep 1
cmd "pick 0"
cmd "line"
cmd "key Return"
waitreq PUT /api/v9/guilds/500/bans/302 1
cmd "type /timeout "
sleep 1
cmd "type bob"
sleep 1
cmd "pick 0"
cmd "type 60"
sleep 1
cmd "pick 0"
cmd "line"
cmd "key Return"
waitreq PATCH /api/v9/guilds/500/members/302 1
sleep 1
cmd "end"
sleep 1
cmd "shot $OUT/clyde.png"
cmd "quit"
sleep 1
