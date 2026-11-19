# The network going and coming (#219), as GIO reports it: while connected each
# sends a heartbeat at once (op 40 in fake.log), and while waiting to reconnect
# the network coming back connects at once instead of after the 1-3 s wait.
# Run with FAKE_CLOSE=4000:6, which closes the first connection 6 s in.
waitfor identify 1
sleep 2
cmd "network down"
sleep 1
cmd "network up"
sleep 1.9
cmd "network up"
sleep 4
python3 - $FAKE_LOG > $OUT/timeline.txt <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.startswith("{")]
rows = [r for r in rows if r.get("kind") in ("connect", "identify", "resume") or (r.get("kind") == "gateway" and r.get("op") == 40)]
start = rows[0]["at"] if rows else 0
for r in rows:
    print(f'{r["at"] - start:7.2f}s  {r["kind"]}{" op 40" if r.get("op") == 40 else ""}')
PY
cmd "quit"
sleep 1
