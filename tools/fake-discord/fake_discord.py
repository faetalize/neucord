#!/usr/bin/env python3
"""A loopback Discord with made-up data: REST and a zlib-stream gateway.

Everything it is sent goes to $FAKE_LOG as JSON lines. $FAKE_VARIANTS names
which Apex experiments the account is assigned, e.g. "hiding=1,lazy=1".
$FAKE_INTERACTION is how apps answer: ok (the default), fail, silent, 429,
modal or iframe. $FAKE_MODAL_SUBMIT is how an app takes its form: ok (the
default), fail, silent or 429. $FAKE_ANSWERS=1 ends #general's history with
apps' answers to commands others ran.
"""
import base64, json, os, re, socket, struct, sys, threading, time, zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = 8099
LOG = open(os.environ["FAKE_LOG"], "a", buffering=1)
# Guards the log and the gateway's zlib stream, which REST threads write to as well.
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
}
LADDER = json.dumps({"ladder": [64, 128, 256], "maxUpscale": 1.1, "minSnapDownDpr": 2})
INTERACTION = os.environ.get("FAKE_INTERACTION") or "ok"
if INTERACTION not in ("ok", "fail", "silent", "429", "modal", "iframe"):
    sys.exit("unknown FAKE_INTERACTION " + INTERACTION)
MODAL_SUBMIT = os.environ.get("FAKE_MODAL_SUBMIT") or "ok"
if MODAL_SUBMIT not in ("ok", "fail", "silent", "429"):
    sys.exit("unknown FAKE_MODAL_SUBMIT " + MODAL_SUBMIT)

ME = {"id": "100", "username": "tester_fake", "global_name": "Tester Fake", "avatar": None, "discriminator": "0",
      "premium_type": 0, "flags": 0, "verified": True, "mfa_enabled": False}
ALICE = {"id": "301", "username": "alice_fake", "global_name": "Alice Fake", "avatar": None, "discriminator": "0"}
BOB = {"id": "302", "username": "bob_fake", "global_name": "Bob Fake", "avatar": None, "discriminator": "0"}
VIEW, SEND, HISTORY, ATTACH, REACT, CONNECT = 1 << 10, 1 << 11, 1 << 16, 1 << 15, 1 << 6, 1 << 20
USE_APPLICATION_COMMANDS = 1 << 31


def bot(id, username, discriminator):
    return {"id": id, "username": username, "global_name": None, "avatar": None, "discriminator": discriminator,
            "bot": True}


# One id range per app. A bot's name differs from its app's, so a screenshot shows which of the two a list uses.
# Apps without a bot answer as themselves, with the discriminator 0000 of an author that is no user.
DICEY, FILER = bot("901", "Dice Roller", "4242"), bot("701", "Filer Bot", "1111")
FORMY = bot("801", "Formy Bot", "2222")
PROMOTED, PARTNER = 1 << 29, 1 << 30
APPS = {
    "900": {"id": "900", "name": "Dicey", "icon": "900a" * 8, "flags": PROMOTED | PARTNER, "bot": DICEY,
            "description": "Rolls made-up dice for made-up games, keeps a made-up score in every server, picks a "
                           "fruit when asked and says back whatever it is told. None of it is real, and all of it "
                           "is long enough to need a second line."},
    "700": {"id": "700", "name": "Filer", "icon": "700a" * 8, "flags": 0, "bot": FILER,
            "description": "Takes files"},
    "800": {"id": "800", "name": "Formy", "icon": "800a" * 8, "flags": 0, "bot": FORMY,
            "description": "Asks with forms"},
    "820": {"id": "820", "name": "Pinger", "icon": None, "flags": 0, "description": "Answers pings"},
    "830": {"id": "830", "name": "Playground", "icon": "830a" * 8, "flags": 0, "embedded_surfaces": [0],
            "description": "A made-up activity"},
    "840": {"id": "840", "name": "Translator", "icon": "840a" * 8, "flags": 0,
            "description": "Says it again in another made-up language"},
}
USER_APPS = {"840"}
USERS = {u["id"]: u for u in (ME, ALICE, BOB, DICEY, FILER, FORMY)}


