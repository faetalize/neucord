# Emoji packs in the picker (#268), with FAKE_NITRO=1 FAKE_EMOJI_PACKS=1 FAKE_EMOJI_IMAGES=1 FAKE_VARIANTS=packs=1: opening the
# picker asks `GET /emoji-packs` once; the packs page lists both packs (the one that is gone can't be pinned);
# pinning "Blobs" (its button) saves PinnedEmojiPacks (field 14 of settings-proto 2), and the picker then heads its custom
# emoji with a "Blobs" section.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "emoji-picker"
waitreq GET /api/v9/emoji-packs 1
sleep 2
cmd "emoji-packs"
sleep 2
cmd "packs-shot $OUT/packs-page.png"
cmd "packs-pin 0"
waitreq PATCH /api/v9/users/@me/settings-proto/2 1
sleep 1
cmd "packs-shot $OUT/packs-pinned.png"
cmd "packs-close"
sleep 1
cmd "emoji-picker"
sleep 2
cmd "shot-popover emoji_button $OUT/packs-picker.png"
cmd "quit"
sleep 1
