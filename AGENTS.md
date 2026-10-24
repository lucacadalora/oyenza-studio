# Notes for coding agents

Codex, Claude Code, Kimi or any other agent working in this repo: read this before you change the app, cut a release or change the website.

## Keep it light

Oyen Poni sits on screen all day, so how little it costs is part of the product. The maintainer's rule (24 Oct 2026): keep it very light on CPU, memory and download size, and fast, however many features it gets. Every change follows these:

- **Nothing runs at rest unless the person switched it on or its page is open.** No new thread, timer, poll or network call that keeps going while the island is idle. React to Windows' own notifications where there are some. When something has to repeat, give the interval a reason in a comment and stop it as soon as nothing needs it.
- **Anything that repeats rests when nobody is there:** while the session is locked, the display is off or battery saver is on. The app already knows all three (`App::locked`, `App::display_off`, `st.power.saver`), and ambient animation already pauses for the first two.
- **Motion costs a frame every refresh.** Something that moves by itself on the closed island (the equalizer, Oyen's notes) redraws for as long as it shows. Add one only where people watch it, and let it stop.
- **Load heavy Windows parts when the feature first needs them, not at start.** Performance counters, graphics devices and WinRT classes bring their own libraries: the System page's counters alone add about 6 MB once loaded.
- **Big data is an add-on, never part of poni.exe.** Dictionaries, sounds, place lists and models go on a GitHub pre-release and download when first needed, checked against the size and SHA-256 in `poni_core::addons` (`addons/`, `services/addons.rs`). `include_bytes!` is for small things the app can't start without, such as the icon and the Outfit font.
- **A new crate needs a reason and its size.** Build with `CARGO_PROFILE_RELEASE_STRIP=false cargo wbuild` before and after, and compare `x86_64-w64-mingw32-nm --size-sort -S -C` totals per crate. Code that runs for moments (a handshake, a parser, a file format) can be built for size in the workspace `Cargo.toml` (`[profile.release.package.<name>] opt-level = "s"`). Code that runs every frame stays at full speed.
- **Measure CPU in cycles, not milliseconds.** `TotalProcessorTime`, `GetProcessTimes` and Task Manager count in 15.6 ms clock ticks, so a reading that takes a fraction of a millisecond shows as zero. Use `QueryProcessCycleTime` or `QueryThreadCycleTime`.
- **Every PR says what it costs:** download size (the installer in the `oyen-poni-setup` artifact), memory at rest (`smoke-out/metrics.txt` in the `poni-screens` artifact), and anything new that runs at rest or on every frame. "Nothing new" is a fine answer.

## Every release updates the website

The landing page, https://lucacadalora.github.io/poni/, is `site/index.html`. The Website workflow (`.github/workflows/pages.yml`) publishes it whenever `site/` or `docs/brand/` changes on main. A release changes neither, so it never updates the page by itself. Every release ships with a website update, and a release isn't done until the page shows it.

Two things on the page follow the latest release on their own: the Download buttons (`releases/latest/download/Oyen-Poni-Setup.exe` for Windows, `releases/latest/download/Oyen-Poni-Mac.dmg` for Mac) and the download line's version, sizes and notes link, which the page reads from the GitHub API when it loads. Everything else is written by hand:

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

The version lives in several files: copy the last "Release x.y.z" commit (`git log --grep '^Release '`). Add `docs/release-notes/vX.md`, which becomes the release text, and the website change above. Then push an annotated tag `vX`. The `ci` workflow builds, tests and publishes the release from the tag. The `mac` workflow builds the Mac app from the same tag and, once the release is up, adds `Oyen-Poni-X-mac.dmg` and `Oyen-Poni-Mac.dmg` (the name the Mac download buttons use) to it.