def option(kind, name, description, **extra):
    return {"type": kind, "name": name, "description": description, **extra}


def command(id, app, name, description, options=(), kind=1, **extra):
    return {"id": id, "application_id": app, "version": id + "1", "type": kind, "name": name,
            "description": description, "options": list(options), "default_member_permissions": None,
            "dm_permission": True, "contexts": [0, 1, 2], "integration_types": [0], **extra}


STRING, INTEGER, USER, CHANNEL, ROLE, ATTACHMENT = 3, 4, 6, 7, 8, 11
SUB, GROUP = 1, 2
COMMANDS = [
    command("910", "900", "roll", "Roll made-up dice", [
        option(INTEGER, "count", "How many dice", required=True, min_value=1, max_value=10),
        option(STRING, "size", "Which dice", choices=[{"name": "D6", "value": "6"}, {"name": "D20", "value": "20"}]),
        option(USER, "who", "Who rolls"),
        option(ROLE, "as", "Roll for a role"),
    ], global_popularity_rank=1),
    command("911", "900", "echo", "Say it back", [option(STRING, "message", "What to say", min_length=2)]),
    command("912", "900", "pick", "Pick a fruit", [
        option(STRING, "fruit", "Which fruit", required=True, autocomplete=True),
        option(CHANNEL, "where", "Where to pick it", channel_types=[0]),
    ]),
    # The server's own command: listed only in its index, and kept by frecency as "<id>:<server>".
    command("913", "900", "stats", "Made-up scores", [
        option(SUB, "show", "Show a score", options=[option(USER, "who", "Whose score", required=True)]),
        option(GROUP, "reset", "Start again", options=[option(SUB, "all", "Every score")]),
    ], guild_id="500"),
    command("914", "900", "settings", "Dicey's settings", [
        option(SUB, "show", "Show them"),
        option(SUB, "set", "Change one", options=[option(STRING, "value", "The new value")]),
    ]),
    command("915", "900", "spicy", "Something for adults", nsfw=True),
    command("916", "900", "Report User", "", kind=2),
    command("917", "900", "Quote Message", "", kind=3),
    command("918", "900", "Spicy profile", "", kind=2, nsfw=True),
    command("710", "700", "upload", "Upload a file", [
        option(ATTACHMENT, "file", "A file", required=True),
        option(ATTACHMENT, "picture", "An image or PDF", file_types=["image", ".pdf"]),
        option(STRING, "note", "A note", autocomplete=True),
    ], integration_types=[0, 1]),
    command("810", "800", "form", "Fill in a made-up form"),
    command("821", "820", "ping", "Check it answers"),
    # PRIMARY_ENTRY_POINT, launched by Discord (handler 2).
    command("831", "830", "launch", "Start the made-up activity", kind=4, handler=2),
    command("841", "840", "translate", "Say it in another language",
            [option(STRING, "text", "What to translate", required=True)], integration_types=[1]),
]
FRUITS = ["apple", "apricot", "banana", "blackberry", "cherry", "grape", "lemon", "mango", "pear", "plum"]


def index(apps, guild=None):
    """The application-command-index of `apps`; a DM's leaves out servers' own commands."""
    commands = [c for c in COMMANDS if c["application_id"] in apps and c.get("guild_id") in (None, guild)]
    return {"version": "1", "applications": [APPS[a] for a in apps], "application_commands": commands}


def snowflake(ms_after_epoch_2026, n):
    return str(((1767225600000 - 1420070400000 + ms_after_epoch_2026) << 22) | n)


COUNTER = [0]


def snowflake_now():
    COUNTER[0] += 1
    return str(((int(time.time() * 1000) - 1420070400000) << 22) | (COUNTER[0] & 0xFFF))


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
        {"id": "500", "name": "@everyone", "position": 0,
         "permissions": str(VIEW | SEND | HISTORY | ATTACH | REACT | CONNECT | USE_APPLICATION_COMMANDS)},
        {"id": "510", "name": "Mods", "permissions": "0", "position": 1, "color": 0, "hoist": False,
         "mentionable": True, "managed": False, "flags": 0}],
        "channels": channels, "emojis": [], "stickers": [], "threads": [], "member_count": 3,
        "data_mode": "full", "joined_at": "2026-01-01T00:00:00.000000+00:00", "large": False, "lazy": True}


