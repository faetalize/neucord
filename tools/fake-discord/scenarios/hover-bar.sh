# The bar over a message (#271), with FAKE_HOVER=1 so every row shows it: the
# reactions used most (here the defaults, 100, laughing, sparkling heart), the
# add-reaction button, reply, and more. Clicking a quick reaction reacts
# (`PUT …/reactions/…/@me` with `location=Message Hover Bar`).
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "shot $OUT/hover-bar.png"
cmd "hover-react 0"
waitreq PUT /api/v9/channels/600/messages 1
sleep 1
cmd "quit"
sleep 1
