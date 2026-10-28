# Notes for coding agents

Codex, Claude Code, Kimi or any other agent working in this repo: read this before you change the app, cut a release or change the website.

## Keep it light

Oyenza Studio sits on screen all day, so how little it costs is part of the product. The maintainer's rule (24 Oct 2026): keep it very light on CPU, memory and download size, and fast, however many features it gets. Every change follows these:

- **Nothing runs at rest unless the person switched it on or its page is open.** No new thread, timer, poll or network call that keeps going while the island is idle. React to Windows' own notifications where there are some. When something has to repeat, give the interval a reason in a comment and stop it as soon as nothing needs it.
- **Anything that repeats rests when nobody is there:** while the session is locked, the display is off or battery saver is on. The app already knows all three (`App::locked`, `App::display_off`, `st.power.saver`), and ambient animation already pauses for the first two.
- **Motion costs a frame every refresh.** Something that moves by itself on the closed island (the equalizer, Oyen's notes) redraws for as long as it shows. Add one only where people watch it, and let it stop.
- **Load heavy Windows parts when the feature first needs them, not at start.** Performance counters, graphics devices and WinRT classes bring their own libraries: the System page's counters alone add about 6 MB once loaded.
- **Big data is an add-on, never part of poni.exe.** Dictionaries, sounds, place lists and models go on a GitHub pre-release and download when first needed, checked against the size and SHA-256 in `poni_core::addons` (`addons/`, `services/addons.rs`). `include_bytes!` is for small things the app can't start without, such as the icon and the Outfit font.
- **A new crate needs a reason and its size.** Build with `CARGO_PROFILE_RELEASE_STRIP=false cargo wbuild` before and after, and compare `x86_64-w64-mingw32-nm --size-sort -S -C` totals per crate. Code that runs for moments (a handshake, a parser, a file format) can be built for size in the workspace `Cargo.toml` (`[profile.release.package.<name>] opt-level = "s"`). Code that runs every frame stays at full speed.
- **Measure CPU in cycles, not milliseconds.** `TotalProcessorTime`, `GetProcessTimes` and Task Manager count in 15.6 ms clock ticks, so a reading that takes a fraction of a millisecond shows as zero. Use `QueryProcessCycleTime` or `QueryThreadCycleTime`.
- **Every PR says what it costs:** download size (the installer in the `oyenza-studio-setup` artifact), memory at rest (`smoke-out/metrics.txt` in the `poni-screens` artifact), and anything new that runs at rest or on every frame. "Nothing new" is a fine answer.

## Names that stay

The app is Oyenza Studio; it was Oyen Poni until 0.2.0. Only the names people see changed. `poni.exe`, its folders, the uninstall key, the Run value, `Local\Poni.Island`, the Mac bundle id `com.oyenza.poni`, the crates, the `PONI_*` variables and the Oyenza sign-in app id `poni` stay as they are, so older copies update in place and stay signed in. Every release also carries `Oyen-Poni-Setup.exe` and `Oyen-Poni-Mac.dmg` next to the new names, because links to them are out there.

Never create a repository named `lucacadalora/poni`: GitHub forwards that old name to this repository, and the add-on downloads of the 0.1.x versions go through it.

## Every release updates the website

The landing page, https://studio.oyenza.com/, is `site/index.html`. The Website workflow (`.github/workflows/pages.yml`) publishes it whenever `site/` or `docs/brand/` changes on main. A release changes neither, so it never updates the page by itself. Every release ships with a website update, and a release isn't done until the page shows it.

Two things on the page follow the latest release on their own: the Download buttons (`releases/latest/download/Oyenza-Studio-Setup.exe` for Windows, `releases/latest/download/Oyenza-Studio-Mac.dmg` for Mac) and the download line's version, sizes and notes link, which the page reads from the GitHub API when it loads. Everything else is written by hand:

- Set the fallback version, `<span data-release="version">`, to the new version, and the fallback size next to it. The checks below read this number to tell which release the page describes, so change it once the rest of the page is up to date.
- The page has one "New in X." band (`#new`): rename it to the new version. It shows only the five newest changes, as cards with the app in them: a shot from `tools/site_shots.py` or a Mac render (`mac-renders` artifact) on the hero's wallpaper, or a small drawing of the app where there is no picture; never plain-text tiles. Put a card first for each change people will notice. The cards that drop out of the five become lines in the "Everything new" sheet (`#m-new`), under "New in X" for the release that brought them, with their section's sidebar icon. Never add a second band. The maintainer's rule (28 Oct 2026): the landing page highlights the newest five, the rest lives in the sheet. Fix copy the release makes out of date, such as "coming soon" lines.
- Retake the screenshots the release changed with `tools/site_shots.py`, from the self-test.
- If the share card changes, redraw it with `tools/make_share_card.py` and bump the `?v=` on its `og:image` and `twitter:image` URLs. Sites that show link previews cache images by URL.

### When to merge it

Put the website change in the release PR, or merge it to main just before you push the tag. The workflows take care of the timing:

1. The Website workflow holds a page that names a version newer than the latest release, so nothing goes live before people can download it.
2. Once the tag's `ci` run has published the release, its `site` job runs `tools/ci-site-check.sh release`. If the page on main names the new version, the job starts the Website workflow, which puts the page live.
3. If the page still names an older version, the job opens an issue, "Landing page not updated for vX", with this checklist. The first website deploy that names vX closes it.

## Cutting a release

Merges, tags and releases happen only when the maintainer says so.

The version lives in several files: copy the last "Release x.y.z" commit (`git log --grep '^Release '`). `Cargo.lock` holds it once for each of the workspace's crates (poni, poni-core and studio-usage), and CI's `cargo fetch --locked` fails if one is left behind. Add `docs/release-notes/vX.md`, which becomes the release text, and the website change above. Then push an annotated tag `vX`. The `ci` workflow builds, tests and publishes the release from the tag. The `mac` workflow builds the Mac app from the same tag and, once the release is up, adds `Oyenza-Studio-X-mac.dmg` and `Oyenza-Studio-Mac.dmg` (the name the Mac download buttons use) to it.

On a tag, the `mac` workflow's `sign` job signs the DMG with the Developer ID, has Apple notarize it and staples it, when the `release` environment holds the signing secrets ([docs/mac-signing.md](docs/mac-signing.md)); the release then ships that DMG. Without the secrets, or when signing fails, it ships the ad-hoc DMG (a failure shows as a warning in the run). Never move those secrets out of the `release` environment or print them. The first release that ships a signed DMG also updates the README's Mac install note and the website's Mac section: no more "Open Anyway".

Every deploy of the website also carries `studio.oyenza.com/latest.json`, the signed file every copy of the app reads to learn that a new version is out ([docs/updates.md](docs/updates.md)). It describes GitHub's latest release, so a release updates it without a step of yours. The `UPDATE_FEED_KEY` secret that signs it stays in the `release` environment, like the Mac signing secrets: never print it or move it.

To try the release path without releasing, run `ci` by hand with `release_dry_run` on (`gh workflow run ci.yml --ref <branch> -f release_dry_run=true`). It runs every job a tag runs and publishes nothing: no tag, release or issue, and no website deploy. Its summary says what a tag would publish and what would happen to the website.
