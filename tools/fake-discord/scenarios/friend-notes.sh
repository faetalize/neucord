# Friend request messages (#184), with FAKE_FRIEND_NOTES=1 (Bob's request carries a message, and the account is an
# adult): the Pending tab shows "View Request" for it, which opens the message in place of the name's line; the Add
# Friend tab has the message field with its count, the hint under it, and sends `note` with the request (trimmed,
# line breaks as spaces); the field keeps to 120 UTF-16 units.
waitfor identify 1
sleep 3
cmd "friends pending"
sleep 2
cmd "shot $OUT/pending-closed.png"
cmd "view-request"
sleep 1
cmd "shot $OUT/pending-open.png"
cmd "friends add"
sleep 1
cmd "friend-entry some_user"
cmd "friend-note  Hello there  "
cmd "friend-state"
cmd "shot $OUT/add-form.png"
cmd "friend-send"
waitreq POST /api/v9/users/@me/relationships 1
sleep 1
cmd "friend-state"
cmd "shot $OUT/add-sent.png"
# The field holds 120 units: 130 typed leaves 120 and none left; an emoji is two units.
cmd "friend-note $(printf 'a%.0s' $(seq 130))"
cmd "friend-state"
cmd "friend-note $(printf '😀%.0s' $(seq 61))"
cmd "friend-state"
cmd "quit"
sleep 1
