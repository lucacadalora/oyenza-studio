# Creator links

Every creator who makes videos about Oyenza Studio gets a short address to say and to put in their bio:
**studio.oyenza.com/&lt;code&gt;**, for example studio.oyenza.com/budi.

The address is a tiny page that forwards to the landing page with the creator's tags:

```
https://studio.oyenza.com/?utm_source=budi&utm_medium=creator&utm_campaign=pilot-1
```

The site's visit counter (`site/track.js`) keeps those tags with the visit, so the creator's numbers come
out per code: visits on a phone and on a computer, links sent to a computer from the phone card (they keep
the tags), and Windows and Mac download taps. The app itself counts nothing, so installs are not counted.

## Add a creator

Add one line to `site/creators.json` and merge it to main. The Website workflow deploys it in about a
minute.

```json
{
  "campaign": "pilot-1",
  "creators": [
    { "code": "budi" },
    { "code": "rara", "campaign": "pilot-2" }
  ]
}
```

- **Codes only.** The repository is public and the code is in the address. Keep who is who (names,
  handles, rates) in your own sheet, never in this file.
- A code is 2 to 24 characters, `a-z` and `0-9` only. It can't be a name the site already uses
  (`assets`, `brand`, `places`, files at the top) or keeps for later (`get`, `pc`, `pricing`, `privacy`,
  `store` and a few more in `tools/creator_links.py`). A bad code stops the build, so the deploy fails
  instead of a link breaking.
- `campaign` is the default for every code; a creator's own `campaign` overrides it.
- Keep old codes in the list after a campaign ends. Removing one makes its address a 404 for anyone who
  still has the video.

The forwarding pages are made by `tools/creator_links.py`, which `tools/build_site.sh` runs. They carry
the landing page's link preview and `noindex`, so search engines list only the landing page.
