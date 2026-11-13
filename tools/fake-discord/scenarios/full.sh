waitfor identify 1
sleep 3
cmd "home"
cmd "shot $OUT/dm-list.png"
cmd "channel 201"
sleep 1
cmd "attach"
cmd "shot-popover composer_box $OUT/attach-menu.png"
cmd "shot $OUT/dm-open.png"
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "shot $OUT/server-channel.png"
waitfor identify 2
sleep 2
cmd "quit"
sleep 1
