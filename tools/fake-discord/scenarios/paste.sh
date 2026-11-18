# Pasting text and images into the message box (#260): a text within the
# message length goes into the box, a longer one becomes message.txt, and an
# image copied from a page is named after its <img>'s file (cat.png), else
# image.png (`filename` of each in the attachments request in fake.log).
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "clip-text 100"
cmd "paste"
sleep 1
cmd "box-length"
cmd "clip-text 2500"
cmd "paste"
waitreq POST /api/v9/channels/600/attachments 1
sleep 1
cmd "box-length"
cmd "clip-page-image https://example.com/a/cat.photo.jpg?x=1"
cmd "paste"
waitreq POST /api/v9/channels/600/attachments 2
sleep 1
cmd "clip-image"
cmd "paste"
waitreq POST /api/v9/channels/600/attachments 3
sleep 1
cmd "shot $OUT/pasted.png"
cmd "quit"
sleep 1
