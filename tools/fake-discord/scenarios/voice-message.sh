# Pressing and dragging a voice message's waveform (#360): run with
# FAKE_VOICE=1. It is 8 s long; `vmsg` lines in neucord.log say
# where the player is.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 3
cmd "vmsg state"
cmd "vmsg scrub 0.5"
sleep 2
cmd "vmsg state"
cmd "vmsg play"
sleep 2
cmd "vmsg state"
cmd "vmsg pause"
cmd "vmsg scrub 0.25"
sleep 1
cmd "vmsg state"
cmd "vmsg play"
sleep 1
cmd "vmsg scrub 1"
sleep 1
cmd "vmsg state"
cmd "quit"
sleep 1
