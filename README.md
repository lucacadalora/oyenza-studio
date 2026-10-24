<p align="center"><img src="docs/brand/oyen-poni-logo.svg" width="96" alt="Oyen Poni logo: an orange tile with the black island at the top playing music, and a white Oyen below"></p>

<h1 align="center">Oyen Poni</h1>

<p align="center"><b>A living notch for Windows 10 and 11.</b><br>Music with synced lyrics, a focus timer, water reminders, a file shelf, tasks and your clipboard, one hover away.</p>

<p align="center">
  <a href="https://github.com/lucacadalora/poni/releases/latest/download/Oyen-Poni-Setup.exe"><b>Download for Windows</b></a>
  &nbsp;·&nbsp;
  <a href="https://lucacadalora.github.io/poni/"><b>Website</b></a>
  &nbsp;·&nbsp;
  <a href="#install">Install notes</a>
</p>

<p align="center"><img src="docs/brand/readme-hero.webp" width="880" alt="The open island hanging from the top of the screen, showing the Music page with synced lyrics, next to Oyen peeking over a playing island"></p>

A small black island hangs from the top of your screen. It shows what's happening (music, timers, volume, headphones connecting), and when you point at it, it opens into a set of quick pages.

Native Rust, no web view, no runtime, around 1.7 MB, and **zero CPU while nothing is moving**. It draws on the GPU, in step with your display's refresh rate (60, 144 or 240 Hz). No accounts, no tracking: your data stays on your PC.

| Collapsed | |
|---|---|
| ![Music playing](docs/screenshots/03-media-activity.png) | ![Music and a focus timer](docs/screenshots/05-media-timer.png) |
| ![Volume](docs/screenshots/06-volume-hud.png) | ![Headphones connected](docs/screenshots/08-bluetooth-hud.png) |
| ![Water reminder with a one-click action](docs/screenshots/09-water-notice.png) | |

## What it does

**While collapsed, the island shows live activities:**
- Album art and an equalizer while music plays. The title peeks for a moment when the song changes.
- A focus-timer countdown.
- Pop-ups for volume, charging and Bluetooth headsets, with battery level.
- Reminders with one-click actions, such as **+250 ml** on the water reminder.

**Point at the island (or click it) to open it.** Scroll over the header to switch pages.

