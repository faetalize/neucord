#!/usr/bin/env python3
"""A loopback Discord with made-up data: REST and a zlib-stream gateway.

Everything it is sent goes to $FAKE_LOG as JSON lines. $FAKE_VARIANTS names
which Apex experiments the account is assigned, e.g. "hiding=1,lazy=1".
"""
import base64, json, os, socket, struct, sys, threading, time, zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = 8099
LOG = open(os.environ["FAKE_LOG"], "a", buffering=1)
LOCK = threading.Lock()


def log(**kw):
    with LOCK:
        LOG.write(json.dumps(kw) + "\n")


def murmur3(key):
    data = key.encode()
    c1, c2, h = 0xCC9E2D51, 0x1B873593, 0
    n = len(data) // 4 * 4
    for i in range(0, n, 4):
        k = int.from_bytes(data[i : i + 4], "little")
        k = (k * c1) & 0xFFFFFFFF
        k = ((k << 15) | (k >> 17)) & 0xFFFFFFFF
        k = (k * c2) & 0xFFFFFFFF
        h ^= k
        h = ((h << 13) | (h >> 19)) & 0xFFFFFFFF
        h = (h * 5 + 0xE6546B64) & 0xFFFFFFFF
    tail = data[n:]
    if tail:
        k = 0
        for b in reversed(tail):
            k = (k << 8) | b
        k = (k * c1) & 0xFFFFFFFF
        k = ((k << 15) | (k >> 17)) & 0xFFFFFFFF
        k = (k * c2) & 0xFFFFFFFF
        h ^= k
    h ^= len(data)
    h ^= h >> 16
    h = (h * 0x85EBCA6B) & 0xFFFFFFFF
    h ^= h >> 13
    h = (h * 0xC2B2AE35) & 0xFFFFFFFF
    return h ^ (h >> 16)


assert murmur3("hello") == 0x248BFA47

VARIANTS = dict(p.split("=") for p in os.environ.get("FAKE_VARIANTS", "").split(",") if p)
NAMES = {
    "hiding": "2026-02-private-channel-hiding",
    "voice": "2026-09-desktop-voice-message-recording",
    "ladder": "2026-07-attachment-image-ladder",
    "lazy": "2026-02-lazy-load-all-images",
    "upload": "2026-09-nitro-file-upload-rollout",
    "chatbar": "2026-06-chat-bar-customization",
}
LADDER = json.dumps({"ladder": [64, 128, 256], "maxUpscale": 1.1, "minSnapDownDpr": 2})

ME = {"id": "100", "username": "tester_fake", "global_name": "Tester Fake", "avatar": None, "discriminator": "0",
      "premium_type": 0, "flags": 0, "verified": True, "mfa_enabled": False}
ALICE = {"id": "301", "username": "alice_fake", "global_name": "Alice Fake", "avatar": None, "discriminator": "0"}
BOB = {"id": "302", "username": "bob_fake", "global_name": "Bob Fake", "avatar": None, "discriminator": "0"}
VIEW, SEND, HISTORY, ATTACH, REACT, CONNECT = 1 << 10, 1 << 11, 1 << 16, 1 << 15, 1 << 6, 1 << 20


def snowflake(ms_after_epoch_2026, n):
    return str(((1767225600000 - 1420070400000 + ms_after_epoch_2026) << 22) | n)


def apex():
    entries = []
    for key, variant in VARIANTS.items():
        e = [murmur3(NAMES[key]), int(variant), 0, 1]
        if key == "ladder":
            e += [None, LADDER]
        entries.append(e)
    return {"assignments": {"1": {"100": {"assignments": entries, "evaluation_id": "e"}}}, "installation": "fake-install"}


def guild():
    channels = [
        {"id": "600", "type": 0, "name": "general", "position": 0, "parent_id": None, "permission_overwrites": [],
         "last_message_id": snowflake(5000, 40), "flags": 0},
        {"id": "601", "type": 0, "name": "hidden-plans", "position": 1, "parent_id": None, "flags": 1 << 17,
         "permission_overwrites": [{"id": "500", "type": 0, "allow": "0", "deny": str(VIEW)}]},
        {"id": "603", "type": 0, "name": "leaked-name", "position": 2, "parent_id": None, "flags": 1 << 17,
         "permission_overwrites": []},
        {"id": "602", "type": 2, "name": "Lounge", "position": 3, "parent_id": None, "bitrate": 64000,
         "permission_overwrites": [], "flags": 0},
    ]
    return {"id": "500", "properties": {"name": "Test Server", "owner_id": "999", "icon": None, "features": [],
            "premium_tier": 0, "default_message_notifications": 0}, "roles": [
        {"id": "500", "name": "@everyone", "permissions": str(VIEW | SEND | HISTORY | ATTACH | REACT | CONNECT),
         "position": 0}], "channels": channels, "emojis": [], "stickers": [], "threads": [], "member_count": 3,
        "data_mode": "full", "joined_at": "2026-01-01T00:00:00.000000+00:00", "large": False, "lazy": True}


