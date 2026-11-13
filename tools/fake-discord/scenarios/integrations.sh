# `/gif` and `/sticker` (#293): one search per query 300 ms after typing
# stops, each query asked once; Enter sends the first GIF through the box's
# own send, a click sends with the reply, Tab puts the link in the box
# (typing follows); Enter while loading sends nothing. `/sticker` fetches
# the packs as it is chosen and sends a pick alone. `results` prints the
# popup. Nothing goes to /interactions or /gifs/select.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "type /gi"
sleep 1
cmd "suggestions"
cmd "key Return"
cmd "line"
# Three changes within 300 ms: one GET, for "cat".
cmd "type c" "type a" "type t" "results"
waitreq GET /api/v9/gifs/search 1
sleep 1
cmd "results $OUT/gifs.png"
cmd "shot $OUT/gifs-window.png"
cmd "key Return"
waitreq POST /api/v9/channels/600/messages 1
cmd "line"
# While replying /gif is still offered; "cat" is answered, so nothing is asked.
cmd "reply"
cmd "type /gi"
sleep 1
cmd "suggestions"
cmd "key Return"
cmd "type cat"
sleep 1
cmd "results"
cmd "result-click 1"
waitreq POST /api/v9/channels/600/messages 2
cmd "line"
# Tab inserts the selected GIF's link as typed text, and typing starts.
cmd "type /gif "
sleep 1
cmd "type dog"
waitreq GET /api/v9/gifs/search 2
sleep 1
cmd "key Down"
cmd "results"
cmd "key Tab"
cmd "line"
waitreq POST /api/v9/channels/600/typing 1
cmd "set "
sleep 1
# Enter while the spinner shows sends nothing and keeps the command.
cmd "type /gif "
sleep 1
cmd "type owl" "results $OUT/loading.png" "key Return" "line"
waitreq GET /api/v9/gifs/search 3
sleep 1
cmd "results"
cmd "set "
sleep 1
# /sticker: the packs as it is chosen, then a pick sent alone.
cmd "type /sti"
sleep 1
cmd "key Return"
waitreq GET /api/v9/sticker-packs 1
cmd "line"
cmd "type wa"
sleep 1
cmd "results $OUT/stickers.png"
cmd "key Down"
cmd "key Return"
waitreq POST /api/v9/channels/600/messages 3
cmd "line"
sleep 1
cmd "quit"
sleep 1
