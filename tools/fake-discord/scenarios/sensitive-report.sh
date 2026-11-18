# Mark As Not Sensitive (#253): run with FAKE_SENSITIVE=1 FAKE_NSFW=allowed=0. The flagged picture is revealed, its menu
# offers the item (shot), and the confirmation sends one POST /attachments/report-false-positive, read in fake.log.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 3
cmd "reveal-covers"
sleep 2
cmd "mark-sensitive"
sleep 1
cmd "shot $OUT/mark-dialog.png"
cmd "confirm"
waitreq POST /api/v9/attachments/report-false-positive 1
sleep 1
cmd "shot $OUT/mark-toast.png"
cmd "embed-menu $OUT/mark-menu.png"
sleep 2
cmd "quit"
sleep 1