def ready():
    return {
        "v": 9, "user": ME, "users": [ME, ALICE], "session_id": "sess-1", "resume_gateway_url": f"ws://127.0.0.1:{PORT}",
        "guilds": [guild()], "merged_members": [[{"user_id": "100", "roles": [], "flags": 0,
                                                  "joined_at": "2026-01-01T00:00:00.000000+00:00"}]],
        "private_channels": [{"id": "201", "type": 1, "last_message_id": snowflake(4000, 3), "recipient_ids": ["301"],
                              "flags": 0}],
        "relationships": [{"id": "301", "type": 1, "user_id": "301", "since": "2026-01-01T00:00:00.000000+00:00",
                           "nickname": None}],
        "read_state": {"entries": [
            {"id": "201", "last_message_id": snowflake(4000, 3), "mention_count": 0},
            {"id": "500", "read_state_type": 3, "last_message_id": "0", "mention_count": 7},
        ], "partial": False, "version": 1},
        "user_guild_settings": {"entries": [], "partial": False, "version": 1},
        "apex_experiments": apex(), "experiments": [], "guild_experiments": [],
        "auth_token": os.environ.get("FAKE_NEW_TOKEN") or None,
        "country_code": "ZZ", "analytics_token": "fake", "api_code_version": 0,
    }


def supplemental():
    return {
        "guilds": [{"id": "500", "voice_states": [{"user_id": "301", "channel_id": "602", "session_id": "v1",
                                                  "deaf": False, "mute": False, "self_deaf": False, "self_mute": False}]}],
        "merged_members": [[{"user_id": "301", "roles": [], "nick": "Al", "flags": 0}]],
        "merged_presences": {"friends": [{"user_id": "301", "status": "online", "activities": [],
                                          "client_status": {"desktop": "online"}}], "guilds": [[]]},
        "lazy_private_channels": [{"id": "202", "type": 1, "last_message_id": snowflake(3000, 2), "flags": 0,
                                   "recipients": [BOB]}],
    }


def message(n, author, content, attachments=()):
    return {"id": snowflake(1000 + n * 100, n), "channel_id": "600", "author": author, "content": content,
            "timestamp": "2026-01-01T10:%02d:00.000000+00:00" % n, "edited_timestamp": None, "tts": False,
            "mention_everyone": False, "mentions": [], "mention_roles": [], "attachments": list(attachments),
            "embeds": [], "pinned": False, "type": 0, "flags": 0}


def attachment(n):
    return {"id": str(900 + n), "filename": "pic%d.png" % n, "size": 4000, "width": 1200, "height": 800,
            "content_type": "image/png",
            "url": f"http://127.0.0.1:{PORT}/attachments/1/{900 + n}/pic{n}.png?ex=ffffffff&is=1&hm=2&",
            "proxy_url": f"http://127.0.0.1:{PORT}/attachments/1/{900 + n}/pic{n}.png?ex=ffffffff&is=1&hm=2&"}


def messages(channel):
    if channel != "600":
        return []
    out = []
    for n in range(40):
        who = ALICE if n % 2 else BOB
        text = "line %d of the made-up conversation" % n
        files = ()
        if n == 0:
            text, files = "the oldest picture", (attachment(0),)
        if n == 39:
            text, files = "the newest picture, and see <#603> for the plan", (attachment(39),)
        out.append(message(n, who, text, files))
    return list(reversed(out))


def png(w, h):
    raw = b"".join(b"\x00" + bytes([40, 120, 200]) * w for _ in range(h))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def frame(payload, opcode=2):
    n = len(payload)
    head = bytes([0x80 | opcode])
    if n < 126:
        head += bytes([n])
    elif n < 65536:
        head += bytes([126]) + struct.pack(">H", n)
    else:
        head += bytes([127]) + struct.pack(">Q", n)
    return head + payload


def read_exact(rfile, n):
    data = b""
    while len(data) < n:
        chunk = rfile.read(n - len(data))
        if not chunk:
            raise EOFError
        data += chunk
    return data


