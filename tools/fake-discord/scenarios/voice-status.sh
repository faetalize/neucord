# What is set on a voice channel shows under its name (#341). With
# FAKE_VOICE_STATUS="Movie night: Spirited Away" the answer to op 43 (the
# first draw of a voice channel asks, once; fake.log has the one frame) puts
# it under Lounge; with FAKE_VOICE_STATUS_LATER="Quiz time" it changes eight
# seconds after the first draw, and with "none" it goes.
waitfor identify 1
sleep 3
cmd "guild 500"
sleep 2
cmd "shot $OUT/status.png"
sleep 9
cmd "shot $OUT/later.png"
cmd "quit"
sleep 1
