# Op 40 and op 41 (#193): READY is followed by op 41 naming the heartbeat session, then an op 40, and the
# interval's heartbeats are op 40 with `qos` (it waits for five). Run twice with one ACCOUNT, and the second
# IDENTIFY's `qos_token` holds the first READY's. Read the frames in fake.log.
waitfor identify 1
for i in $(seq 200); do [ "$(grep -c '"op": 40' $FAKE_LOG)" -ge 5 ] && break; sleep 0.5; done
cmd "quit"
sleep 1
