# Oyenza Studio architecture

Oyenza Studio is a "living notch" for Windows 10/11: a black island that hangs from the
top-centre of the screen, shows live activities (media, timers, pop-ups) while
collapsed, and expands on hover into pages (Music + synced lyrics, Today,
Shelf, Tasks, Focus and Oyenza, plus the add-ons' Health, System, Script,
Prayer and Stocks pages once they are switched on). The Shelf's Phone drop serves a
small web page on the local network so a phone can send files to it.

It was called Oyen Poni until 0.2.0, and plain Poni before that. The code keeps the
old short name where people don't see it: the crates (`poni`, `poni-core`), `poni.exe`,
its folders (`%LOCALAPPDATA%\Programs\Poni`, `%APPDATA%\Poni`), the uninstall key and
Run value, `Local\Poni.Island`, the Mac bundle id `com.oyenza.poni` and the `PONI_*`
variables. Changing those would break upgrades and settings, so they stay.

## Crates

| Crate | What | Builds on |
|---|---|---|
| `crates/poni-core` | Pure logic + models: geometry/colour, springs, LRC parsing, LRCLIB URL/JSON, Apple Music lyrics (web-player token, API replies, TTML), romaji under Japanese, Korean and Chinese lyrics (`romanize/`; Japanese kanji come from Windows' reading analyzer, or from Oyenza Studio's own dictionary, `romanize/ja_dict.rs` with `data/ja-readings.dic.zst`, built by `tools/make_ja_dict.py`, which both apps download as the Japanese readings add-on), player metadata fix-ups, Pomodoro, the steps snapshot Oyenza sends (`health.rs`), tasks, shelf, settings, JSON store, SVG path parser, Oyen's face (`oyen/`, a port of the Oyenza app's built face, checked against its Swift port by `oyen/swift_check.rs`), Phone drop (`phonedrop/`: the LAN web server, the phone page, safe file names, QR codes), add-ons (`addons.rs`: each add-on's files with their size and SHA-256, and what the Add-ons page says; `sha256.rs` checks them), prayer times (`prayer/`: Adhan's sun math ported from adhan-js, with Kemenag RI's rules for Indonesia, Umm al-Qura for Saudi Arabia and each country's method elsewhere, checked against Kemenag's published 2026 schedule, Oyen Umroh's engine and adhan-js itself; Kemenag's places are `addons/sholat/places.json`; the world's cities are small files on the website, `site/places/1/`, built by `tools/make_world_places.py` and read by `prayer/world.rs`), the System page's arithmetic (`sysmon.rs`: CPU load, per-app usage from two process lists, the process list's binary layout, a minute of history, number formats), and the whole PC's power (`power.rs`: the Corsair HXi/RMi and MSI Ai power supplies' USB protocols, decoded from their replies, and the estimate when a PSU doesn't report). **No OS calls** beyond `std` file and socket I/O (`store`, `phonedrop`), so the server's tests run over real sockets anywhere. | any OS (`cargo test -p poni-core`) |
| `crates/poni` | The Windows app (Win32 + Direct2D + WinRT via `windows` 0.58) | Windows only (`cargo wcheck` / `cargo wbuild` cross from macOS/Linux) |
| `crates/poni/src/mac/` | The Mac app (AppKit + Core Graphics + Core Text via `objc2`): its own window, renderer, main loop, notch layouts and menu bar item around the same state, pages, activities and Oyen. See [The Mac app](#the-mac-app). | macOS 14+ (`cargo mcheck` type-checks it from Linux) |

## Threads and data flow

```
 services (own threads / COM callbacks)          UI thread (owns everything below)
 ─────────────────────────────────────          ─────────────────────────────────
 media (GSMTC)      ─┐                           App ── AppState (state.rs)
 lyrics (LRCLIB)    ─┤  EventSink.send(AppEvent)  │      ├─ pages/*      draw via Ui, push Commands
 audio (volume)     ─┼──► mpsc + PostMessage ───► │      ├─ activities  collapsed pill content
 bluetooth          ─┤     (WM_APP_EVENT)         │      └─ island       spring-animated outline
 imaging (thumbs)   ─┤                            │
 phone drop (LAN)   ─┘ on demand, while the Shelf's phone panel is open
 frame clock thread ── WM_APP_FRAME (display vblank, only while animating)
```

* **Rendering**: `gfx.rs`. The island window is a fixed-size, per-pixel-alpha
  layered window big enough for the largest island (widest, tallest page plus
  spring overshoot); the island is drawn centred in it, so a morph never moves
  or resizes the window and the centre stays on one pixel.
  * GPU renderer (default): Direct2D on a Direct3D 11 device (hardware, else
    WARP) draws into a premultiplied flip-model swap chain that
    DirectComposition shows on the window. The layered window's own bitmap is
    then an invisible hit mask (alpha 1/255 where the island takes the mouse,
    0 elsewhere), refreshed at ~30 Hz while the island moves and exactly at
    rest, so transparent pixels stay click-through.
  * Software renderer (fallback: no DirectComposition, e.g. Wine; repeated
    device loss; `PONI_RENDERER=software`): Direct2D in software into a DIB
    pushed with `UpdateLayeredWindowIndirect`, copying only the rectangle the
    island covered this frame and last.
  * Content that fades in or out during a morph is drawn into an offscreen
    layer and composited (`Canvas::layered`): group opacity, and its text is
    rasterised once instead of re-hinting while it scales.
  * All coordinates are **DIPs** (the render target DPI = monitor DPI), in
    window space: the island body is `Island::body_rect_at(cx)`.
* **Frame clock**: `window.rs`. A thread waits on the island display's vblank
  (`IDXGIOutput::WaitForVBlank`; DwmFlush or a steady timer as fallbacks) and
  posts `WM_APP_FRAME`. Timestamps are phase-locked to the display
  (`poni_core::frametime`), and each frame is stamped with the vblank it will
  be shown on, so every animation step is a whole number of refresh periods
  (smooth at 60, 144 or 240 Hz). A morph that starts from rest advances one
  refresh on its first frame. Morphs log a `motion` line (frames, fps,
  hitches, worst gap) for the first 20, then only when frames were dropped.
* **Idle = no frames**: frames are produced only while the island animates,
  a page calls `ui.request_anim()`, input arrives, or a live activity is
  showing. Real motion runs on every vblank; ambient motion on every n-th
  (the equalizer near 60 fps, a drifting progress bar near 30). Otherwise the
  frame clock thread is parked and one-shot timers wake the UI for the next
  visible change (countdown second, HUD expiry, minute tick). The one thing
  that can read at rest is the System page's kept minute, and only once the
  System add-on is on (`Settings::system_history_on`: the add-on, and
  `system_history`, on by default): a light reading a second, about 0.35 ms
  of CPU. It rests while the session is locked or the display is off, and on
  battery saver (`poni_core::sysmon::reads`). See "Keep it light" in
  AGENTS.md.
* **Window**: `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`.
  It never takes focus on click; text fields temporarily drop NOACTIVATE
  (`Command::FocusText`) and give focus back afterwards.
* **Helper window** (hidden): tray icon, AppBar registration (full-screen
  notifications), `WM_POWERBROADCAST`.
* **Step aside** (Settings › Over maximized windows, on by default): the
  full-screen watcher also keeps the app windows whose top edge is at the
  top of the island's monitor, within 8 DIPs (maximized, snapped or dragged
  up; one placed lower keeps its title below the island), as
  `AppEvent::TopWindows`, from the foreground and location WinEvents it
  already listens to (no polling). Each frame the app checks whether one is
  under where the island rests (`activities::resting_mode`'s geometry) and,
  if so, rests as `Mode::Aside`: the Hidden look's invisible strip until the
  pointer reaches it. Pop-ups, notices and a new song's title still drop
  down; over a full-screen app the full-screen choice decides.

## Contracts

### Pages (`crates/poni/src/pages/*.rs`)

```rust
pub fn height(st: &AppState) -> f32;                      // content height in DIPs
pub fn draw(ui: &mut Ui, st: &mut AppState, area: Rect);  // draw + handle input
```

* `area` is the content rect below the header (x/y absolute in canvas DIPs);
  its width is `settings.expanded_width - 2*PAD` (default 620 - 36 = 584).
* Immediate mode: `ui.button(id, rect) -> Resp { hovered, pressed, clicked, rclicked, drag }`.
  Ids from `wid("name")` / `wid_n("row", i)`; must be unique per frame.
  A click arrives on the frame after mouse-up (the app repaints automatically).
* Widgets: `icon_button`, `pill`, `progress_bar` (returns seek fraction),
  `text_input` / `text_input_row` (return submitted/cancelled), `wheel_in(rect)`.
* Drawing: `ui.c` is a `Canvas`: `fill_rrect`, `stroke_rrect`, `fill_circle`,
  `ring`, `arc`, `line`, `text(s, rect, &TextStyle)`, `measure`, `image`,
  `icon(Icon, rect, color)`, `clip/unclip`, `with_alpha`.
* Text: `TextStyle::new(size).weight(Weight::Semibold).color(c).center()/.right().tabular().wrap().top()`.
  Single-line text is ellipsized automatically.
* Colours: `poni_core::geom::palette` (island is pure black; text is white at
  0.96 / 0.62 / 0.50, disabled 0.30; accents GREEN, ORANGE, RED, BLUE, CYAN,
  PURPLE, YELLOW).
* Type scale: `gfx::ts` (11 / 12 / 13 / 15 / 18 / 30 / 50) via `TextStyle::caption()`,
  `footnote()`, `body()`, `headline()`, `value()`, `display()`, `hero()`. Nothing below 11.
* Buttons: `ui.pill(id, rect, label, icon, PillStyle::Utility | Neutral | Primary(c) | UtilityAccent(c))`
  at `PILL_SMALL` (28) or `PILL_REGULAR` (34) height, `PILL_GAP` 8 apart.
* Settings (`pages/settings.rs`) has no tab (`PageKind::tabbed`): the gear right of
  the tabs opens it, and pressing it again goes back. It changes what the tray menu
  changes: settings through `Command::SaveSettings` (applied as a tray change is),
  Start with Windows through `Command::SetAutostart` (read and set off the UI thread).
* Add-ons (`pages/addons.rs`) is the last tab, a "+" (tiles with a plus): one row
  per add-on the app has (`pages::addons::OFFERED`: every one in
  `poni_core::addons::ALL` on Windows, Japanese readings on the Mac) with a switch. Switching one flips
  `settings.addons` through `Command::SaveSettings`; `App::settings_changed` tells
  `services::addons`, which downloads the files (on) or deletes them (off) and
  reports through `AppEvent::Addon` into `st.addons`.
* Prayer (`pages/prayer.rs`) is a tab only while Prayer times is on (`AppState::prayer_on`):
  the next prayer, today's times, qibla, the sound and the countdown switch, and
  a place picker. The app (`App::prayer_*`) reads the add-on's `places.json` and asks
  `services::location` where the PC is (`pages::prayer::located`: the nearest place,
  the point never saved). Cities abroad are looked up online (`services::cities`,
  `Command::GetCities` -> `AppEvent::Cities`, a thread per file): the picker asks for
  the file of the two letters typed, a location away from Kemenag's places for the
  5-degree squares around it, and both are dropped when done (`st.prayer.cities`).
  A city picked or found is saved as its own few lines (`settings.prayer.world_place`),
  so its times need nothing online; and in `tick` turns prayer times that just passed into a notice plus the tone or
  the adzan (`services::adzan`, WinRT MediaPlayer without media controls).
  `activities` shows the minutes before each prayer (`Activity::Prayer`).
* Stocks (`pages/stocks.rs`) is a tab only while the Stocks add-on is on
  (`AppState::stocks_on`; built in, nothing to download): the IHSG and up to
  ten IDX stocks as Apple's Stocks lays them out on a Mac, the list on the
  left (the day's line against the previous close, the price, a change pill
  that switches between percent, points and market cap) and the picked one
  on the right (a chart over 1D, 1M, 6M or 1Y that morphs from one line into
  the next, a scrub under the pointer, the day's numbers), with a search to
  add a stock and an edit mode to remove and reorder them. The list is
  `store.stocks` (`poni_core::stocks::Watchlist`); the prices come from
  oyenza.trade's `/dashboard/api/live/` (`watch`, `names`, `chart`) through
  `services::stocks`, one thread started the first time the page opens,
  which asks only while the page is on screen and someone is at the computer
  (`pages::stocks::watch`, `App::set_stocks_watch`), as often as the answer
  says a fresher one comes (15 s while IDX trades, 10 minutes while it's
  shut), with `If-None-Match`. Parsing, formats, the search and the chart's
  arithmetic are in `poni_core::stocks`; the prices stay in memory
  (`AppState::stocks`).
* Timed page changes (an Undo pill lapsing) report their deadline through
  `pages::next_wake` so the app wakes exactly then instead of polling.
  `st.accent` = Windows accent (lightened); `st.media_accent()` = album-art colour.
* Continuous animation: call `ui.request_anim()` while something must move
  every frame (lyrics scroll, progress bar while playing). Never animate idle.
* Side effects: push `Command`s (`st.cmd(...)`, `st.media_cmd(...)`, `st.save()`,
  `st.chime()`, `st.notify(...)`). Never call services from pages.
* Transient page state lives in `st.ui` (`PageUiState`); add-only requests for
  new fields go through the lead.

### Services (`crates/poni/src/services/*.rs`)

* `start(sink: EventSink) -> windows::core::Result<Self>`; failures are logged
  and the feature is skipped. Never panic, never block the UI thread.
* Talk to the UI only through `sink.send(AppEvent::…)` (see `events.rs`).
* COM on worker threads: initialise with `CoInitializeEx(None, COINIT_MULTITHREADED)`.
* WinRT async: call `.get()` only on worker threads.

### Events (`events.rs`) and commands (`commands.rs`)

`AppEvent` = everything services report. `Command` = everything pages request.
`app.rs` maps both onto `AppState` and service calls.

## Files

| File | Owner area |
|---|---|
| `app.rs` | message loop, island state machine, input, timers, commands |
| `activities.rs` | collapsed modes + their geometry and drawing |
| `oyen.rs` | Oyen: draws `poni_core::oyen` frames peeking over the island's edge in his theme (idle, beside the clock, in the media pill), and in every theme as the Oyenza tab's icon (`oyen::tab`: he rises into the tab and waves as the island opens, blinks and flicks an ear now and then, watches the pointer over the tab row, hops when his tab is picked) |
| `pill/*.rs` | the Pill theme (`Theme::Pill`, `look::PILL` with `Shape::Pill`): the island floats `pill::TOP_GAP` below the top edge (`Island::set_floating`); `shape` draws the floating outline, shadow and hit area, `motion` its springs, and one module per surface (`idle`, `status`, `media`, `lyric`, `live`, `hud`, `notice`, `expanded`, `music`) behind hooks in `activities.rs` and `pages` that fall back to Classic's drawing |
| `gfx.rs`, `icons.rs`, `ui.rs`, `island.rs`, `window.rs` | rendering/UI foundation |
| `state.rs`, `events.rs`, `commands.rs` | shared contracts |
| `pages/*.rs` | expanded pages |
| `services/*.rs` | media (Apple Music's own progress slider and back button through UI Automation in `applemusic_ui`: it declines both from Windows' media controls), lyrics/http, audio, bluetooth, fullscreen, clipboard (plain-text read and write for paste and the Apple Music token; nothing watches it), imaging, oyenza, power, sound, stocks (the Stocks page's prices from oyenza.trade, only while the page is open), sysstats, phonedrop (started and stopped by the Shelf's phone panel), sysmon (the System page's readings: each second while the page is open or a game's readout shows, a light reading a second for the kept minute while it's closed; all of it rests while nobody is at the PC, the kept minute also on battery saver; while the page is open on a desktop it also loads NVML for an NVIDIA card's watts and reads a power supply that reports over USB, both closed with the page), psu (that power supply: found among the HID devices by its USB ids, read with requests only, replies matched to them), addons (add-on files: downloaded from their GitHub pre-release when an add-on is switched on, checked against the size and SHA-256 in `poni_core::addons`, deleted when it is switched off; asleep otherwise), frames (Show over games' frame count: a real-time event tracing session of the game's Present calls, only while the readout shows; the one-time OK that lets the app read it is `poni.exe --allow-fps <SID>`, run elevated) |
| `dragdrop.rs`, `shell.rs`, `tray.rs`, `autostart.rs` | OS integration |
| `version.rs` | which build this is for the menu's About line: the version, and from `build.rs` release, test build (commit) or local build, and the day CI built it |
| `updates.rs` | new versions, the app's side: when to read the update file (`update.json` remembers), the island's notice ("Update available x", Hide), the offer the menu and Settings show, and on Windows the new version's Setup (downloaded first, run with `/S /reopen` now or before the next start); the Update sheet it opens is drawn by `pages/settings.rs` (rules in `poni_core::update`, the read and signature check in `services/update.rs`; docs/updates.md) |
| `selftest.rs` | `poni.exe --selftest <dir>` scripted screenshots for CI |

## Data

* Settings: `%APPDATA%\Poni\settings.json` (created with defaults; tray → "Open settings file").
* Data (tasks, shelf, focus stats, the Stocks list): `%APPDATA%\Poni\data.json`. Drinks logged by the old Water page stay in it untouched (`water`).
* Cache + log: `%LOCALAPPDATA%\Poni\` (`lyrics\*.json`, `poni.log`), plus the Apple Music
  token sealed with DPAPI (`apple-music-token.bin`) and the web player's public
  developer token (`apple-music-web.json`).
* Phone drop: received files go to `Downloads\Oyenza Studio` (`paths::phone_inbox_dir`),
  staged in a `.oyen-poni-incoming-<n>` folder there until every file of a send has arrived.
* Oyenza (optional): the sign-in, this PC's own key, sealed with DPAPI
  (`%LOCALAPPDATA%\Poni\oyenza-link.bin`; `oyenza-revoke.bin` while a sign-out
  still has to reach the server). The worker (`services/oyenza.rs`) asks
  api.oyenza.app on its own thread, paced by the PC's use and power, and
  checks the tab's day list every 15 s only while the tab is on screen
  (`App::set_oyenza_watch`, beside the System page's sampler); words,
  parsing and cadence are in `poni_core::oyenza`, the tab in `pages/oyenza.rs`.
  The Today page's days (`GET /island/day?days=35`: today and the next five
  weeks of Google Calendar's events and the flights from Gmail that the person
  switched on in oyenza.app) are asked for when Today opens and every 5
  minutes while it stays (`App::set_today_watch`); reading them, the list's
  days and rows and each date's dots are in `poni_core::agenda`, the page in
  `pages/today.rs`, which keeps the stat tiles for a server without the day.
* `PONI_HOME` overrides both (the self-test uses it).

## Building

```
cargo test -p poni-core          # logic tests, any OS
python3 tools/make_ja_dict.py    # rebuild the Japanese reading dictionary, fixes in tools/ja_dict_fixes.csv (--check compares)
cargo wcheck                     # type-check the Windows app from macOS/Linux
cargo wbuild                     # cross-build target/x86_64-pc-windows-gnu/release/poni.exe (needs mingw-w64)
cargo build -p poni --release    # on Windows (MSVC)
cargo mcheck                     # type-check the Mac app from Linux/Windows (no linker needed)
cargo build -p poni --release    # on a Mac: the Mac app (tools/make_mac_app.sh makes the .app and DMG)
```

## The Mac app

Built in slices (slice 1: the island, drawing, clock, timer and the menu bar
item; music, the Shelf, pop-ups and Oyenza sign-in came later). `main.rs` builds the shared modules (`activities`, `commands`,
`icons`, `island`, `oyen`, `pages`, `plog`, `png`, `state`, `ui`) for both
systems and takes the platform ones from `src/mac/` by `#[path]` on macOS, so
the Windows files are untouched.

| File | What |
|---|---|
| `mac/gfx.rs` | `Canvas` on Core Graphics + Core Text, call for call the Windows API: sRGB colours, the theme's look, SF Pro (or the embedded Outfit), images, icons, clips, shape masks, transparency layers for fades. Also `Bitmap` (offscreen) |
| `mac/window.rs` | A borderless, non-activating `NSPanel` above the menu bar (level 27, all Spaces), sized once for the largest island; its view draws (y down) and forwards input; a display link on the view paces frames at the screen's refresh (120 Hz on ProMotion) |
| `mac/app.rs` | The island's behaviour, ported from `app.rs`: hover lift, dwell to open, collapse delay, morphs and cross-fades, page switches, notices, the Pomodoro, settings and data. Clicks pass through to the apps below except over the island (the app toggles the panel's `ignoresMouseEvents` from global mouse-move monitors, which need no permission) |
| `mac/notch.rs` | The camera notch: the resting clock and live activities sit in the menu bar's line left and right of the camera, the resting eyes play hide and seek with it (`poni_core::peek`), Minimal and Hidden hide inside it, notices and the open island hang below it. Pill is the pill or the notch: it rests inside it (the eyes beside it) and everything else floats below it |
| `mac/menubar.rs` | The menu bar item (Oyen's mark as a template image) and its menu, also on a right-click of the island |
| `mac/render.rs` | `poni --render <dir> [--theme oyen] [--notch]`: every state with sample data as PNGs, offscreen (CI's way to look at the Mac drawing; the runners have no notch) |
| `mac/clock.rs`, `paths.rs`, `events.rs`, `services.rs`, `dragdrop.rs` | Local time, `~/Library` folders, the event channel (drained on the main queue), the accent colour, links, clipboard, sounds |
| `mac/services/media.rs`, `mediaremote.rs` | What plays: macOS's own now playing (browsers, Spotify, Music, any player), read through the MediaRemote adapter (`third_party/mediaremote-adapter`, BSD-3), which `/usr/bin/perl` runs since macOS 15.4 answers Apple's own programs only; its stream runs while there is sound (Core Audio, `audio.rs`) or a track to show, `poni_core::mediaremote` reads it. Until it answers, Music and Spotify's own notifications and AppleScript |

CI (`.github/workflows/mac.yml`, on `main` and `mac-*` branches) builds for
Apple Silicon and Intel, renders every state, runs the real window through a
few states with screenshots (`poni --smoke <dir>` writes where its window is
and waits for each screenshot), and packages `Oyenza-Studio-<version>-mac-unsigned.dmg` with the MediaRemote
adapter's framework built by CMake (ad-hoc signed; Developer ID signing and
notarization need the Apple secrets).

