# The age gate (#275): opening #after-dark, which is age-restricted, shows a page instead of loading the channel, and
# no messages are asked for. Run with FAKE_NSFW="allowed=1" (the default: an adult, asked once: Continue, then the
# channel opens, and so does it again later), "allowed=0" (underage: only Go Back) or "allowed=1,status=1,gated=1"
# (the region asks to confirm an age Neucord can't: no Continue). Go Back returns to where we were.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "channel 604"
sleep 1
cmd "gate-state"
cmd "shot $OUT/gate.png"
if [ -n "${GATE_CONTINUE:-}" ]; then
  cmd "gate-press Continue"
  waitreq GET "/api/v9/channels/604/messages" 1
  sleep 1
  cmd "gate-state"
  cmd "shot $OUT/gate-continued.png"
  cmd "channel 600"
  sleep 1
  cmd "channel 604"
  sleep 1
  cmd "gate-state"
else
  cmd "gate-press Go Back"
  sleep 1
  cmd "gate-state"
  cmd "shot $OUT/gate-back.png"
fi
cmd "quit"
sleep 1