<table>
<tr><td width="84" align="center"><img src="docs/brand/art/poses/oyen-music.webp" width="64" alt=""></td><td><b>Music</b><br>Controls for whatever is playing (Spotify, Apple Music, YouTube in your browser, any app in Windows' media controls), with time-synced lyrics from <a href="https://lrclib.net">LRCLIB</a>, or Apple Music's own lyrics if you add <a href="#apple-music-lyrics">your Apple Music token</a></td></tr>
<tr><td width="84" align="center"><img src="docs/brand/art/poses/oyen-focus.webp" width="64" alt=""></td><td><b>Focus</b><br>A Pomodoro timer that keeps counting in the island</td></tr>
<tr><td width="84" align="center"><img src="docs/brand/art/poses/oyen-water.webp" width="64" alt=""></td><td><b>Water</b><br>A daily goal, one-tap logging, a week of history and gentle reminders</td></tr>
<tr><td width="84" align="center"><img src="docs/brand/art/poses/oyen-shelf.webp" width="64" alt=""></td><td><b>Shelf</b><br>Drop files on the island to keep them handy; click to open or drag them back out</td></tr>
<tr><td width="84" align="center"><img src="docs/brand/art/poses/oyen-tasks.webp" width="64" alt=""></td><td><b>Tasks</b><br>A tiny to-do list</td></tr>
<tr><td width="84" align="center"><img src="docs/brand/art/poses/oyen-clipboard.webp" width="64" alt=""></td><td><b>Clipboard</b><br>Recent copied text, kept in memory only. It skips anything password managers mark as private.</td></tr>
<tr><td width="84" align="center"><img src="docs/brand/art/poses/oyen-today.webp" width="64" alt=""></td><td><b>Today</b><br>Date and calendar, plus CPU, memory, disk and battery</td></tr>
</table>

| | |
|---|---|
| ![Focus](docs/screenshots/11-page-focus.png) | ![Water](docs/screenshots/12-page-water.png) |
| ![Shelf](docs/screenshots/13-page-shelf.png) | ![Tasks](docs/screenshots/15-page-tasks.png) |
| ![Clipboard](docs/screenshots/16-page-clipboard.png) | ![Today](docs/screenshots/17-page-today.png) |

### The Oyen theme

The island stays black by default. Switch on the Oyen theme (right-click › **Theme › Oyen**) and Oyen, the Oyenza cat, moves in: he peeks over the island's edge while it rests, watches your pointer when it comes near, sits beside the clock and bobs along while music plays. He sleeps from midnight to five, and his orange becomes the accent colour. He is drawn from the Oyenza logo, not a picture, so he stays sharp at any scaling, and he only moves for a moment now and then, so the app still uses no CPU at rest.

![Classic and the Oyen theme side by side: resting, with the clock, and playing music](docs/screenshots/oyen-theme.png)

Right-click the island or the tray icon for options:
- the theme: Classic or Oyen
- the idle look: clock, eyes (Oyen in his theme), minimal, or hidden until you hover
- lyrics, water reminders, clipboard history and sounds
- hide in full-screen apps
- start with Windows, and quit

## Install

<img src="docs/brand/art/oyen-poni-peek.webp" width="260" align="right" alt="Oyen peeking over the island while it plays music">

**[Download Oyen-Poni-Setup.exe](https://github.com/lucacadalora/poni/releases/latest/download/Oyen-Poni-Setup.exe)** and run it. It installs just for you and needs no admin rights. Older versions and a portable zip are on the [Releases](https://github.com/lucacadalora/poni/releases) page.

Builds aren't code-signed yet, so Windows SmartScreen may say "Windows protected your PC". Click **More info → Run anyway**.

Your data lives in `%APPDATA%\Poni\`, and cache plus log in `%LOCALAPPDATA%\Poni\`. Uninstalling removes the app, cache and log, and keeps your data.

## Apple Music lyrics

Lyrics come from LRCLIB by default. With an Apple Music subscription, Oyen Poni can show Apple Music's own synced lyrics instead: the ones the Apple Music app shows. This works whichever app is playing. LRCLIB stays the backup for songs Apple has no synced lyrics for.

It needs your Apple Music token, copied once from the web player:

1. Open [music.apple.com](https://music.apple.com) in Chrome or Edge and sign in.
2. Press F12, open **Application** → **Cookies** → `https://music.apple.com`, and copy the value of `media-user-token`.
3. Right-click the island (or the tray icon) → **Apple Music lyrics** → **Use token from clipboard**.

Oyen Poni checks the token with Apple and says whether it worked. It stores the token encrypted for your Windows account (`%LOCALAPPDATA%\Poni\apple-music-token.bin`) and clears it from the clipboard. If Windows clipboard history (Win+V) is on, delete it there too. A token lasts for months; when Apple stops accepting it, Oyen Poni says so and you copy a fresh one. **Remove token** in the same menu turns this off.

With a token saved, Oyen Poni sends each song's title and artist to Apple to find its lyrics. It uses the Apple Music web player's own API, which Apple doesn't publish for other apps, so it can stop working when Apple changes its site. LRCLIB keeps working either way.

## Build

Oyen Poni is a Cargo workspace:

| Crate | What |
|---|---|
| `crates/poni-core` | Pure logic (springs, lyrics parsing, Pomodoro, water log, tasks, settings…). Unit-tested on any OS. |
| `crates/poni` | The Windows app: Win32 + Direct2D + WinRT through [windows-rs](https://github.com/microsoft/windows-rs). |

On Windows, with Rust stable and the MSVC toolchain:

```
cargo test -p poni-core
cargo build -p poni --release        # target\release\poni.exe
```

To cross-build from macOS or Linux, install `mingw-w64` and add the target (`rustup target add x86_64-pc-windows-gnu`):

```
cargo wcheck                          # type-check the Windows app
cargo wbuild                          # target/x86_64-pc-windows-gnu/release/poni.exe
NSIS_HOST=user@linux-box tools/package.sh   # exe + portable zip + installer into dist/
```

### Testing without clicking around

`poni.exe --selftest <dir>` runs a scripted tour of every state with fake data and writes a screenshot of each one, plus `report.json` with layout and timing checks. The exit code is non-zero on failure. It also works under Wine on Linux (`tools/run-selftest-wine.sh`), which is how the screenshots above were made, so the font is a stand-in for Segoe UI.

| Variable | Effect |
|---|---|
| `PONI_SELFTEST_DPI=144` | Render at 150% scaling (also 120 and 192) |
| `PONI_SELFTEST_ANIM=1` | Also record the transitions frame by frame (plus each frame's screen position in `anim-<name>.json`) |
| `PONI_RENDERER=software` | Draw in software instead of on the GPU (also outside the self-test, for troubleshooting) |

![The expand animation, frame by frame](docs/screenshots/expand-motion.png)

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit: threads, the rendering model, the page and service contracts, and data files.

### Website

The website is `site/index.html`, published with GitHub Pages by `.github/workflows/pages.yml` whenever `site/` or `docs/brand/` changes on main. Preview it with `tools/build_site.sh && python3 -m http.server -d _site 8000`. Its island screenshots come from the self-test at 200 % in both themes (`tools/site_shots.py`).

## Status

Early, but in daily use on Windows 11. Most rough edges are in places a test machine can't fully exercise: specific media players, Bluetooth devices, and multi-monitor setups. Issues and pull requests are welcome. Please attach `%LOCALAPPDATA%\Poni\poni.log` when something misbehaves.

## Credits

- Synced lyrics come from [LRCLIB](https://lrclib.net), a free and open lyrics database. Oyen Poni fetches them only while a song plays and caches them on your PC.
- With your token, lyrics can also come from [Apple Music](https://music.apple.com) (see [Apple Music lyrics](#apple-music-lyrics)).
- Built with [windows-rs](https://github.com/microsoft/windows-rs) and [serde](https://serde.rs).

## License

The code is [MIT](LICENSE). The Oyen Poni name, its logo and the Oyen character (the files in `docs/brand`, `installer/art` and the app icon) are not covered by the MIT license, so please don't reuse them in other projects.
