# Searching every DM at once and `include_nsfw` (#285). Run with FAKE_NSFW="dms=1" (which puts the settings in READY,
# so the cog can change one). In a DM the search is of that DM (GET /channels/201/messages/search); the cog's
# Search all DMs saves `is_cross_dm_search_enabled` and asks the same words of every DM, as a POST to
# /users/@me/messages/search/tabs (fake.log has its body: one `messages` tab, `limit` 25, `offset`, and `channel_ids`
# for `in:`). In a server, `include_nsfw=true` is added to the query only after Continue on an age-restricted channel.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "search-open"
cmd "search-type line 3"
cmd "search-enter"
waitreq GET "/api/v9/guilds/500/messages/search" 1
sleep 1
cmd "search-open"
cmd "channel 604"
sleep 1
cmd "gate-press Continue"
waitreq GET "/api/v9/channels/604/messages" 1
sleep 1
cmd "search-open"
cmd "search-type line 3"
cmd "search-enter"
waitreq GET "/api/v9/guilds/500/messages/search" 2
sleep 1
cmd "search-open"
cmd "home"
cmd "channel 201"
sleep 2
cmd "search-open"
cmd "search-type dinner"
cmd "search-enter"
waitreq GET "/api/v9/channels/201/messages/search" 1
sleep 1
cmd "shot $OUT/one-dm.png"
cmd "search-dms on"
waitreq POST "/api/v9/users/@me/messages/search/tabs" 1
sleep 2
cmd "shot $OUT/all-dms.png"
cmd "search-type in:alice_fake dinner"
cmd "search-enter"
waitreq POST "/api/v9/users/@me/messages/search/tabs" 2
sleep 2
cmd "shot $OUT/in-alice.png"
cmd "search-dms off"
waitreq GET "/api/v9/channels/201/messages/search" 2
sleep 2
cmd "shot $OUT/back-to-one.png"
python3 - "$FAKE_LOG" > "$OUT/requests.txt" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    if not line.startswith("{"):
        continue
    r = json.loads(line)
    path = r.get("path", "")
    if r.get("kind") == "http" and ("messages/search" in path or "settings-proto" in path):
        print(r["method"], path, json.dumps(r.get("body", r.get("body_length")), sort_keys=True))
PY
cmd "quit"
sleep 1
