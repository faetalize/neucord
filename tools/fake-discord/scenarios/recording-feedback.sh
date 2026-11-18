# What a recording shows near the limit and when another one starts (#263),
# in a made-up recording, there being no microphone: its clock blinks from
# 19:30, and starting a recording in another channel stops it and says so.
# Run with FAKE_VARIANTS=voice=1.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "voice-recording 1168"
cmd "voice-blink 8"
sleep 9
cmd "channel 201"
cmd "voice-recording-elsewhere 5"
cmd "voice-record"
sleep 1
cmd "voice-state"
cmd "shot $OUT/stopped.png"
cmd "quit"
sleep 1
