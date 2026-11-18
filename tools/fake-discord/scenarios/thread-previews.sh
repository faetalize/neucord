# The thread browser's rows (#323), with FAKE_THREADS=1: an active thread
# shows its newest message with who wrote it and when ("Alice Fake: see you
# there at noon"), one without says "No recent messages".
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "threads"
sleep 2
cmd "shot $OUT/threads.png"
cmd "quit"
sleep 1
