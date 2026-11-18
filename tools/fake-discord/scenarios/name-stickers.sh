# The `:name` list gives its rows to the stickers we may send (#270): ten
# emoji are asked for, the stickers that match come after under their own
# header, and picking one takes the `:name` out of the box and sends it
# alone (`sticker_ids` in the message request in fake.log).
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "type :wave"
sleep 1
cmd "suggestions $OUT/name-stickers.png"
cmd "pick 2"
waitreq POST /api/v9/channels/600/messages 1
sleep 1
cmd "box-length"
cmd "quit"
sleep 1
