# An upload that is rate limited (#261): with FAKE_UPLOAD_FAIL=429 the first PUT is refused with a `retry_after`
# of 4 s, and the check of where the upload stands that follows waits for it. `timeline.txt` has each request to
# the upload with its seconds since the first: the check goes about 4 s after the refusal (not 0.5-5 s, the
# upload's own backoff), then the PUT again.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "clip-image"
cmd "paste"
for i in $(seq 100); do
  [ "$(grep -c '"path": "/api/v9/upload/' $FAKE_LOG)" -ge 3 ] && break
  sleep 0.3
done
sleep 2
python3 - $FAKE_LOG > $OUT/timeline.txt <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.startswith("{")]
rows = [r for r in rows if r.get("kind") == "http" and r.get("path", "").startswith("/api/v9/upload/")]
start = rows[0]["at"] if rows else 0
for r in rows:
    print(f'{r["at"] - start:6.2f}s  {r["method"]} {r["path"]}')
PY
cmd "quit"
sleep 1