GUILD_CHANNELS = {c["id"] for c in guild()["channels"]}


def ready():
    return {
        "v": 9, "user": ME, "users": [ME, ALICE, BOB, DICEY], "session_id": "sess-1",
        "resume_gateway_url": f"ws://127.0.0.1:{PORT}",
        "guilds": [guild()], "merged_members": [[{"user_id": "100", "roles": [], "flags": 0,
                                                  "joined_at": "2026-01-01T00:00:00.000000+00:00"}]],
        "private_channels": [
            {"id": "201", "type": 1, "last_message_id": snowflake(4000, 3), "recipient_ids": ["301"], "flags": 0},
            {"id": "203", "type": 1, "last_message_id": None, "recipient_ids": ["901"], "flags": 0},
        ],
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
        "merged_members": [[{"user_id": "301", "roles": ["510"], "nick": "Al", "flags": 0},
                            {"user_id": "302", "roles": [], "flags": 0}]],
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


# What was posted since the fake started, by channel, oldest first.
POSTED = {}


def messages(channel):
    out = []
    if channel == "600":
        for n in range(40):
            who = ALICE if n % 2 else BOB
            text = "line %d of the made-up conversation" % n
            files = ()
            if n == 0:
                text, files = "the oldest picture", (attachment(0),)
            if n == 39:
                text, files = "the newest picture, and see <#603> for the plan", (attachment(39),)
            out.append(message(n, who, text, files))
        if os.environ.get("FAKE_ANSWERS"):
            out += answers(out)
    return list(reversed(out + POSTED.get(channel, [])))


def answers(lines):
    """Dicey's answers to what Alice and Bob ran in #general, after its 40 lines: a slash command, another a
    minute later (its own group), a user command, a message command on line 30, a button's answer (no name, so
    no line), an activity, and Alice's reply to an answer with only an embed."""
    def answer(n, who, name, kind=20, content=None, metadata=(), **fields):
        msg = message(n, DICEY, "A made-up answer to %s." % name if content is None else content)
        meta = {"id": snowflake(1000 + n * 100, 99), "type": 2, "user": who,
                "authorizing_integration_owners": {"0": "500"}, "command_type": 1 if kind == 20 else 2,
                **dict(metadata)}
        msg.update(type=kind, application_id="900", webhook_id="900", interaction_metadata=meta,
                   interaction={"id": meta["id"], "type": meta["type"], "name": name, "user": who}, **fields)
        return msg
    quoted = lines[30]
    reference = {"type": 0, "message_id": quoted["id"], "channel_id": "600", "guild_id": "500"}
    embed_only = answer(46, BOB, "stats show", content="", embeds=[{"title": "Made-up scores", "type": "rich"}])
    reply = message(47, ALICE, "what did that say?")
    reply.update(type=19, message_reference={"type": 0, "message_id": embed_only["id"], "channel_id": "600"},
                 referenced_message=embed_only)
    return [
        answer(40, ALICE, "settings set"),
        answer(41, BOB, "roll"),
        answer(42, ALICE, "Report User", kind=23, metadata={"command_type": 2, "target_user": BOB}),
        answer(43, BOB, "Quote Message", kind=23, metadata={"command_type": 3, "target_message_id": quoted["id"]},
               message_reference=reference, referenced_message=quoted),
        answer(44, ALICE, "", content="A made-up answer to a button.", metadata={"type": 3}),
        answer(45, BOB, "launch", content="", metadata={"command_type": None}, activity_instance={"id": "a1"}),
        embed_only,
        reply,
    ]


def find(channel, id):
    return next((m for m in messages(channel) if m["id"] == id), None)


def post(channel, author, content, **fields):
    """A new message in `channel`, kept for its history and pushed as MESSAGE_CREATE."""
    msg = {"id": snowflake_now(), "channel_id": channel, "author": author, "content": content,
           "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S.000000+00:00", time.gmtime()), "edited_timestamp": None,
           "tts": False, "mention_everyone": False, "mentions": [], "mention_roles": [], "attachments": [],
           "embeds": [], "components": [], "pinned": False, "type": 0, "flags": 0, **fields}
    POSTED.setdefault(channel, []).append(msg)
    event = dict(msg)
    if channel in GUILD_CHANNELS:
        event["guild_id"] = "500"
        if author is ME:
            event["member"] = {"roles": [], "joined_at": "2026-01-01T00:00:00.000000+00:00", "deaf": False,
                               "mute": False, "flags": 0}
    return msg, lambda: dispatch("MESSAGE_CREATE", event)


