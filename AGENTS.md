# Notes for coding agents

Codex, Claude Code, Kimi or any other agent working in this repo: read this before you cut a release or change the website.

## Every release updates the website

The landing page, https://lucacadalora.github.io/poni/, is `site/index.html`. The Website workflow (`.github/workflows/pages.yml`) publishes it whenever `site/` or `docs/brand/` changes on main. A release changes neither, so it never updates the page by itself. Every release ships with a website update, and a release isn't done until the page shows it.

Two things on the page follow the latest release on their own: the Download buttons (`releases/latest/download/Oyen-Poni-Setup.exe`) and the download line's version, size and notes link, which the page reads from the GitHub API when it loads. Everything else is written by hand:

- Set the fallback version, `<span data-release="version">`, to the new version, and the fallback size next to it. The checks below read this number to tell which release the page describes, so change it once the rest of the page is up to date.
- Add a "New in X" section for each change people will notice, and fix copy the release makes out of date, such as "coming soon" lines.
- Retake the screenshots the release changed with `tools/site_shots.py`, from the self-test.
- If the share card changes, redraw it with `tools/make_share_card.py` and bump the `?v=` on its `og:image` and `twitter:image` URLs. Sites that show link previews cache images by URL.

### When to merge it

Put the website change in the release PR, or merge it to main just before you push the tag. The workflows take care of the timing:

1. The Website workflow holds a page that names a version newer than the latest release, so nothing goes live before people can download it.
2. Once the tag's `ci` run has published the release, its `site` job runs `tools/ci-site-check.sh release`. If the page on main names the new version, the job starts the Website workflow, which puts the page live.
3. If the page still names an older version, the job opens an issue, "Landing page not updated for vX", with this checklist. The first website deploy that names vX closes it.

## Cutting a release

Merges, tags and releases happen only when the maintainer says so.

The version lives in several files: copy the last "Release x.y.z" commit (`git log --grep '^Release '`). Add `docs/release-notes/vX.md`, which becomes the release text, and the website change above. Then push an annotated tag `vX`. The `ci` workflow builds, tests and publishes the release from the tag.
