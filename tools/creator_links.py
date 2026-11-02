#!/usr/bin/env python3
"""Creator links: studio.oyenza.com/<code> for each code in site/creators.json (docs/creators.md).

Each link is a tiny page that forwards to the landing page with the creator's tags (utm_source=<code>,
utm_medium=creator, utm_campaign=<campaign>), so the visit counter (site/track.js) counts that creator's
visits, sends to a computer and download taps. The page carries the landing page's link preview, and is
hidden from search engines (noindex, canonical to the landing page).

The codes are public, since they are in the address, so creators.json holds codes only; who is who stays
outside the repo. A bad code stops the build, so a typo fails the deploy instead of breaking a link.

Run by tools/build_site.sh, after site/ is copied:  python3 tools/creator_links.py site/creators.json _site
"""
import html
import json
import re
import sys
from pathlib import Path

CODE = re.compile(r"[a-z0-9]{2,24}")
CAMPAIGN = re.compile(r"[a-z0-9][a-z0-9-]{0,39}")
# Never a code, besides every name already at the top of the site: pages the site may want one day.
RESERVED = {"api", "app", "blog", "brand", "c", "docs", "download", "downloads", "get", "go", "help", "mac",
            "new", "pc", "pricing", "privacy", "pro", "send", "store", "windows"}

PAGE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<meta name="robots" content="noindex">
<link rel="canonical" href="https://studio.oyenza.com/">
{preview}
<meta http-equiv="refresh" content="0; url={href}">
<script>location.replace({target} + location.hash)</script>
</head>
<body><a href="{href}">Oyenza Studio</a></body>
</html>
"""


def die(msg):
    print(f"creator_links: {msg}", file=sys.stderr)
    sys.exit(1)


def main(src, out):
    try:
        data = json.loads(Path(src).read_text())
    except (OSError, ValueError) as e:
        die(f"can't read {src}: {e}")
    campaign = data.get("campaign", "")
    creators = data.get("creators", [])
    if not CAMPAIGN.fullmatch(str(campaign)):
        die(f'campaign "{campaign}": a-z, 0-9 and "-", up to 40 characters')
    if not isinstance(creators, list):
        die('"creators" must be a list')

    out = Path(out)
    taken = {p.name.lower() for p in out.iterdir()}
    index = (out / "index.html").read_text()
    title = re.search(r"<title>([^<]*)</title>", index)
    preview = "\n".join(m.group(0) for m in re.finditer(
        r'<meta (?:property="og:[^"]+"|name="twitter:[^"]+"|name="description")[^>]*>', index))

    made = []
    for c in creators:
        c = c if isinstance(c, dict) else {}
        code, camp = str(c.get("code", "")), str(c.get("campaign", campaign))
        if not CODE.fullmatch(code):
            die(f'code "{code}": 2 to 24 characters, a-z and 0-9 only')
        if code in made:
            die(f'code "{code}" is listed twice')
        if code in taken or code in RESERVED:
            die(f'code "{code}" is a name the site uses or keeps; pick another')
        if not CAMPAIGN.fullmatch(camp):
            die(f'campaign "{camp}" for "{code}": a-z, 0-9 and "-", up to 40 characters')
        target = f"/?utm_source={code}&utm_medium=creator&utm_campaign={camp}"
        page = PAGE.format(title=title.group(1) if title else "Oyenza Studio", preview=preview,
                           href=html.escape(target), target=json.dumps(target))
        (out / code).mkdir()
        (out / code / "index.html").write_text(page)
        made.append(code)
    print(f"creator links: {len(made)}" + (f" ({', '.join(made)})" if made else ""))


if __name__ == "__main__":
    if len(sys.argv) != 3:
        die("usage: creator_links.py site/creators.json _site")
    main(sys.argv[1], sys.argv[2])
