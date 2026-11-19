# The account panel as the client's: our avatar with the status at its
# corner, our name and status as wide as they are, the settings button at
# the end; our profile popout from it, with the status and Switch Accounts,
# and each of their menus.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "shot $OUT/panel.png"
cmd "own-profile"
sleep 2
cmd "shot-profile $OUT/popout.png"
cmd "own-row 1 0 $OUT/accounts.png"
sleep 2
cmd "own-profile"
sleep 2
cmd "own-row 0 0 $OUT/status.png"
sleep 2
cmd "quit"
sleep 1
