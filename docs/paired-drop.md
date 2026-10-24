# Paired phone drop

Pair a phone with Oyen Poni once, by scanning a QR code on the Shelf. After
that the phone finds the PC on any network (same Wi-Fi, another Wi-Fi, mobile
data) whenever Oyen Poni is running, and sends files straight onto the Shelf.

Three parts work together:

| Part | Where | What it does |
|---|---|---|
| Oyen Poni | the PC (`crates/poni-core/src/pairdrop`, `crates/poni/src/services/pairdrop.rs`) | keeps the pairs, waits for phones through the meeting server, answers with a direct WebRTC link, saves the files |
| Phone page | `server/public`, served at `https://drop.oyenza.com` | pairing, your PCs (online or offline), send, save Shelf files; installs to the Home Screen; joins Android's share sheet |
| Meeting server | `server/drop-server.mjs` | tells a phone whether its PC is online and passes the two sides' connection details; hands out short-lived relay passes |

The files themselves go phone ↔ PC over a WebRTC data channel: straight over
the Wi-Fi, straight across the internet, or (when both networks block that)
through a TURN relay that only forwards encrypted bytes. No server stores a
file, a file name or a device name.

Today's QR phone drop (`poni_core::phonedrop`, same Wi-Fi, nothing through
the internet) stays as it is for phones that aren't paired.

## Pairing secret and what is derived from it

The PC makes a 16-byte random secret `S` per pairing (one pairing = one PC
and one phone). It goes to the phone only inside the QR code, in the link's
fragment, which browsers never send to a server:

```
https://drop.oyenza.com/p#k=<b64u(S)>
```

`b64u` is base64url without padding. Both sides derive, with SHA-256:

| Name | Value | Used for |
|---|---|---|
| `room` | `b64u(SHA256("oyen-drop/1 room" ‖ S))` | the address of this pair on the meeting server |
| `proof` | `b64u(SHA256("oyen-drop/1 proof" ‖ S))` | shown to the meeting server with every request; it keeps `SHA256(proof)` per room (first use wins) |
| `K` | `SHA256("oyen-drop/1 mac" ‖ S)` (32 raw bytes) | HMAC key for every signalling message |

The meeting server sees `room` and `proof`, never `S` or `K`, so it can't
make or alter a signed message. Every SDP is signed, and an SDP carries the
DTLS fingerprint of its side, so the server also can't sit in the middle of
the WebRTC link.

### Signed messages

Every message between phone and PC through the meeting server is a JSON
object with these fields:

| Field | Meaning |
|---|---|
| `t` | type (below) |
| `sid` | the phone's session: 16 random bytes, b64u (22 chars), new for every attempt |
| `ts` | sender's clock, Unix milliseconds (integer) |
| `name` | sender's device name, one line, at most 60 characters ("" when not needed) |
| `sdp` | SDP text for `offer`/`answer` ("" otherwise) |
| `mac` | `b64u(HMAC-SHA256(K, text))` |

`text` is these lines joined with `\n` (the SDP verbatim, `\r\n` and all):

```
oyen-drop/1
<room>
<t>
<sid>
<ts as decimal>
<name>
<sdp>
```

A receiver drops a message whose MAC doesn't match, whose `ts` is more than
10 minutes away from its own clock, or that it has seen before (same `sid`,
`t` and `ts`, remembered for 15 minutes).

