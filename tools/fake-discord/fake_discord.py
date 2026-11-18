#!/usr/bin/env python3
"""A loopback Discord with made-up data: REST and a zlib-stream gateway.

Everything it is sent goes to $FAKE_LOG as JSON lines. $FAKE_VARIANTS names
which Apex experiments the account is assigned, e.g. "hiding=1,lazy=1".
$FAKE_INTERACTION is how apps answer: ok (the default), fail, silent, 429,
modal or iframe. $FAKE_MODAL_SUBMIT is how an app takes its form: ok (the
default), fail, silent or 429. $FAKE_ANSWERS=1 ends #general's history with
apps' answers to commands others ran. $FAKE_NSFW sets what decides
age-restricted commands, e.g. "allowed=1,dms=1,status=3,gated=0,level=0,owner=0"
(those are the defaults but dms, which is 0).
"""
import base64, json, os, re, socket, struct, subprocess, sys, tempfile, threading, time, urllib.parse, zlib
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
    "nsfwlevel": "2025-09-server-nsfw-level",
    "chatbar": "2026-06-chat-bar-customization",
    "dice": "2026-05-dice-roll-slash-command",
    "packs": "2026-09-default-emojis",
}
LADDER = json.dumps({"ladder": [64, 128, 256], "maxUpscale": 1.1, "minSnapDownDpr": 2})
# Under FAKE_OWNER the account owns the server, so its moderation commands show.
OWNER = bool(os.environ.get("FAKE_OWNER"))
INTERACTION = os.environ.get("FAKE_INTERACTION") or "ok"
if INTERACTION not in ("ok", "fail", "silent", "429", "modal", "iframe"):
    sys.exit("unknown FAKE_INTERACTION " + INTERACTION)
MODAL_SUBMIT = os.environ.get("FAKE_MODAL_SUBMIT") or "ok"
if MODAL_SUBMIT not in ("ok", "fail", "silent", "429"):
    sys.exit("unknown FAKE_MODAL_SUBMIT " + MODAL_SUBMIT)
# Under FAKE_SLOWMODE=<s> #general has that slowmode, and a thread there is refused as inside its cooldown.
SLOWMODE = int(os.environ.get("FAKE_SLOWMODE") or 0)
UPLOAD_FAIL = os.environ.get("FAKE_UPLOAD_FAIL") or ""
# FAKE_IMAGE_FAIL=1: every image on the CDN answers 404.
IMAGE_FAIL = bool(os.environ.get("FAKE_IMAGE_FAIL"))
# FAKE_VIDEO=<path to a .webm>: channel 600 gets a message with that file as a video attachment.
VIDEO = os.environ.get("FAKE_VIDEO")
if UPLOAD_FAIL not in ("", "too-large"):
    sys.exit("unknown FAKE_UPLOAD_FAIL " + UPLOAD_FAIL)

ME = {"id": "100", "username": "tester_fake", "global_name": "Tester Fake", "avatar": None, "discriminator": "0",
      "premium_type": 0, "flags": 0, "verified": True, "mfa_enabled": False}
NSFW = {"allowed": 1, "dms": 0, "status": 3, "gated": 0, "level": 0, "owner": 0}
NSFW.update((k, int(v)) for k, v in (p.split("=") for p in os.environ.get("FAKE_NSFW", "").split(",") if p))
# FAKE_NITRO=1: the account has Nitro (premium_type 2), which unlocks emoji packs' emoji.
if os.environ.get("FAKE_NITRO"):
    ME["premium_type"] = 2
ME.update(nsfw_allowed=bool(NSFW["allowed"]), age_verification_status=NSFW["status"])
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


def custom_emoji():
    """FAKE_TOP_EMOJI: two custom emoji of the server, for its top list."""
    if not os.environ.get("FAKE_TOP_EMOJI"):
        return []
    base = {"available": True, "managed": False, "animated": False, "roles": [], "require_colons": True}
    return [{**base, "id": "8101", "name": "blob_wave"}, {**base, "id": "8102", "name": "blob_cry"}]


def guild_stickers():
    """The server's stickers: one found by its name, one only by its tag (`/sticker`'s "Similar to")."""
    sticker = {"format_type": 1, "available": True, "guild_id": "500", "type": 2, "description": None}
    return [{**sticker, "id": "710", "name": "wave", "tags": "wave"},
            {**sticker, "id": "711", "name": "hi there", "tags": "wave"}]


