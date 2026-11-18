# Record Voice Message in the attach menu while a recording elsewhere is
# being finished (#266): a made-up draft in #general is put in the state of
# one just stopped, and the menu of another channel shows the item greyed.
# Run with FAKE_VARIANTS=voice=1 E2E_NO_PLAYER=1: the experiment offers
# recording, and the draft's player would keep the shot from
# finishing.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "voice-draft 3"
cmd "voice-finishing"
cmd "channel 201"
sleep 2
cmd "attach"
sleep 1
cmd "shot-popover composer_box $OUT/attach-finishing.png"
cmd "quit"
sleep 1