def png(w, h):
    raw = b"".join(b"\x00" + bytes([40, 120, 200]) * w for _ in range(h))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


# Protobuf, as much as the settings protos need: varints, length-delimited fields and packed varints.
def varint(n):
    out = b""
    while True:
        low, n = n & 0x7F, n >> 7
        out += bytes([low | (0x80 if n else 0)])
        if not n:
            return out


def field(number, value):
    if isinstance(value, int):
        return varint(number << 3) + varint(value)
    return varint(number << 3 | 2) + varint(len(value)) + value


def fields(data):
    """The top-level fields of a message, as (number, the field's bytes)."""
    out, i = [], 0
    def read():
        nonlocal i
        shift = value = 0
        while True:
            value |= (data[i] & 0x7F) << shift
            i, shift = i + 1, shift + 7
            if not data[i - 1] & 0x80:
                return value
    while i < len(data):
        start, key = i, read()
        wire = key & 7
        if wire == 0:
            read()
        elif wire == 2:
            length = read()
            i += length
        else:
            i += 8 if wire == 1 else 4
        out.append((key >> 3, data[start:i]))
    return out


def frecency(uses):
    """A map of FrecencyItems by key (803805): each key's use times, in ms."""
    item = lambda times: field(1, len(times)) + field(2, b"".join(map(varint, times)))
    return b"".join(field(1, field(1, key.encode()) + field(2, item(times))) for key, times in uses.items())


def frecency_settings():
    now, day = int(time.time() * 1000), 86400000
    commands = {"910": [now - 3 * day, now - day], "914\0set": [now - 2 * 3600000, now - 3600000],
                "913\0show:500": [now - 5 * day] * 3}
    apps = {"840": [now - 3600000], "900": [now - 7 * day] * 3}
    # FrecencyUserSettings: application_command_frecency, application_frecency.
    return field(7, frecency(commands)) + field(9, frecency(apps))


# Each settings proto as Discord keeps it, without `versions`, and its data_version.
SETTINGS = {"1": [b"", 1], "2": [frecency_settings(), 1]}


def settings_answer(kind):
    proto, version = SETTINGS[kind]
    # Versions.data_version
    return base64.b64encode(field(1, field(3, version)) + proto).decode()


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
# The `send` of the gateway that identified last, which REST answers push their events through.
SESSION = [None]


def dispatch(t, d):
    send = SESSION[0]
    if send is None:
        return
    try:
        send(0, d, t)
    except OSError:
        return
    log(kind="dispatch", t=t, at=time.time())


ROUTES = []


def route(method, pattern):
    """A REST answer: called with the path's groups and the request's JSON body, it returns the status, what to
    answer (None for no body) and optionally what to push on the gateway once that is sent."""
    def add(answer):
        ROUTES.append((method, re.compile("/api/v9" + pattern), answer))
        return answer
    return add


@route("GET", r"/channels/(\d+)/messages")
def history(channel, body):
    return 200, messages(channel)


@route("POST", r"/channels/(\d+)/messages")
def send_message(channel, body):
    content = body.get("content", "")
    extra = {"tts": bool(body.get("tts")), "flags": body.get("flags", 0), "nonce": body.get("nonce"),
             "mentions": [USERS[u] for u in re.findall(r"<@!?(\d+)>", content) if u in USERS],
             "mention_roles": re.findall(r"<@&(\d+)>", content)}
    reference = body.get("message_reference")
    if reference:
        replied = find(channel, reference.get("message_id"))
        extra.update(type=19, message_reference={"type": 0, **reference}, referenced_message=replied)
        if replied and body.get("allowed_mentions", {}).get("replied_user", True):
            extra["mentions"].append(replied["author"])
    return (200, *post(channel, ME, content, **extra))


