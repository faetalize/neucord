# The channel list's colours and a category's row (FAKE_CATEGORY=1): read
# channels dimmed, #fresh-news unread and bold, the open one bright, and the
# category's name centred in its row with the gap above it.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 3
cmd "shot $OUT/sidebar.png"
cmd "quit"
sleep 1