def guild():
    channels = [
        {"id": "600", "type": 0, "name": "general", "position": 0, "parent_id": None, "permission_overwrites": [],
         "last_message_id": snowflake(5000, 40), "flags": 0, "rate_limit_per_user": SLOWMODE},
        {"id": "601", "type": 0, "name": "hidden-plans", "position": 1, "parent_id": None, "flags": 1 << 17,
         "permission_overwrites": [{"id": "500", "type": 0, "allow": "0", "deny": str(VIEW)}]},
        {"id": "603", "type": 0, "name": "leaked-name", "position": 2, "parent_id": None, "flags": 1 << 17,
         "permission_overwrites": []},
        {"id": "602", "type": 2, "name": "Lounge", "position": 3, "parent_id": None, "bitrate": 64000,
         "permission_overwrites": [], "flags": 0},
        {"id": "604", "type": 0, "name": "after-dark", "position": 4, "parent_id": None, "nsfw": True,
         "permission_overwrites": [], "flags": 0},
    ]
    threads = []
    if os.environ.get("FAKE_THREADS"):
        # Two active threads of #general (#323): one with its newest message, one without.
        now = time.strftime("%Y-%m-%dT%H:%M:%S.000000+00:00", time.gmtime())
        recent = {"id": snowflake_now(), "channel_id": "7201", "author": ALICE, "content": "see you there at **noon**",
                  "timestamp": now, "edited_timestamp": None, "tts": False, "mention_everyone": False, "mentions": [],
                  "mention_roles": [], "attachments": [], "embeds": [], "pinned": False, "type": 0, "flags": 0}
        for tid, name, last in [("7201", "lunch plans", recent), ("7202", "quiet corner", None)]:
            threads.append({"id": tid, "type": 11, "guild_id": "500", "parent_id": "600", "name": name,
                            "owner_id": "301", "last_message_id": last["id"] if last else None, "message_count": 1,
                            "member_count": 1, "rate_limit_per_user": 0, "flags": 0,
                            "thread_metadata": {"archived": False, "locked": False, "archive_timestamp": now,
                                                "auto_archive_duration": 4320, "create_timestamp": now},
                            **({"most_recent_message": last} if last else {})})
    if os.environ.get("FAKE_CHAIN"):
        # The thread line 10 started: a thread's id is its starter message's.
        now = time.strftime("%Y-%m-%dT%H:%M:%S.000000+00:00", time.gmtime())
        threads.append({"id": snowflake(1000 + 10 * 100, 10), "type": 11, "guild_id": "500", "parent_id": "600",
                        "name": "about line ten", "owner_id": "301", "last_message_id": None, "message_count": 1,
                        "member_count": 1, "rate_limit_per_user": 0, "flags": 0,
                        "thread_metadata": {"archived": False, "locked": False, "archive_timestamp": now,
                                            "auto_archive_duration": 4320, "create_timestamp": now}})
    if os.environ.get("FAKE_FORUM"):
        # A forum (#605) with two tags and three active posts, as THREAD_LIST_SYNC and the guild's threads have them.
        channels.append({"id": "605", "type": 15, "name": "ideas", "position": 5, "parent_id": None, "flags": 0,
                         "permission_overwrites": [], "topic": "Share an **idea**.", "template": "",
                         "available_tags": [{"id": "9001", "name": "alpha", "moderated": False},
                                            {"id": "9002", "name": "beta", "moderated": False}],
                         "default_sort_order": 0, "default_forum_layout": 1})
        now = time.strftime("%Y-%m-%dT%H:%M:%S.000000+00:00", time.gmtime())
        for n, (tid, name, tags) in enumerate([("7101", "weekend plans", ["9001"]), ("7102", "plan b", ["9002"]),
                                               ("7103", "other things", [])]):
            threads.append({"id": tid, "type": 11, "guild_id": "500", "parent_id": "605", "name": name,
                            "owner_id": "301", "last_message_id": snowflake(9000 + n, 5), "message_count": 1,
                            "member_count": 1, "rate_limit_per_user": 0, "flags": 0, "applied_tags": tags,
                            "thread_metadata": {"archived": False, "locked": False, "archive_timestamp": now,
                                                "auto_archive_duration": 4320, "create_timestamp": now}})
    stages = []
    if os.environ.get("FAKE_STAGE"):
        # FAKE_STAGE: a live stage (#606) with a topic; its speakers and audience are in supplemental().
        channels.append({"id": "606", "type": 13, "name": "town-hall", "position": 6, "parent_id": None, "flags": 0,
                         "permission_overwrites": []})
        stages.append({"id": "8000", "guild_id": "500", "channel_id": "606", "privacy_level": 2,
                       "topic": "Quarterly town hall: ask us anything about the roadmap", "discoverable_disabled": False})
    return {"id": "500", "properties": {"name": "Test Server", "owner_id": "100" if OWNER else "999", "icon": None, "features": [],
            "premium_tier": 0, "default_message_notifications": 0, "nsfw_level": NSFW["level"],
            "owner_configured_content_level": NSFW["owner"]}, "roles": [
        {"id": "500", "name": "@everyone", "position": 0,
         "permissions": str(VIEW | SEND | HISTORY | ATTACH | REACT | CONNECT | USE_APPLICATION_COMMANDS)},
        {"id": "510", "name": "Mods", "permissions": "0", "position": 1, "color": 0,
         "hoist": bool(os.environ.get("FAKE_THREAD_MEMBERS")),
         "mentionable": True, "managed": False, "flags": 0}],
        "channels": channels, "emojis": custom_emoji(), "stickers": guild_stickers(), "threads": threads, "member_count": 3,
        "data_mode": "full", "joined_at": "2026-01-01T00:00:00.000000+00:00", "large": False, "lazy": True,
            "stage_instances": stages}


