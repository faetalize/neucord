# Which age-restricted commands `/` offers: run with FAKE_NSFW and
# FAKE_VARIANTS=nsfwlevel=1 as the case needs (see README). /spicy is nsfw.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 1
cmd "type /"
waitreq GET /api/v9/users/@me/application-command-index 1
sleep 1
cmd "suggestions"
cmd "set "
cmd "channel 604"
sleep 1
cmd "type /"
sleep 1
cmd "suggestions"
cmd "set "
cmd "home"
cmd "channel 203"
sleep 1
cmd "type /"
waitreq GET /api/v9/channels/203/application-command-index 1
sleep 1
cmd "suggestions"
cmd "quit"
sleep 1
