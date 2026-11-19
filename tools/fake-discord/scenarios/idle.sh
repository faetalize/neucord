# Going idle (#232): run with NEUCORD_DEBUG_IDLE=4, so four seconds without input are idle and away. Op 3 goes out
# once when that passes, as {status: idle, since: <ms>, afk: true} (the account's status is Online), and again when a
# channel is opened, as {status: online, since: 0, afk: false}. `presence.txt` lists the op 3 frames of fake.log.
# Without it, a minute is away (a first frame of {status: online, since: 0, afk: true}, within 90 s): run with
# IDLE_STEPS=300 to wait that long.
waitfor identify 1
for i in $(seq ${IDLE_STEPS:-60}); do
  [ "$(grep -c '"op": 3,' $FAKE_LOG)" -ge 1 ] && break
  sleep 0.5
done
sleep 2
cmd "channel 600"
for i in $(seq 40); do
  [ "$(grep -c '"op": 3,' $FAKE_LOG)" -ge 2 ] && break
  sleep 0.5
done
sleep 2
grep '"op": 3,' $FAKE_LOG | python3 -c "
import json, sys
for line in sys.stdin:
    r = json.loads(line)
    print(r['d'])
" > $OUT/presence.txt
cmd "quit"
sleep 1