def read_frame(rfile):
    b1, b2 = read_exact(rfile, 2)
    opcode, n = b1 & 0x0F, b2 & 0x7F
    if n == 126:
        n = struct.unpack(">H", read_exact(rfile, 2))[0]
    elif n == 127:
        n = struct.unpack(">Q", read_exact(rfile, 8))[0]
    mask = read_exact(rfile, 4) if b2 & 0x80 else None
    data = read_exact(rfile, n)
    if mask:
        data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
    return opcode, data


IDENTIFIES = [0]


def gif(n, id=None):
    """A made-up GIF result; `id` replaces its string id, as an answer may carry a number."""
    return {"id": f"fake-gif-{n}" if id is None else id, "url": f"https://gifs.example/view/fake-{n}",
            "src": f"//127.0.0.1:{PORT}/attachments/gifs/{n}.png", "width": 300, "height": 200}


# What the GIF picker's endpoints answer, by path.
GIFS = {
    "/gifs/trending": lambda: {"categories": [{"name": "happy", "src": f"//127.0.0.1:{PORT}/attachments/gifs/0.png"}],
                               "gifs": [gif(1)]},
    "/gifs/trending-gifs": lambda: [gif(1), gif(2)],
    "/gifs/search": lambda: [gif(3), gif(4, id=4004), gif(5)],
    "/gifs/suggest": lambda: ["kitten", "cats"],
}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_GET(self):
        if self.headers.get("Upgrade", "").lower() == "websocket":
            return self.gateway()
        path = self.path
        if path.startswith("/attachments/"):
            query = dict(p.split("=", 1) for p in path.split("?", 1)[-1].split("&") if "=" in p)
            log(kind="image", path=path, width=query.get("width"), height=query.get("height"), at=time.time())
            body = png(int(query.get("width", 1200)) // 4, int(query.get("height", 800)) // 4)
            return self.send(200, body, "image/png")
        self.record()
        route = path.split("?")[0].removeprefix("/api/v9")
        if route in GIFS:
            return self.send(200, json.dumps(GIFS[route]()).encode())
        if "/messages" in path.split("?")[0]:
            channel = path.split("/channels/")[1].split("/")[0]
            return self.send(200, json.dumps(messages(channel)).encode())
        self.send(404, b'{"message":"Unknown","code":0}')

    def do_POST(self):
        self.record()
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        if self.path == "/api/v9/gifs/select":
            log(kind="body", path=self.path, body=json.loads(body), at=time.time())
            return self.send(204, b"")
        self.send(404, b'{"message":"Unknown","code":0}')

    do_PUT = do_PATCH = do_DELETE = do_POST

    def send(self, status, body, kind="application/json"):
        self.send_response(status)
        self.send_header("Content-Type", kind)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def record(self):
        raw = self.headers.get("X-Super-Properties")
        order = None
        if raw:
            order = json.loads(base64.b64decode(raw), object_pairs_hook=lambda p: p)
        log(kind="http", method=self.command, path=self.path, authorization=self.headers.get("Authorization"),
            super_properties=order, locale=self.headers.get("X-Discord-Locale"), at=time.time())

    def gateway(self):
        import hashlib
        key = self.headers["Sec-WebSocket-Key"]
        accept = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        self.wfile.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                          f"Sec-WebSocket-Accept: {accept}\r\n\r\n").encode())
        self.wfile.flush()
        log(kind="connect", path=self.path, at=time.time())
        comp = zlib.compressobj()
        conn, seq = self.connection, [0]

        def send(op, d=None, t=None):
            payload = {"op": op, "d": d}
            if t:
                seq[0] += 1
                payload.update(s=seq[0], t=t)
            body = comp.compress(json.dumps(payload).encode()) + comp.flush(zlib.Z_SYNC_FLUSH)
            conn.sendall(frame(body))

        send(10, {"heartbeat_interval": 41250})
        try:
            while True:
                opcode, data = read_frame(self.rfile)
                if opcode == 8:
                    break
                if opcode != 1:
                    continue
                text = data.decode()
                message = json.loads(text)
                if message["op"] == 1:
                    send(11)
                elif message["op"] == 2:
                    IDENTIFIES[0] += 1
                    log(kind="identify", raw=text, at=time.time())
                    send(0, ready(), "READY")
                    send(0, supplemental(), "READY_SUPPLEMENTAL")
                    if IDENTIFIES[0] == 1 and os.environ.get("FAKE_REIDENTIFY"):
                        threading.Timer(float(os.environ["FAKE_REIDENTIFY"]), lambda: send(9, False)).start()
                else:
                    log(kind="gateway", op=message["op"], d=message.get("d"), at=time.time())
        except (EOFError, OSError):
            pass
        self.close_connection = True


if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    server.daemon_threads = True
    print("fake discord on", PORT, flush=True)
    server.serve_forever()
