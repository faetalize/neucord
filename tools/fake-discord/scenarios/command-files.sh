# A command's file option (#290): an image pasted with /upload in the box
# goes to its file option, uploads at once, and the run names it
# (`{type: 11, value: 0}` and `attachments`, in fake.log).
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "clip-image"
cmd "type /upl"
sleep 1
cmd "key Return"
cmd "line"
cmd "paste"
waitreq POST /api/v9/channels/600/attachments 1
sleep 1
cmd "line"
cmd "shot $OUT/staged.png"
cmd "key Return"
waitreq POST /api/v9/interactions 1
sleep 1
cmd "quit"
sleep 1
