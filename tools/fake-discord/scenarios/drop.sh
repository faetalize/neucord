# Dropping files on the chat (#259): the drag's overlay, a plain drop adding
# the file to the box to be sent with a comment, and a drop with Shift held
# sending it at once on a message with no text (`content` of the POST in
# fake.log is empty, its `attachments` has the file).
python3 - "$OUT" <<'PY'
import struct, sys, zlib
out = sys.argv[1]
def chunk(t, d):
    c = struct.pack(">I", len(d)) + t + d
    return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
raw = b"".join(b"\x00" + bytes([200, 80, 40]) * 16 for _ in range(16))
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 16, 16, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")
for name in ("plain.png", "instant.png"):
    open(f"{out}/{name}", "wb").write(png)
PY
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "drop-overlay"
sleep 1
cmd "shot $OUT/overlay.png"
cmd "drop $OUT/plain.png"
waitreq POST /api/v9/channels/600/attachments 1
sleep 1
cmd "shot $OUT/drafted.png"
cmd "drop-shift $OUT/instant.png"
waitreq POST /api/v9/channels/600/attachments 2
waitreq POST /api/v9/channels/600/messages 1
sleep 2
cmd "shot $OUT/sent.png"
cmd "quit"
sleep 1
