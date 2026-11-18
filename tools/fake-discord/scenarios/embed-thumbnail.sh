# A card's thumbnail has the image menu (#256): the message menu opened on the
# small picture of an embed offers its media and link entries as it does on
# any other image. Needs FAKE_EMBED=1: line 37 of #general is a card with a
# GIF thumbnail.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 3
cmd "embed-menu $OUT/menu.png"
sleep 2
cmd "shot $OUT/card.png"
cmd "quit"
sleep 1
