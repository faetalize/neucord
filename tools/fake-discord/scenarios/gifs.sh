# The GIF picker: what a pick reports (POST /gifs/select), and the GIF
# button's pulse after a favourite. Read fake.log with
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
# Favouriting pulses the GIF button, with "Added to Favorites" over it for
# the first two favourites and not the third. The 0.6 s blink is checked by eye.
cmd "gif"
cmd "gif-search cat"
sleep 2
cmd "gif-star 0"
cmd "gif-hint $OUT/hint-1.png"
sleep 2
cmd "gif-hint"
cmd "gif-star 1"
cmd "gif-hint"
sleep 2
cmd "gif-star 2"
cmd "gif-hint"
cmd "shot $OUT/hint-3-window.png"
sleep 2
# A favourite reports nothing: pick one from the Favorites tile.
cmd "gif-back"
sleep 1
cmd "gif-tile 0"
sleep 1
cmd "gif-pick 0"
sleep 2
cmd "quit"
sleep 1
