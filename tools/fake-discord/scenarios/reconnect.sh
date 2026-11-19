# Reconnecting as the client does (#219): waits for WANT_CONNECTS gateway connections (default 3, up to
# 150 s) and writes `timeline.txt`, one line per connect, IDENTIFY and RESUME with its seconds since the
# first. Drive the first connection with NEUCORD_DEBUG_DROP=<s> (a drop that is resumed after 1-3 s),
# FAKE_RESUME=invalid|invalid-resumable (op 9 answering the RESUME), FAKE_RECONNECT=<s> (op 7, answered at
# once), FAKE_CLOSE=<code>[:<s>] (4010 reconnects, 4004 logs out) and FAKE_NO_HELLO=1 (the first connection
# is silent: Neucord gives up on it after 30 s).
want=${WANT_CONNECTS:-3}
for i in $(seq 300); do
  [ "$(grep -c '"kind": "connect"' $FAKE_LOG)" -ge "$want" ] && break
  sleep 0.5
done
sleep 4
python3 - $FAKE_LOG > $OUT/timeline.txt <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.startswith("{")]
rows = [r for r in rows if r.get("kind") in ("connect", "identify", "resume")]
start = rows[0]["at"] if rows else 0
for r in rows:
    print(f'{r["at"] - start:7.2f}s  {r["kind"]}')
PY
cmd "quit"
sleep 1