@route("POST", r"/channels/(\d+)/typing")
def typing(channel, body):
    return 204, None


@route("GET", r"/guilds/(\d+)/application-command-index")
def guild_index(guild, body):
    return 200, index(["900", "700", "800", "820", "830"] if guild == "500" else [], guild)


@route("GET", r"/channels/(\d+)/application-command-index")
def dm_index(channel, body):
    return 200, index(["900"] if channel == "203" else [])


@route("GET", r"/users/@me/application-command-index")
def user_index(body):
    return 200, index(sorted(USER_APPS))


@route("GET", r"/users/@me/settings-proto/([12])")
def settings(kind, body):
    return 200, {"settings": settings_answer(kind)}


@route("PATCH", r"/users/@me/settings-proto/([12])")
def save_settings(kind, body):
    """Each top-level field sent replaces Discord's, unless the edit was made against an older data_version."""
    proto, version = SETTINGS[kind]
    required = body.get("required_data_version")
    if required is not None and required < version:
        return 200, {"settings": settings_answer(kind), "out_of_date": True}
    sent = [f for f in fields(base64.b64decode(body.get("settings", ""))) if f[0] != 1]
    numbers = {number for number, _ in sent}
    kept = [raw for number, raw in fields(proto) if number not in numbers]
    SETTINGS[kind] = [b"".join(kept) + b"".join(raw for _, raw in sent), version + 1]
    return 200, {"settings": settings_answer(kind), "out_of_date": False}


def fake_gif(n, id=None):
    """A made-up result of the GIF endpoints; `id` overrides the string id, as an answer may carry a number."""
    return {"id": f"fake-gif-{n}" if id is None else id, "url": f"https://gifs.example/view/fake-{n}",
            "src": f"//127.0.0.1:{PORT}/attachments/gifs/{n}.png", "width": 300, "height": 200}


@route("GET", r"/gifs/trending")
def gif_tiles(body):
    return 200, {"categories": [{"name": "happy", "src": f"//127.0.0.1:{PORT}/attachments/gifs/0.png"}],
                 "gifs": [fake_gif(1)]}


@route("GET", r"/gifs/trending-gifs")
def gifs_trending(body):
    return 200, [fake_gif(1), fake_gif(2)]


@route("GET", r"/gifs/search")
def gifs_search(body):
    return 200, [fake_gif(3), fake_gif(4, id=4004), fake_gif(5), fake_gif(6)]


@route("GET", r"/gifs/suggest")
def gifs_suggest(body):
    return 200, ["kitten", "cats"]


@route("POST", r"/gifs/select")
def gifs_select(body):
    return 204, None


# The nonces turned down once, under FAKE_INTERACTION=429.
LIMITED = set()


@route("POST", r"/interactions")
def interaction(body):
    """204, then what the app does on the gateway as $FAKE_INTERACTION says: commands are answered by a message
    from the app, autocomplete with the fruits that start with the focused value, presses and forms with success
    alone. A 429 turns each nonce down once."""
    kind, nonce, iid = body.get("type"), body.get("nonce"), snowflake_now()
    if INTERACTION == "429" and nonce not in LIMITED:
        LIMITED.add(nonce)
        return 429, {"message": "You are being rate limited.", "retry_after": 1, "global": False}
    app = APPS.get(body.get("application_id"), {"id": body.get("application_id"), "name": "Unknown"})
    if kind == 5:
        return modal_submitted(body, app, nonce, iid)

    def answer():
        if INTERACTION == "silent":
            return
        dispatch("INTERACTION_CREATE", {"id": iid, "nonce": nonce})
        if INTERACTION == "fail":
            # 2: the app didn't answer in time.
            return dispatch("INTERACTION_FAILURE", {"id": iid, "nonce": nonce, "reason_code": 2})
        if kind == 4:
            typed = focused(body.get("data", {}).get("options")) or ""
            choices = [{"name": f, "value": f} for f in FRUITS if f.startswith(typed.strip().lower())]
            return dispatch("APPLICATION_COMMAND_AUTOCOMPLETE_RESPONSE",
                            {"application_id": app["id"], "nonce": nonce, "choices": choices[:25]})
        if kind in (2, 3) and INTERACTION in ("modal", "iframe"):
            form = {"id": iid, "nonce": nonce, "channel_id": body.get("channel_id"), "custom_id": "feedback",
                    "title": "Tell us", "application": app}
            if INTERACTION == "iframe":
                return dispatch("INTERACTION_IFRAME_MODAL_CREATE", {**form, "iframe_path": "/", "modal_size": 1})
            return dispatch("INTERACTION_MODAL_CREATE", {**form, **modal_form()})
        dispatch("INTERACTION_SUCCESS", {"id": iid, "nonce": nonce})
        if kind == 2:
            _, push = answered(body, app, iid)
            push()

    return 204, None, answer


