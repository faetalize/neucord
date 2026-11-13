# The message box's menu and send button: the button off by default, ticked
# in the menu, greyed while the box is empty, sending what Enter sends; Paste
# as Plain Text with a copied file, and Paste enabled for a copied image.
waitfor identify 1
sleep 3
cmd "channel 201"
sleep 2
cmd "composer-menu"
cmd "shot-popover composer $OUT/menu.png"
cmd "menu-item send-button"
cmd "send"
cmd "type hello"
cmd "shot $OUT/send-button.png"
cmd "send"
waitreq POST /api/v9/channels/201/messages 1
sleep 1
cmd "clip-files /tmp/neucord-e2e-note.txt"
cmd "composer-menu"
cmd "shot-popover composer $OUT/menu-files.png"
cmd "menu-item paste-plain-text"
sleep 1
cmd "shot $OUT/pasted-plain.png"
cmd "set "
cmd "clip-image"
cmd "composer-menu"
sleep 1
cmd "shot-popover composer $OUT/menu-image.png"
# Off again, as later runs on this account expect.
cmd "menu-item send-button"
cmd "quit"
sleep 1