GUILD_CHANNELS = {c["id"] for c in guild()["channels"]}


def friend_requests():
    """FAKE_FRIEND_NOTES=1: Bob's request to us, with the message he wrote."""
    if not os.environ.get("FAKE_FRIEND_NOTES"):
        return []
    return [{"id": "302", "type": 3, "user_id": "302", "since": "2026-02-01T00:00:00.000000+00:00",
             "nickname": None, "note": "Hi, we met at the meetup!"}]


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
                           "nickname": None}] + friend_requests(),
        "read_state": {"entries": [
            {"id": "201", "last_message_id": snowflake(4000, 3), "mention_count": 0},
            {"id": "500", "read_state_type": 3, "last_message_id": "0", "mention_count": 7},
        ], "partial": False, "version": 1},
        "user_guild_settings": {"entries": [], "partial": False, "version": 1},
        "apex_experiments": apex(), "experiments": [], "guild_experiments": [],
        "regional_feature_config": {"age_gated_features": NSFW["gated"], "teen_by_default_settings": 0},
        **({"user_settings_proto": settings_answer("1")} if NSFW["dms"] else {}),
        "auth_token": os.environ.get("FAKE_NEW_TOKEN") or None,
        "country_code": "ZZ", "analytics_token": "fake", "api_code_version": 0,
        # A DerivedQosData of made-up claims and signature, key 1, which IDENTIFY sends back.
        "qos_token": "Cg5tYWRlLXVwIGNsYWltcxIRbWFkZS11cCBzaWduYXR1cmUYAQ==",
    }