def modal_form():
    """The form every press and command gets under FAKE_INTERACTION=modal: one of each field a form submits."""
    def label(text, component, description=None):
        return {"type": 18, "label": text, "description": description, "component": component}

    def options(*names, default=None):
        return [{"label": n.title(), "value": n, "description": "the %s one" % n, "default": n == default}
                for n in names]

    components = [
        {"type": 1, "components": [{"type": 4, "custom_id": "name", "style": 1, "label": "Name", "required": True,
                                    "placeholder": "Who you are", "max_length": 20}]},
        label("About", {"type": 4, "custom_id": "about", "style": 2, "min_length": 5, "max_length": 200,
                        "required": True}, "A few made-up words"),
        label("Flavour", {"type": 3, "custom_id": "flavour", "options": options("lemon", "mint", default="mint")}),
        {"type": 10, "content": "Answers go to **Formy**, a made-up app."},
        label("Size", {"type": 21, "custom_id": "size", "options": options("small", "large"), "required": True}),
        label("Toppings", {"type": 22, "custom_id": "toppings", "options": options("nuts", "fruit", "cream"),
                           "max_values": 2}),
        label("Agree", {"type": 23, "custom_id": "agree"}, "Tick to agree"),
        label("Friend", {"type": 5, "custom_id": "friend", "default_values": [{"id": "301", "type": "user"}]}),
        label("Picture", {"type": 19, "custom_id": "picture", "file_types": ["image"]}),
    ]
    return {"components": components, "resolved": {"users": {"301": ALICE},
                                                   "members": {"301": {"nick": "Al", "roles": []}}}}


def modal_submitted(body, app, nonce, iid):
    """A form's submit: 204, then as $FAKE_MODAL_SUBMIT says. A success is followed by the app's reply."""
    if MODAL_SUBMIT == "429" and nonce not in LIMITED:
        LIMITED.add(nonce)
        return 429, {"message": "You are being rate limited.", "retry_after": 1, "global": False}

    def answer():
        if MODAL_SUBMIT == "silent":
            return
        dispatch("INTERACTION_CREATE", {"id": iid, "nonce": nonce})
        if MODAL_SUBMIT == "fail":
            return dispatch("INTERACTION_FAILURE", {"id": iid, "nonce": nonce})
        dispatch("INTERACTION_SUCCESS", {"id": iid, "nonce": nonce})
        channel = body.get("channel_id")
        owners = {"0": body.get("guild_id") or "0"}
        _, push = post(channel, app.get("bot") or ME, "Thanks for filling in the made-up form.", type=20,
                       application_id=app["id"], webhook_id=app["id"],
                       interaction_metadata={"id": iid, "type": 5, "user": ME,
                                             "authorizing_integration_owners": owners})
        push()

    return 204, None, answer


def form_message():
    """Formy's message in 600, the newest of its history, whose button asks for its form."""
    button = {"type": 2, "style": 1, "label": "Open form", "custom_id": "open-form"}
    msg, _ = post("600", FORMY, "Press it for a made-up form.", application_id="800",
                  components=[{"type": 1, "components": [button]}], id=snowflake(5000, 41),
                  timestamp="2026-01-01T10:40:00.000000+00:00")
    return msg


def focused(options):
    for o in options or []:
        if o.get("focused"):
            return str(o.get("value", ""))
        inner = focused(o.get("options"))
        if inner is not None:
            return inner
    return None


