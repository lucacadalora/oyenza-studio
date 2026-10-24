# Poni architecture

Poni is a "living notch" for Windows 10/11: a black island that hangs from the
top-centre of the screen, shows live activities (media, timers, pop-ups) while
collapsed, and expands on hover into pages (Music + synced lyrics, Focus,
Health, Shelf, Tasks, Clipboard, Today, System). The Shelf's Phone drop serves a
small web page on the local network so a phone can send files to it.

## Crates

| Crate | What | Builds on |
|---|---|---|
| `crates/poni-core` | Pure logic + models: geometry/colour, springs, LRC parsing, LRCLIB URL/JSON, Apple Music lyrics (web-player token, API replies, TTML), player metadata fix-ups, Pomodoro, the steps snapshot Oyenza sends (`health.rs`), tasks, shelf, clipboard history, settings, JSON store, SVG path parser, Oyen's face (`oyen/`, a port of the Oyenza app's built face, checked against its Swift port by `oyen/swift_check.rs`), Phone drop (`phonedrop/`: the LAN web server, the phone page, safe file names, QR codes), and the System page's arithmetic (`sysmon.rs`: CPU load, per-app usage from two process lists, the process list's binary layout, a minute of history, number formats). **No OS calls** beyond `std` file and socket I/O (`store`, `phonedrop`), so the server's tests run over real sockets anywhere. | any OS (`cargo test -p poni-core`) |
| `crates/poni` | The Windows app (Win32 + Direct2D + WinRT via `windows` 0.58) | Windows only (`cargo wcheck` / `cargo wbuild` cross from macOS/Linux) |

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
* **Idle = zero CPU**: frames are produced only while the island animates,
  a page calls `ui.request_anim()`, input arrives, or a live activity is
  showing. Real motion runs on every vblank; ambient motion on every n-th
  (the equalizer near 60 fps, a drifting progress bar near 30). Otherwise the
  frame clock thread is parked and one-shot timers wake the UI for the next
  visible change (countdown second, HUD expiry, minute tick).
* **Window**: `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`.
  It never takes focus on click; text fields temporarily drop NOACTIVATE
  (`Command::FocusText`) and give focus back afterwards.
* **Helper window** (hidden): tray icon, AppBar registration (full-screen
  notifications), `WM_POWERBROADCAST`, `WM_CLIPBOARDUPDATE`.

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
| `oyen.rs` | the Oyen theme: draws `poni_core::oyen` frames peeking over the island's edge (idle, beside the clock, in the media pill) |
| `gfx.rs`, `icons.rs`, `ui.rs`, `island.rs`, `window.rs` | rendering/UI foundation |
| `state.rs`, `events.rs`, `commands.rs` | shared contracts |
| `pages/*.rs` | expanded pages |
| `services/*.rs` | media, lyrics/http, audio, bluetooth, fullscreen, clipboard, imaging, oyenza, power, sound, sysstats, phonedrop (started and stopped by the Shelf's phone panel), sysmon (the System page's readings: its thread runs only while the page is open) |
| `dragdrop.rs`, `shell.rs`, `tray.rs`, `autostart.rs` | OS integration |
| `selftest.rs` | `poni.exe --selftest <dir>` scripted screenshots for CI |

## Data

* Settings: `%APPDATA%\Poni\settings.json` (created with defaults; tray → "Open settings file").
* Data (tasks, shelf, focus stats): `%APPDATA%\Poni\data.json`. Drinks logged by the old Water page stay in it untouched (`water`).
* Cache + log: `%LOCALAPPDATA%\Poni\` (`lyrics\*.json`, `poni.log`), plus the Apple Music
  token sealed with DPAPI (`apple-music-token.bin`) and the web player's public
  developer token (`apple-music-web.json`).
* Phone drop: received files go to `Downloads\Oyen Poni` (`paths::phone_inbox_dir`),
  staged in a `.oyen-poni-incoming-<n>` folder there until every file of a send has arrived.
* Oyenza (optional): the sign-in, this PC's own key, sealed with DPAPI
  (`%LOCALAPPDATA%\Poni\oyenza-link.bin`; `oyenza-revoke.bin` while a sign-out
  still has to reach the server). The worker (`services/oyenza.rs`) asks
  api.oyenza.app on its own thread, paced by the PC's use and power; words,
  parsing and cadence are in `poni_core::oyenza`, the tab in `pages/oyenza.rs`.
* `PONI_HOME` overrides both (the self-test uses it).
* Clipboard history is memory-only and skips content marked
  `ExcludeClipboardContentFromMonitorProcessing` / `CanIncludeInClipboardHistory=0`
  (password managers set these).

## Building

```
cargo test -p poni-core          # logic tests, any OS
cargo wcheck                     # type-check the Windows app from macOS/Linux
cargo wbuild                     # cross-build target/x86_64-pc-windows-gnu/release/poni.exe (needs mingw-w64)
cargo build -p poni --release    # on Windows (MSVC)
```