def supplemental():
    voice = [{"user_id": "301", "channel_id": "602", "session_id": "v1",
              "deaf": False, "mute": False, "self_deaf": False, "self_mute": False}]
    if os.environ.get("FAKE_STAGE"):
        # Alice speaks on the stage, Bob and a made-up third listen.
        voice += [{"user_id": "302", "channel_id": "606", "session_id": "v2", "suppress": False, "deaf": False,
                   "mute": False, "self_deaf": False, "self_mute": False},
                  {"user_id": "303", "channel_id": "606", "session_id": "v3", "suppress": True, "deaf": False,
                   "mute": False, "self_deaf": False, "self_mute": False},
                  {"user_id": "304", "channel_id": "606", "session_id": "v4", "suppress": True, "deaf": False,
                   "mute": False, "self_deaf": False, "self_mute": False}]
    return {
        "guilds": [{"id": "500", "voice_states": voice}],
        "merged_members": [[{"user_id": "301", "roles": ["510"], "nick": "Al", "flags": 0, "joined_at": JOINED},
                            {"user_id": "302", "roles": [], "flags": 0, "joined_at": JOINED}]],
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


def video_attachment(n):
    url = f"http://127.0.0.1:{PORT}/attachments/1/{900 + n}/clip.webm?ex=ffffffff&is=1&hm=2&"
    return {"id": str(900 + n), "filename": "clip.webm", "size": os.path.getsize(VIDEO), "width": 320, "height": 180,
            "content_type": "video/webm", "description": "A test card in motion", "url": url, "proxy_url": url}


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
            if n == 38 and VIDEO:
                text, files = "a made-up clip", (video_attachment(38),)
            msg = message(n, who, text, files)
            if n == 10 and os.environ.get("FAKE_CHAIN"):
                msg["flags"] = 1 << 5
            if n == 37 and os.environ.get("FAKE_EMBED"):
                # A card whose small picture (the thumbnail) is a GIF, to the fake's own image.
                pic = f"http://127.0.0.1:{PORT}/attachments/1/900/embed.gif?ex=ffffffff&is=1&hm=2&"
                msg["content"] = "a card with a thumbnail embed-probe"
                msg["embeds"] = [{"type": "rich", "title": "A made-up card", "description": "With a small picture beside it.",
                                  "thumbnail": {"url": pic, "proxy_url": pic, "width": 64, "height": 64}}]
            out.append(msg)
        if os.environ.get("FAKE_CHAIN"):
            out += reply_chain()
        if os.environ.get("FAKE_ANSWERS"):
            out += answers(out)
        if os.environ.get("FAKE_POLL"):
            out.append(poll_message(60))
        if os.environ.get("FAKE_VOICE"):
            out.append(voice_message(61))
        if os.environ.get("FAKE_SENSITIVE"):
            out += sensitive_messages()
    return list(reversed(out + POSTED.get(channel, [])))


# FAKE_SENSITIVE: pictures Discord's scan has flagged as explicit, ones it has not looked at yet, ones it found
# clean, and an embed with a flagged picture. An account that is not an adult's (FAKE_NSFW=allowed=0) has them
# covered in #general.
def sensitive_messages():
    def picture(n, **scan):
        return {**attachment(n), "filename": "scan%d.png" % n, **scan}
    out = [message(62, ALICE, "scanned, flagged explicit", (picture(62, flags=16, content_scan_version=4),)),
           message(63, ALICE, "not scanned yet", (picture(63),)),
           message(64, ALICE, "scanned, clean", (picture(64, content_scan_version=4),)),
           message(65, ALICE, "an embed with a flagged picture")]
    pic = f"http://127.0.0.1:{PORT}/attachments/1/900/embed.gif?ex=ffffffff&is=1&hm=2&"
    out[3]["embeds"] = [{"type": "rich", "title": "A made-up card", "description": "Its text stays.", "flags": 16,
                         "content_scan_version": 4,
                         "image": {"url": pic, "proxy_url": pic, "width": 300, "height": 200}}]
    for i, msg in enumerate(out):
        msg["timestamp"] = "2026-01-01T11:%02d:00.000000+00:00" % i
    return out


# FAKE_POLL: a poll that is over, so its counts show. Its first answer has 130 voters (more than a page of 100),
# the second one, the third none.
POLL_VOTERS = {1: 130, 2: 1, 3: 0}


# FAKE_VOICE: a voice message of 8 s of silence in #general, and its Ogg Opus file, made by GStreamer.
VOICE_SECS = 8


def voice_file():
    path = os.path.join(tempfile.gettempdir(), "fake-discord-voice-%d.ogg" % VOICE_SECS)
    if not os.path.exists(path):
        buffers = int(VOICE_SECS * 44100 / 1024)
        subprocess.run(["gst-launch-1.0", "-q", "audiotestsrc", "num-buffers=%d" % buffers, "wave=silence", "!",
                        "audioconvert", "!", "audioresample", "!", "opusenc", "!", "oggmux", "!", "filesink",
                        "location=" + path], check=True)
    return open(path, "rb").read()


def voice_message(n):
    url = f"http://127.0.0.1:{PORT}/attachments/1/{900 + n}/voice-message.ogg?ex=ffffffff&is=1&hm=2&"
    waveform = base64.b64encode(bytes(int(127 + 120 * ((k * 7) % 11 - 5) / 5) for k in range(256))).decode()
    file = {"id": str(900 + n), "filename": "voice-message.ogg", "size": 9000, "content_type": "audio/ogg",
            "duration_secs": float(VOICE_SECS), "waveform": waveform, "url": url, "proxy_url": url}
    return {**message(n, ALICE, "", (file,)), "flags": 1 << 13}


def poll_message(n):
    poll = {"question": {"text": "Best snack?"}, "allow_multiselect": False, "layout_type": 1,
            "expiry": "2026-01-02T00:00:00.000000+00:00",
            "answers": [{"answer_id": 1, "poll_media": {"text": "Crisps"}},
                        {"answer_id": 2, "poll_media": {"text": "Fruit"}},
                        {"answer_id": 3, "poll_media": {"text": "Nothing"}}],
            "results": {"is_finalized": True,
                        "answer_counts": [{"id": k, "count": c, "me_voted": False} for k, c in POLL_VOTERS.items()]}}
    return {**message(n, ALICE, ""), "poll": poll}


def voter(i):
    return {"id": str(4000 + i), "username": "voter_%03d" % i, "global_name": "Voter %03d" % i, "avatar": None,
            "discriminator": "0"}


def reply_chain():
    """FAKE_CHAIN=1: lines 70 to 73, each a reply to the one before, so a reply to 73 continues a chain of three
    replies; and line 10 of #general started a thread (see `guild()`), so it carries HAS_THREAD."""
    chain = [message(70, BOB, "the start of a chain")]
    for n in (71, 72, 73):
        quoted = chain[-1]
        reply = message(n, ALICE if n % 2 else BOB, "a reply, number %d" % (n - 70))
        reply.update(type=19, message_reference={"type": 0, "message_id": quoted["id"], "channel_id": "600"},
                     referenced_message=quoted)
        chain.append(reply)
    return chain


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


# Each settings proto as Discord keeps it, without `versions`, and its data_version. With FAKE_NSFW's dms=1,
# proto 1 has text_and_images.view_nsfw_commands (6.23, a BoolValue) on.
SETTINGS = {"1": [field(6, field(23, field(1, 1))) if NSFW["dms"] else b"", 1], "2": [frecency_settings(), 1]}


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
# The query string of the request being answered, by name, each with its values.
QUERY = {}


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


@route("GET", r"/(?:guilds|channels)/(\d+)/messages/search")
def search_messages(scope, body):
    """The lines of #general that have the text asked for (`content`), a message to a result."""
    text = QUERY.get("content", [""])[0].lower()
    found = [m for m in messages("600") if text and text in m["content"].lower()]
    return 200, {"messages": [[m] for m in found[:25]], "total_results": len(found)}


# The pins of #600 are lines 5, 7 and 9, pinned in that order (newest pin first), two to a page.
PINNED = ["5", "7", "9"]


@route("GET", r"/channels/(\d+)/polls/(\d+)/answers/(\d+)")
def poll_answer_voters(channel, message_id, answer, body):
    """A page of the answer's voters, by id, `limit` of them after `after`."""
    limit = int(QUERY.get("limit", ["25"])[0])
    after = int(QUERY.get("after", ["0"])[0] or 0)
    everyone = [voter(i) for i in range(POLL_VOTERS.get(int(answer), 0))]
    page = [u for u in everyone if int(u["id"]) > after][:limit]
    return 200, {"users": page}


# The server's top emoji by rank: a custom one, a Unicode one named by its character, and one nobody has.
@route("GET", r"/guilds/(\d+)/top-emojis")
def top_emojis(guild, body):
    return 200, {"items": [{"emoji_id": "8102", "emoji_rank": 1}, {"emoji_id": "🔥", "emoji_rank": 2},
                           {"emoji_id": "9999", "emoji_rank": 3}, {"emoji_id": "8101", "emoji_rank": 4}]}


# FAKE_EMOJI_PACKS=1: two packs (one gone), as `GET /emoji-packs` lists them; without it the route is Discord's 404
# for an account that doesn't have packs. Run with FAKE_VARIANTS=packs=1 for the client to ask.
@route("GET", r"/emoji-packs")
def emoji_packs(body):
    if not os.environ.get("FAKE_EMOJI_PACKS"):
        return 404, {"message": "Unknown", "code": 0}
    def emoji(id, name, available=True):
        return {"id": id, "name": name, "animated": False, "require_colons": True, "available": available,
                "managed": False, "roles": []}
    return 200, [
        {"id": "9010", "name": "Blobs", "description": "Round and friendly", "available": True,
         "representative_emoji_id": "8201", "emojis": [emoji("8201", "pack_blob"), emoji("8202", "pack_blob_cry")]},
        {"id": "9020", "name": "Retired", "description": "No longer on offer", "available": False,
         "representative_emoji_id": "8203", "emojis": [emoji("8203", "pack_old")]},
    ]


# A friend request, which the fake takes whatever it says; a message over 120 characters is refused the way
# Discord does, as `{"note": [...]}`.
@route("POST", r"/users/@me/relationships")
def send_friend_request(body):
    note = body.get("note")
    if note is not None and len(note) > 120:
        return 400, {"note": ["Notes must be 120 characters or fewer"]}
    return 204, None


@route("GET", r"/channels/(\d+)/messages/pins")
def pins(channel, body):
    if channel != "600":
        return 200, {"items": [], "has_more": False}
    lines = {m["id"]: m for m in messages(channel)}
    by_number = {str(n): snowflake(1000 + n * 100, n) for n in range(40)}
    items = [{"pinned_at": "2026-01-02T10:%02d:00.123456+00:00" % int(n), "message": dict(lines[by_number[n]], pinned=True)}
             for n in reversed(PINNED)]
    before = QUERY.get("before", [None])[0]
    if before:
        items = [i for i in items if i["pinned_at"][:19] < before[:19]]
    return 200, {"items": items[:2], "has_more": len(items) > 2}


# The inbox: every line of #general mentions the account, 25 to a page, newest first. Marked done are kept here.
MENTION_DONE = set()


@route("GET", r"/users/@me/mentions")
def mentions(body):
    before = int(QUERY.get("before", ["9" * 30])[0])
    lines = [dict(m, mentions=[ME]) for m in messages("600") if int(m["id"]) < before and m["id"] not in MENTION_DONE]
    return 200, lines[:25]


@route("DELETE", r"/users/@me/mentions/(\d+)")
def mention_done(message, body):
    MENTION_DONE.add(message)
    return 204, None


@route("POST", r"/channels/(\d+)/messages")
def send_message(channel, body):
    content = body.get("content", "")
    extra = {"tts": bool(body.get("tts")), "flags": body.get("flags", 0), "nonce": body.get("nonce"),
             "mentions": [USERS[u] for u in re.findall(r"<@!?(\d+)>", content) if u in USERS],
             "mention_roles": re.findall(r"<@&(\d+)>", content)}
    reference = body.get("message_reference")
    if reference and reference.get("type") == 1:
        # A forward: a copy of the original rides along as a snapshot, of just what `forward_only` names.
        original = find(reference.get("channel_id"), reference.get("message_id")) or {}
        only = reference.get("forward_only") or {}
        files = [a for a in original.get("attachments", [])
                 if "attachment_ids" not in only or a["id"] in only["attachment_ids"]]
        if "embed_indices" in only:
            files = []
        snapshot = {"type": 0, "content": original.get("content", ""), "timestamp": original.get("timestamp"),
                    "edited_timestamp": None, "attachments": files, "embeds": [], "flags": 0}
        extra.update(message_reference=reference, message_snapshots=[{"message": snapshot}],
                     flags=extra["flags"] | (1 << 14))
        return (200, *post(channel, ME, "", **extra))
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


@route("PATCH", r"/users/@me")
def edit_me(body):
    """The change of a password: the current one is "old-password", and Discord answers a new token."""
    if body.get("password") != "old-password":
        return 400, {"code": 50035, "message": "Invalid Form Body", "errors": {
            "password": {"_errors": [{"code": "PASSWORD_DOES_NOT_MATCH", "message": "Password does not match."}]}}}
    return 200, {**ME, "token": "fake-token-2"}


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


@route("GET", r"/sticker-packs")
def sticker_packs(body):
    return 200, {"sticker_packs": [{"id": "720", "name": "Pals", "stickers": [
        {"id": "721", "name": "hello", "format_type": 1, "pack_id": "720", "type": 1, "tags": "wave"}]}]}


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


# Upload URLs handed out, numbered from 1.
UPLOADS = [0]


@route("POST", r"/channels/(\d+)/attachments")
def upload_url(channel, body):
    """An upload URL per file, here at the fake; FAKE_UPLOAD_FAIL=too-large turns it down as Discord does a file
    over the limit."""
    if UPLOAD_FAIL == "too-large":
        return 400, {"code": 40005, "message": "Request entity too large"}
    slots = []
    for f in body.get("files", []):
        UPLOADS[0] += 1
        n = UPLOADS[0]
        slots.append({"id": f.get("id"), "upload_url": f"http://127.0.0.1:{PORT}/api/v9/upload/{n}",
                      "upload_filename": f"{n}/{f.get('filename')}"})
    return 200, {"attachments": slots}


@route("PUT", r"/upload/(\d+)")
def upload_put(n, body):
    return 200, None


@route("DELETE", r"/attachments/(\d+)/([^/]+)")
def upload_delete(n, filename, body):
    return 204, None


# What the built-ins send: the nick comes back as set, the rest are taken.
@route("PATCH", r"/guilds/(\d+)/members/@me/nick")
def change_nick(guild, body):
    return 200, {"nick": (body or {}).get("nick")}


@route("DELETE", r"/guilds/(\d+)/members/(\d+)")
def kick(guild, user, body):
    return 204, None


@route("PUT", r"/guilds/(\d+)/bans/(\d+)")
def ban(guild, user, body):
    return 204, None


@route("PATCH", r"/guilds/(\d+)/members/(\d+)")
def edit_member(guild, user, body):
    return 200, {"user": USERS.get(user, {"id": user}), "roles": [], **(body or {})}


@route("PATCH", r"/channels/(\d+)")
def edit_channel(channel, body):
    return 200, {"id": channel, **(body or {})}


@route("DELETE", r"/channels/(\d+)")
def close_channel(channel, body):
    return 200, {"id": channel}


# The thread browser's name search (`GET /channels/{id}/threads/search?name=…`): two made-up threads of #600,
# those whose name has the query. They are the server's threads from then on.
FOUND_THREADS = {"7001": "planning ideas", "7002": "weekend plans"}
FORUM_POSTS = {"7101": ("weekend plans", ["9001"]), "7102": ("plan b", ["9002"]), "7103": ("other things", [])}


@route("GET", r"/channels/(\d+)/threads/search")
def search_threads(channel, body):
    name = QUERY.get("name", [""])[0].lower()
    now = time.strftime("%Y-%m-%dT%H:%M:%S.000000+00:00", time.gmtime())
    if channel == "605":
        # The forum: its posts whose name has the query and that carry one of the tags asked for, if any.
        wanted = [t for t in QUERY.get("tag", [""])[0].split(",") if t]
        found = [tid for tid, (tname, tags) in FORUM_POSTS.items()
                 if name in tname and (not wanted or any(t in tags for t in wanted))]
        threads = [{"id": tid, "type": 11, "guild_id": "500", "parent_id": "605", "name": FORUM_POSTS[tid][0],
                    "owner_id": "301", "last_message_id": None, "message_count": 1, "member_count": 1,
                    "rate_limit_per_user": 0, "flags": 0, "applied_tags": FORUM_POSTS[tid][1],
                    "thread_metadata": {"archived": False, "locked": False, "archive_timestamp": now,
                                        "auto_archive_duration": 4320, "create_timestamp": now}} for tid in found]
        return 200, {"threads": threads, "members": [], "has_more": False, "first_messages": []}
    threads = [{"id": tid, "type": 11, "guild_id": "500", "parent_id": channel, "name": tname, "owner_id": ME["id"],
                "last_message_id": None, "message_count": 1, "member_count": 1, "rate_limit_per_user": 0, "flags": 0,
                "thread_metadata": {"archived": False, "locked": False, "archive_timestamp": now,
                                    "auto_archive_duration": 4320, "create_timestamp": now}}
               for tid, tname in FOUND_THREADS.items() if name in tname]
    for thread in threads:
        GUILD_CHANNELS.add(thread["id"])
    return 200, {"threads": threads, "members": [], "has_more": False, "first_messages": []}


# /thread's thread: a channel of the server from then on, pushed as THREAD_CREATE.
@route("POST", r"/channels/(\d+)/threads")
def create_thread(channel, body):
    if SLOWMODE and channel == "600":
        return 429, {"message": "You are being rate limited.", "code": 20016, "retry_after": SLOWMODE,
                     "global": False}
    now = time.strftime("%Y-%m-%dT%H:%M:%S.000000+00:00", time.gmtime())
    thread = {"id": snowflake_now(), "type": body.get("type", 11), "guild_id": "500", "parent_id": channel,
              "name": body.get("name", ""), "owner_id": ME["id"], "last_message_id": None, "message_count": 0,
              "member_count": 1, "rate_limit_per_user": 0, "flags": 0,
              "thread_metadata": {"archived": False, "locked": False, "archive_timestamp": now,
                                  "auto_archive_duration": body.get("auto_archive_duration", 4320),
                                  "create_timestamp": now},
              "member": {"id": None, "user_id": ME["id"], "join_timestamp": now, "flags": 1}}
    GUILD_CHANNELS.add(thread["id"])
    return 201, thread, lambda: dispatch("THREAD_CREATE", {**thread, "newly_created": True})


# A thread from a message: its id is the message's, and it is pushed as THREAD_CREATE.
@route("POST", r"/channels/(\d+)/messages/(\d+)/threads")
def create_thread_from_message(channel, message, body):
    now = time.strftime("%Y-%m-%dT%H:%M:%S.000000+00:00", time.gmtime())
    thread = {"id": message, "type": body.get("type", 11), "guild_id": "500", "parent_id": channel,
              "name": body.get("name", ""), "owner_id": ME["id"], "last_message_id": None, "message_count": 0,
              "member_count": 1, "rate_limit_per_user": 0, "flags": 0,
              "thread_metadata": {"archived": False, "locked": False, "archive_timestamp": now,
                                  "auto_archive_duration": body.get("auto_archive_duration", 4320),
                                  "create_timestamp": now},
              "member": {"id": None, "user_id": ME["id"], "join_timestamp": now, "flags": 1}}
    GUILD_CHANNELS.add(thread["id"])
    return 201, thread, lambda: dispatch("THREAD_CREATE", {**thread, "newly_created": True})


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
            if path.split("?")[0].endswith(".webm") and VIDEO:
                with open(VIDEO, "rb") as clip:
                    return self.send(200, clip.read(), "video/webm")
            if path.split("?")[0].endswith(".ogg"):
                return self.send(200, voice_file(), "audio/ogg")
            if IMAGE_FAIL:
                return self.send(404, b"")
            # What asks for WebP (`format=webp`, `.webp`) gets one: a 16x16 picture, or a three-frame animation
            # where it says `animated=true`, from Neucord's own test files.
            if query.get("format") == "webp" or path.split("?")[0].endswith(".webp"):
                name = "animated.webp" if query.get("animated") == "true" else "lossy.webp"
                with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "crates", "discord", "testdata", name), "rb") as file:
                    return self.send(200, file.read(), "image/webp")
            body = png(int(query.get("width", 1200)) // 4, int(query.get("height", 800)) // 4)
            return self.send(200, body, "image/png")
        self.answer()

    def answer(self):
        body = self.record()
        path = self.path.split("?")[0]
        QUERY.clear()
        QUERY.update(urllib.parse.parse_qs(urllib.parse.urlsplit(self.path).query))
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
                if message["op"] in (1, 40):
                    if message["op"] == 40:
                        log(kind="gateway", op=40, d=message.get("d"), at=time.time())
                    send(11)
                elif message["op"] == 2:
                    IDENTIFIES[0] += 1
                    log(kind="identify", raw=text, at=time.time())
                    send(0, ready(), "READY")
                    send(0, supplemental(), "READY_SUPPLEMENTAL")
                    threads = guild()["threads"]
                    if threads:
                        # The active threads, with the newest message of those that have one, as Discord syncs them.
                        recent = [t.pop("most_recent_message") for t in threads if "most_recent_message" in t]
                        send(0, {"guild_id": "500", "threads": threads, "members": [], "most_recent_messages": recent},
                             "THREAD_LIST_SYNC")
                    SESSION[0] = send
                    if IDENTIFIES[0] == 1 and os.environ.get("FAKE_REIDENTIFY"):
                        threading.Timer(float(os.environ["FAKE_REIDENTIFY"]), lambda: send(9, False)).start()
                else:
                    log(kind="gateway", op=message["op"], d=message.get("d"), at=time.time())
                    if message["op"] == 8:
                        member_search(send, message.get("d") or {})
                        member_check(send, message.get("d") or {})
                    if message["op"] == 37:
                        thread_member_lists(send, message.get("d") or {})
        except (EOFError, OSError):
            pass
        if SESSION[0] is send:
            SESSION[0] = None
        self.close_connection = True


def thread_member_lists(send, d):
    """FAKE_THREAD_MEMBERS=1: op 37's `thread_member_lists` is answered for each thread with its members as Discord
    sends them: Alice (a Mods member, on), Bob (idle, no role), us (no presence: offline here), and a user nobody
    has heard of (no member object, so the client can't list them)."""
    if not os.environ.get("FAKE_THREAD_MEMBERS"):
        return
    for guild, subscription in (d.get("subscriptions") or {}).items():
        for thread in subscription.get("thread_member_lists") or []:
            joined = "2026-10-28T00:00:00.000000+00:00"

            def entry(user, roles=(), status=None):
                e = {"user_id": user["id"], "id": thread, "join_timestamp": joined, "flags": 0,
                     "member": {"user": user, "roles": list(roles), "nick": None, "joined_at": JOINED}}
                if status:
                    e["presence"] = {"user": {"id": user["id"]}, "status": status, "client_status": {"desktop": status},
                                     "activities": []}
                return e
            members = [entry(ALICE, ["510"], "online"), entry(BOB, [], "idle"), entry(ME),
                       {"user_id": "999", "id": thread, "join_timestamp": joined, "flags": 0}]
            send(0, {"guild_id": guild, "thread_id": thread, "members": members}, "THREAD_MEMBER_LIST_UPDATE")


@route("DELETE", r"/channels/(\d+)/thread-members/(\d+)")
def remove_thread_member(thread, user, body):
    """Taking someone out of a thread: Discord answers 204 and tells the thread's members."""
    dispatch("THREAD_MEMBERS_UPDATE", {"id": thread, "guild_id": "500", "member_count": 2, "removed_member_ids": [user]})
    return 204, None


def member_search(send, d):
    """Op 8 with a `query`: the made-up people whose username or name starts with it, as one chunk."""
    query = d.get("query")
    if query is None:
        return
    found = [u for u in (ALICE, BOB) if u["id"] not in LEFT
             and (u["username"].startswith(query) or u["global_name"].lower().startswith(query))]
    for guild in d.get("guild_id", []):
        send(0, {"guild_id": guild, "members": [{"user": u, "roles": [], "nick": None, "joined_at": JOINED}
                                                for u in found[:d.get("limit", 10)]],
                 "not_found": [], "chunk_index": 0, "chunk_count": 1}, "GUILD_MEMBERS_CHUNK")


# When the made-up members joined: Discord's members always say.
JOINED = "2026-01-01T00:00:00.000000+00:00"


# Who has left the server, by user id: the `@` suggestions' check finds them gone.
LEFT = set(filter(None, os.environ.get("FAKE_LEFT", "").split(",")))


def member_check(send, d):
    """Op 8 with `user_ids` and a `nonce`: the members found, and whom `FAKE_LEFT` names as not found."""
    nonce, users = d.get("nonce"), d.get("user_ids")
    if nonce is None or users is None:
        return
    people = {u["id"]: u for u in (ALICE, BOB)}
    found = [{"user": people.get(u, {"id": u, "username": "u" + u}), "roles": [], "nick": None,
              "joined_at": "2026-01-01T00:00:00.000000+00:00", "flags": 0} for u in users if u not in LEFT]
    for guild in d.get("guild_id", []):
        send(0, {"guild_id": guild, "members": found, "not_found": [u for u in users if u in LEFT],
                 "chunk_index": 0, "chunk_count": 1, "nonce": nonce}, "GUILD_MEMBERS_CHUNK")


form_message()

if __name__ == "__main__":
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    server.daemon_threads = True
    print("fake discord on", PORT, flush=True)
    server.serve_forever()
