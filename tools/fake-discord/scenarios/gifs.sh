# The GIF picker: what a pick reports (POST /gifs/select). Read fake.log with
# analyse.py: a select and its message are in flight together, so they may
# arrive in either order.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
# A typed search: {"id": "fake-gif-3", "q": "cat"}, then the numeric id as a number.
cmd "gif"
cmd "gif-search cat"
waitreq GET /api/v9/gifs/search 1
sleep 1
cmd "gif-pick 0"
waitreq POST /api/v9/gifs/select 1
cmd "gif"
cmd "gif-search cat"
sleep 2
cmd "gif-pick 1"
waitreq POST /api/v9/gifs/select 2
# Shift puts the link in the box, and reports the pick too.
cmd "gif"
cmd "gif-search cat"
sleep 2
cmd "gif-shift-pick 2"
waitreq POST /api/v9/gifs/select 3
cmd "set "
# Trending reports q "". Its tile is the front page's second, after the Favorites.
cmd "gif"
sleep 1
cmd "gif-tile 1"
sleep 1
cmd "gif-pick 0"
waitreq POST /api/v9/gifs/select 4
# A favourite reports nothing: star one, then pick it from the Favorites tile.
cmd "gif"
cmd "gif-search cat"
sleep 2
cmd "gif-star 0"
cmd "gif-back"
sleep 1
cmd "gif-tile 0"
sleep 1
cmd "gif-pick 0"
sleep 2
cmd "quit"
sleep 1
