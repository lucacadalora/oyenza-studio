# Update count

How many copies of Oyenza Studio are running, from the update check every copy already makes. Nothing in the app changes: it reads `studio.oyenza.com/latest.json` about once an hour ([docs/updates.md](../../docs/updates.md)), and this Cloudflare Worker counts those reads on their way to GitHub Pages. [PRIVACY.md](../../PRIVACY.md) says what people are told.

- `worker.mjs`: the Worker, and the Durable Object that holds the day.
- `count.mjs`, `system.mjs`: the counting, with no Cloudflare in them.
- `test/*.test.mjs`: `node --test`, no dependencies.
- `test/runtime.mjs`: the Worker in Cloudflare's own runtime on this computer, against the live file (its first lines say how).

## What it does

The Worker answers `/latest.json` with GitHub Pages' reply, untouched. Counting runs after that, and if anything in it fails, only that count is lost. If the Worker throws, the request goes on to GitHub Pages. If the free allowance runs out, the route's fail-open setting skips the Worker for the rest of the UTC day.

For each UTC day it keeps these totals, for 90 days. The stats page keeps the history.

| Total | What it is |
|---|---|
| `checks` | Reads by copies. A 304 counts too. |
| `busiest_hour_checks` | Reads in the busiest clock hour: about how many copies were online at once. |
| `addresses` | Different addresses. |
| `copies` | The estimate. A copy reads at most once per clock hour, so an address with 7 reads in its busiest hour is at least 7 copies. Adds up each address's busiest hour, 50 at most per address. |
| `versions`, `systems` | The last two, per version and for Windows or Mac. A copy that updates during the day shows under both versions and once in the total. |
| `other` | Anything but the exact User-Agent of a version we released. |
| `left_out` | Copies on Microsoft's network, where our CI runs. |
| `test` | Our own checks: version `0.0.0`. |

To tell addresses apart, the day's key (32 random bytes) scrambles each one (HMAC-SHA-256, 64 bits kept). IPv6 is cut to its first 64 bits first. The address itself is never stored. At the end of the UTC day the key and the codes are deleted, so no copy can be followed from one day to the next. Observability stays off, and nothing here logs, so no address lands in a log.

Other copies aren't seen: 0.2.5 and older and Microsoft Store copies don't read the file, and 0.2.6 to 0.2.12 read it every 12 hours.

## Run it

```
export CLOUDFLARE_ACCOUNT_ID=... CLOUDFLARE_API_TOKEN=...
cd server/update-count
npx wrangler@4.151.2 deploy
npx wrangler@4.151.2 secret put READ_KEY
```

The token needs Workers Scripts on the account, and Workers Routes, DNS, Config Rules, Cache Rules and Zone Settings (read) on oyenza.com.

On Cloudflare, by hand or through its API:

- Each route's request limit failure mode is **fail open**.
- For `studio.oyenza.com`: Web Analytics off (the zone adds its beacon to pages, and the site already counts visits), and browser caching respects the origin (the zone otherwise stretches 10 minutes to 4 hours).

To read the totals, with the key:

```
curl -H "Authorization: Bearer $READ_KEY" "https://studio-update-count.<account>.workers.dev/days"
```

`?host=studio-check.oyenza.com` reads the trial name's totals, which are kept apart.

## Order of things

Each step on the maintainer's word.

1. Try it on `studio-check.oyenza.com`, a proxied name with its own route, which serves the real file. The site isn't touched.
2. The text changes first: [PRIVACY.md](../../PRIVACY.md), the README and the website's Privacy section say the check is counted.
3. Add the route `studio.oyenza.com/latest.json` in `wrangler.toml`, deploy, and turn on the proxy for `studio.oyenza.com`.
4. Check it against known traffic, then show the totals on the stats page.

## Undo it

Turn off the proxy for `studio.oyenza.com` in Cloudflare's DNS. Live within about 5 minutes, and every check goes straight to GitHub Pages again. Copies that miss a check try again an hour later.

**The date that matters: GitHub's certificate for the site ends 23 Jan 2027.** GitHub may not be able to renew it while the name points at Cloudflare. The site keeps working behind Cloudflare, since this zone accepts an expired certificate at the origin. But after that date, turning off the proxy shows visitors a browser warning until GitHub issues a new one. Decide by mid-December.