def answered(body, app, iid):
    """The app's answer to the command `body` ran: type 20 for a slash command, 23 for a user or message one."""
    data, channel, extra = body.get("data", {}), body.get("channel_id"), {}
    path, options = [data.get("name", "")], data.get("options") or []
    while options and options[0].get("type") in (SUB, GROUP):
        path.append(options[0]["name"])
        options = options[0].get("options") or []
    name, kind = " ".join(path), data.get("type", 1)
    author = app.get("bot") or {"id": app["id"], "username": app["name"], "avatar": None, "discriminator": "0000",
                                "bot": True}
    owners = {"1": ME["id"]} if app["id"] in USER_APPS else {"0": body.get("guild_id") or "0"}
    metadata = {"id": iid, "type": 2, "user": ME, "authorizing_integration_owners": owners, "name": name,
                "command_type": kind}
    if kind == 2 and data.get("target_id") in USERS:
        metadata["target_user"] = USERS[data["target_id"]]
    if kind == 3:
        target = metadata["target_message_id"] = data.get("target_id")
        # It refers to its target as a reply does, and the client draws that above it (`sl8` in 652215 holds 23).
        reference = {"type": 0, "message_id": target, "channel_id": channel}
        if channel in GUILD_CHANNELS:
            reference["guild_id"] = "500"
        extra.update(message_reference=reference, referenced_message=find(channel, target))
    text = "A made-up answer to %s%s." % ("/" if kind == 1 else "", name)
    return post(channel, author, text, type=20 if kind == 1 else 23, application_id=app["id"],
                webhook_id=app["id"], interaction={"id": iid, "type": 2, "name": name, "user": ME},
                interaction_metadata=metadata, **extra)


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
        self.answer()

    def answer(self):
        body = self.record()
        path = self.path.split("?")[0]
        for method, pattern, handle in ROUTES:
            match = pattern.fullmatch(path)
            if method == self.command and match:
                status, payload, *after = handle(*match.groups(), body if isinstance(body, dict) else {})
                headers = [("Retry-After", str(payload["retry_after"]))] if status == 429 else []
                self.send(status, b"" if payload is None else json.dumps(payload).encode(), headers=headers)
                for push in after:
                    push()
                return
        self.send(404, b'{"message":"Unknown","code":0}')

    do_POST = do_PUT = do_PATCH = do_DELETE = answer

    def send(self, status, body, kind="application/json", headers=()):
        self.send_response(status)
        if body:
            self.send_header("Content-Type", kind)
        for name, value in headers:
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def record(self):
        """Logs the request with its JSON body (else the body's length), and X-Audit-Log-Reason and
        X-Context-Properties when sent, even empty. Returns the body."""
        n = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(n) if n else b""
        props = self.headers.get("X-Super-Properties")
        order = json.loads(base64.b64decode(props), object_pairs_hook=lambda p: p) if props else None
        entry = dict(kind="http", method=self.command, path=self.path, authorization=self.headers.get("Authorization"),
                     super_properties=order, locale=self.headers.get("X-Discord-Locale"))
        for header, key in (("X-Audit-Log-Reason", "audit_log_reason"), ("X-Context-Properties", "context_properties")):
            if header in self.headers:
                entry[key] = self.headers[header]
        body = None
        if raw:
            try:
                body = entry["body"] = json.loads(raw)
            except ValueError:
                entry["body_length"] = len(raw)
        log(**entry, at=time.time())
        return body

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
            with LOCK:
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
                    SESSION[0] = send
                    if IDENTIFIES[0] == 1 and os.environ.get("FAKE_REIDENTIFY"):
                        threading.Timer(float(os.environ["FAKE_REIDENTIFY"]), lambda: send(9, False)).start()
                else:
                    log(kind="gateway", op=message["op"], d=message.get("d"), at=time.time())
        except (EOFError, OSError):
            pass
        if SESSION[0] is send:
            SESSION[0] = None
        self.close_connection = True


form_message()

if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    server.daemon_threads = True
    print("fake discord on", PORT, flush=True)
    server.serve_forever()
