# A message's image that can't be loaded (#251): run with FAKE_IMAGE_FAIL=1.
# The row keeps the image's box and shows the missing-image mark in it.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 3
cmd "shot $OUT/broken-image.png"
cmd "quit"
sleep 1