| `t` | From | When | Reply |
|---|---|---|---|
| `pair` | phone | right after scanning the QR (name = the phone's) | `welcome` with the PC's name |
| `offer` | phone | to connect; `sdp` = full offer with every candidate (no trickle) | `answer` (sdp + PC name) or `busy` |
| `answer` | PC | | |
| `busy` | PC | three phones already connected | |
| `welcome` | PC | | |

Test vector (also in the Rust and JavaScript tests):

```
S     = 000102030405060708090a0b0c0d0e0f (hex)
b64u  = AAECAwQFBgcICQoLDA0ODw
room  = geDM0jEhk4eiOz_li-FtORW2YlPBbebr7NsHjLkHI_4
proof = 6JmvhsXX2_bzKYvHQ_iFgLcPnlG-m636-rWXAI5Vubo
K     = PbxVzGOl5YKxg6Fd-uSD6Yqr9716j9MddvFbISuFxL4 (b64u)
mac of t=offer, sid=AAECAwQFBgcICQoLDA0ODw, ts=1790000000000, name="Pixel 10",
       sdp="v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\n"
      = 3QYhMYuCNPzit2tYBw0ejDAQsE1vxoO4DQERkznV8o8
```

## Meeting server API

JSON in, JSON out, every route `POST` unless noted, bodies at most 64 KB.
`{room, proof}` registers the room on first use; a different proof for a
known room is refused (`bad`), and a room the PC removed is `gone`.

| Route | Body | Answer |
|---|---|---|
| `/api/pc/wait` | `{rooms: [{room, proof}] (1-32), client}` | holds up to 25 s; `{msgs: [{room, msg}], bad: [room], gone: [room]}`. Each message is handed out once. While a wait is open, and for 35 s after it ends, the PC counts as online in those rooms. `client` is the PC's own random id (22-char b64u, new each start); a newer wait from the same client ends the older one at once |
| `/api/pc/kick` | `{client}` | `{ok: true}`; ends that client's open wait at once (it answers with what it has, maybe nothing). The PC calls it when its rooms change (a pairing QR appears, a pairing completes, a phone is removed) so the next wait carries the new list straight away |
| `/api/pc/send` | `{room, proof, msg}` | `{ok: true}`; the message goes to the phone session `msg.sid` (`404 no-session` when it has gone) |
| `/api/pc/unpair` | `{room, proof}` | `{ok: true}`; the room becomes `gone` |
| `/api/phone/status` | `{rooms: [{room, proof}] (1-32)}` | `{rooms: [{room, state: "online" \| "offline" \| "gone" \| "bad", seen}]}`, `seen` = Unix ms the PC was last online or null |
| `/api/phone/send` | `{room, proof, msg}` | `{ok: true}`, or `{ok: false, error: "offline"}` when the PC isn't online (nothing is queued then). Opens the session `msg.sid` for 2 minutes |
| `/api/phone/wait` | `{room, proof, sid}` | holds up to 25 s; `{msgs: [msg]}` (each handed out once) |
| `/api/ice` | `{room, proof}` | `{iceServers: [...]}` for `RTCPeerConnection`: STUN always, TURN with credentials for 1 hour when the server has a relay key |
| `GET /api/health` | | `{ok: true}` |

Errors: `400 bad-request`, `403 bad` (wrong proof), `410 gone`, `413 too-large`,
`429 slow-down` (per address: 120 requests a minute), `404 no-session`.
The server keeps everything in memory except the `gone` list.

## The data channel

The phone makes the offer with one data channel labelled `oyen-drop`
(ordered, reliable). Text frames are JSON; binary frames are file bytes.

| From | Message | Meaning |
|---|---|---|
| phone | `{"t":"hello","v":1,"name":"Pixel 10"}` | first message |
| PC | `{"t":"hello","v":1,"name":"RTX-5090-Astral","auto":true,"shelf":[{"id":0,"name":"a.pdf","size":274000}]}` | `auto`: files land without asking |
| PC | `{"t":"shelf","files":[...]}` | the Shelf changed |
| phone | `{"t":"send","files":[{"name":"IMG_1.jpg","size":123}]}` | 1-500 files, at most 64 GB together |
| PC | `{"t":"ok"}` or `{"t":"no","why":"busy"\|"too-many"\|"too-large"\|"declined"}` | |
| phone | binary frames | every file's bytes in the order listed, nothing in between; the announced sizes split the stream. Frames of at most 64 KB (16 KB recommended) |
| PC | `{"t":"got","n":<bytes so far>}` | progress, at most every 250 ms |
| PC | `{"t":"saved","files":<n>}` | every file is in `Downloads\Oyen Poni` and on the Shelf |
| PC | `{"t":"failed","why":"size"\|"disk"\|"stopped"}` | nothing from this send was kept |
| phone | `{"t":"get","id":0}` | save a Shelf file on the phone |
| PC | `{"t":"file","id":0,"name":"a.pdf","size":274000}` then binary frames totalling `size` | or `{"t":"nofile","id":0}` |
| phone | `{"t":"bye"}` | done; either side may also just close |

Files are staged and moved into the inbox only when every byte of every file
arrived, under names made safe for Windows (`poni_core::phonedrop::names`).

## Hosting your own meeting server

`server/` has no dependencies beyond Node 20+:

```
node server/drop-server.mjs --port 8797
```

Put it behind HTTPS (the phone page needs a secure context). For the relay,
set `TURN_KEY_ID` and `TURN_KEY_API_TOKEN` from a Cloudflare Realtime TURN key;
without them, phones and PCs connect directly or not at all. Oyen Poni reads
the server address from the environment variable `PONI_DROP_SERVER`
(default `https://drop.oyenza.com`).
