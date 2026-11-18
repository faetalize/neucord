# The Activity Privacy page of the settings window (#235): opened, then
# "Share my activity" turned off (the notice over the list appears and the
# change is saved), then the default changed (the question about the servers).
waitfor identify 1
sleep 3
cmd "settings activity"
sleep 2
cmd "shot $OUT/page.png"
cmd "activity-share off"
sleep 2
cmd "shot $OUT/not-sharing.png"
cmd "activity-default 3"
sleep 2
cmd "shot $OUT/asked.png"
cmd "quit"
sleep 1
