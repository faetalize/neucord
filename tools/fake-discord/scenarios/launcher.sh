# The app launcher in #general: the home, search (case counts), Dicey's page, Send, a command filled in from
# the page and run from the box, and a DM's home. Check the runs' analytics_location, section_name and source
# in fake.log, and that nothing but the indexes, settings-proto/2 and app icons was asked for.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "apps-visible"
cmd "apps"
waitreq GET /api/v9/guilds/500/application-command-index 1
sleep 2
cmd "launcher-shot $OUT/home.png"
cmd "launcher-search ro"
cmd "launcher-rows"
cmd "launcher-shot $OUT/search-ro.png"
cmd "launcher-search Ro"
cmd "launcher-rows"
cmd "launcher-search "
cmd "launcher-row Dicey"
sleep 1
cmd "launcher-rows"
cmd "launcher-shot $OUT/dicey.png"
cmd "launcher-button Send settings show"
waitreq POST /api/v9/interactions 1
sleep 1
cmd "apps-visible"
cmd "apps"
sleep 1
cmd "launcher-row Dicey"
sleep 1
cmd "launcher-row settings set"
sleep 1
cmd "apps-visible"
cmd "line"
cmd "shot $OUT/filled-in.png"
cmd "key Return"
waitreq POST /api/v9/interactions 2
sleep 1
cmd "apps"
sleep 1
cmd "launcher-close"
cmd "apps"
sleep 1
cmd "apps"
sleep 1
cmd "apps-visible"
cmd "launcher-rows"
cmd "channel 201"
sleep 2
cmd "apps"
sleep 2
cmd "launcher-shot $OUT/dm.png"
cmd "launcher-rows"
cmd "quit"
sleep 1
