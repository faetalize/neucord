# The emoji picker's server top-emoji section (#268), with FAKE_TOP_EMOJI=1:
# opening the picker asks `GET /guilds/500/top-emojis` once, and the section
# "Top Emoji in Test Server" heads the list, in rank order, without what
# nobody has.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "emoji-picker"
waitreq GET /api/v9/guilds/500/top-emojis 1
sleep 2
cmd "shot-popover emoji_button $OUT/top-emoji.png"
cmd "quit"
sleep 1
