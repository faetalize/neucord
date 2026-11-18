# The attach menu's icons (#192): the + button opens it with an icon on each
# entry (an upload, a thread, a poll, the apps, a schedule), and the Schedule row,
# which has a submenu, opens it.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "attach"
sleep 1
cmd "shot-popover attach_button $OUT/attach.png"
cmd "attach-click Schedule"
sleep 1
cmd "shot-popover attach_button $OUT/schedule.png"
cmd "quit"
sleep 1
