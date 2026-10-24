# Paired phone drop: meeting server and phone page

The meeting server and the phone page for Oyen Poni's paired phone drop. The
contract between the phone page, this server and the PC is
[`docs/paired-drop.md`](../docs/paired-drop.md); read it first.

- `drop-server.mjs`: the meeting server. Node 20 or newer, no dependencies.
- `public/`: the phone page it serves (`index.html`, `app.js`, `app.css`,
  `drop-core.js`, `sw.js`, `manifest.webmanifest`, icons).
- `test/`: `node --test` tests for the server and for `drop-core.js`.

## Run it

```
node server/drop-server.mjs --port 8797
```

| Option | Default | |
|---|---|---|
| `--port <n>` | `8797` | `0` picks a free port; the start line says which |
| `--host <addr>` | `127.0.0.1` | put a proxy in front, or use `0.0.0.0` |
| `--data <dir>` | `server/data` | where `gone.json` (removed rooms, 30 days) lives |
| `--trust-proxy` | off | take the address for the rate limit from `CF-Connecting-IP` |

For the relay (TURN), set `TURN_KEY_ID` and `TURN_KEY_API_TOKEN` from a
Cloudflare Realtime TURN key. Without them `/api/ice` hands out STUN only, and
phones and PCs connect directly or not at all. The server prints one start line
and error lines; it never logs rooms, proofs, messages, names or addresses.

Test knobs (not for production): `--hold-ms` (long poll cap, 25000),
`--online-ms` (online window after a wait, 35000), `--session-ms` (phone
session life, 120000), `--rate` (requests a minute per address, 120).

## Host your own

1. Run the server on a machine you control, as a service (systemd or similar),
   with `--data` on a disk that survives restarts.
2. Put it behind HTTPS: the phone page needs a secure context for WebCrypto
   and its service worker. With Cloudflare in
   front, start it with `--trust-proxy` so each phone gets its own rate limit.
   Long polls hold for up to 25 s, which fits Cloudflare's 100 s limit.
3. Point Oyen Poni at it with `PONI_DROP_SERVER=https://your.host` on the PC.
   Pairing QR codes then carry your address.

Every answer carries a strict Content-Security-Policy; the page has no inline
scripts or styles. Memory is capped (200 000 rooms, 20 queued messages per room
or phone session, 64 KB bodies, 120 API requests a minute per address).

## Test

```
node --test server/test/*.test.mjs
```

The end-to-end test (phone page, this server and a PC emulator in headless
browsers) is in [`tools/pairdrop-e2e`](../tools/pairdrop-e2e/run_e2e.py):

```
python3 tools/pairdrop-e2e/run_e2e.py --webkit
```

(needs Python Playwright with Chromium and WebKit; on the build VM wrap it in
`vm-gate run --`). `run_interop.py` in the same folder runs the phone page
against Oyen Poni's real Rust PC side instead of the emulator, through the
`pairdrop_pc` example (build it first with
`cargo build -p poni-core --example pairdrop_pc`).

`python3 server/make_icons.py` redraws the icons from
`docs/brand/oyen-poni-logo-512.png` (the brand files are not MIT; see
`docs/brand/README.md`).
