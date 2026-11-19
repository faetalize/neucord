# Syncs of threads (with FAKE_CHAIN=1 FAKE_THREADS=1 FAKE_THREAD_SYNCS=4) fill in again
# only the box under the message that started the thread whose count moved:
# line 10's says "5 messages" after them, and no other row is touched.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 3
cmd "scroll 0.2"
sleep 12
cmd "shot $OUT/synced.png"
cmd "quit"
sleep 1
