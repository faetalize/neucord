# /roll-dice (#292), with FAKE_VARIANTS=dice=1: the bar above the box while
# the dice roll and once they land, the message they send, its link rolling
# again, and a channel switch mid-roll sending nothing (two POSTs in all).
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "type /roll-dice "
sleep 1
cmd "pick 0"
cmd "type 3"
cmd "key Tab"
cmd "pick 0"
cmd "pick 5"
cmd "line"
cmd "key Return"
cmd "shot $OUT/rolling.png"
waitreq POST /api/v9/channels/600/messages 1
cmd "shot $OUT/rolled.png"
sleep 3
cmd "end"
sleep 1
cmd "shot $OUT/roll-message.png"
cmd "link https://discord.com/channels/500/600/roll-dice/3d20"
waitreq POST /api/v9/channels/600/messages 2
sleep 3
cmd "end"
sleep 1
cmd "link https://discord.com/channels/500/600/roll-dice/3d20"
cmd "channel 603"
sleep 3
cmd "shot $OUT/switched.png"
cmd "quit"
sleep 1
