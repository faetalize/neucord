# Playing a voice message's draft before sending it (#264): a made-up 6 s
# recording is the draft, which plays from the bar, counts down, can be
# dragged through and goes back to the start at its end. Read the player's
# lines in neucord.log; no shot, as a window shot with a media stream open
# hangs in this session's missing GL.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "voice-draft 6"
sleep 1
cmd "voice-state"
cmd "voice-play"
sleep 2
cmd "voice-state"
cmd "voice-play"
sleep 1
cmd "voice-state"
cmd "voice-scrub 0.5"
sleep 1
cmd "voice-state"
cmd "voice-play"
sleep 5
cmd "voice-state"
cmd "quit"
sleep 1
