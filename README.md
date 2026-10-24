<p align="center"><img src="docs/brand/oyen-poni-logo.svg" width="96" alt="Oyen Poni logo: an orange tile with the black island at the top playing music, and a white Oyen below"></p>

<h1 align="center">Oyen Poni</h1>

<h3 align="center">A living notch for Windows and Mac</h3>

<p align="center">Like the Dynamic Island on a phone, for your computer. A small island hangs from the top of your screen, shows what's playing, counting down and arriving, and opens into quick pages when you point at it.</p>

<p align="center">
  <a href="https://github.com/lucacadalora/poni/releases/latest/download/Oyen-Poni-Setup.exe"><img src="https://img.shields.io/badge/Download_for_Windows-0078D4?style=for-the-badge&logo=data:image/svg%2Bxml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0iI2ZmZiI%2BPHBhdGggZD0iTTIgMmg5LjJ2OS4ySDJ6bTEwLjggMEgyMnY5LjJoLTkuMnpNMiAxMi44aDkuMlYyMkgyem0xMC44IDBIMjJWMjJoLTkuMnoiLz48L3N2Zz4=" height="34" alt="Download for Windows"></a>
  &nbsp;
  <a href="https://github.com/lucacadalora/poni/releases/latest/download/Oyen-Poni-Mac.dmg"><img src="https://img.shields.io/badge/Download_for_Mac-333333?style=for-the-badge&logo=apple&logoColor=white" height="34" alt="Download for Mac"></a>
</p>

<p align="center">
  <a href="https://lucacadalora.github.io/poni/"><b>Website</b></a>
  &nbsp;·&nbsp;
  <a href="#features">Features</a>
  &nbsp;·&nbsp;
  <a href="#install">Install</a>
  &nbsp;·&nbsp;
  <a href="#faq">FAQ</a>
  &nbsp;·&nbsp;
  <a href="#guides">Guides</a>
</p>

<p align="center">
  <a href="https://github.com/lucacadalora/poni/releases/latest"><img src="https://img.shields.io/github/v/release/lucacadalora/poni?label=latest&color=F0892E" height="20" alt="Latest release"></a>
  <a href="https://github.com/lucacadalora/poni/blob/metrics/README.md"><img src="https://raw.githubusercontent.com/lucacadalora/poni/metrics/badge.svg" height="20" alt="Downloads so far"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-555555" height="20" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/built_with-Rust-555555?logo=rust" height="20" alt="Built with Rust">
</p>

<p align="center"><img src="docs/brand/readme/hero.webp" width="880" alt="The island hanging from the top of the screen, open on the Music page with lyrics in time, and Oyen the cat peeking over a playing island"></p>

<p align="center"><sub>Free and open source &nbsp;·&nbsp; Windows 10 and 11 &nbsp;·&nbsp; macOS 14 or later &nbsp;·&nbsp; No account needed</sub></p>

## Features

### Music, word by word

Controls for whatever is playing: Spotify, Apple Music, YouTube in your browser, or any app in Windows' media controls. The lyrics scroll in time with the song, and Korean, Japanese and Chinese lines get their reading underneath, the way Apple Music shows pronunciation. Drag the bar to jump anywhere in a song.

<p align="center"><img src="docs/brand/readme/music.webp" width="880" alt="The open island on the Music page: the song, its controls and a Korean lyric line with its reading underneath"></p>

Lyrics come free from [LRCLIB](https://lrclib.net), or from Apple Music itself with [your Apple Music token](#apple-music-lyrics).

### What's happening, at a glance

While it rests, the island is a small black pill that tells you what's going on, then gets out of the way: album art and a beat while music plays, your focus timer, volume and charging, headphones connecting with their battery, money coming in, the next prayer.

<p align="center"><img src="docs/brand/readme/glance.webp" width="880" alt="Six resting islands: music playing, a focus timer, volume, headphones connected, money in, and minutes to the next prayer"></p>

### From your phone to your PC, no app needed

Open the Shelf, scan the code with your phone's camera, pick photos or files and tap **Send**. The island asks first, then they land on the Shelf. It works with iPhones and Android phones over your own Wi-Fi, and a phone you pair once can send from any network.

<p align="center"><img src="docs/brand/readme/phone.webp" width="880" alt="The Shelf with three files, and a request from an iPhone to send three more, with Decline and Accept"></p>

[How Phone drop works](#phone-drop) &nbsp;·&nbsp; [Paired phones](#paired-phones-send-from-any-network)

### FPS, GPU and CPU over your games

**New in 0.1.18.** Switch on **Show over games** and, while a borderless full-screen game is in front, the island becomes a small readout: frames per second, the GPU's load and temperature and the CPU's load. It reads Windows' own log of each frame, the way PresentMon and Xbox Game Bar count them, so nothing is loaded into the game.

<p align="center"><img src="docs/brand/readme/games.webp" width="880" alt="The readout over a game: 144 fps, GPU 97 % at 71 degrees, CPU 38 %; below it the System page's Games view with the frame rate chart"></p>

[More on Show over games](#show-over-games)

### Your whole PC on one page

CPU, GPU, memory, drives, network and battery: rings and gauges, each core's load, the last minute of history and the apps using the most. Read straight from Windows' own counters, with no drivers and no admin rights, and it never leaves your PC.

<p align="center"><img src="docs/brand/readme/system.webp" width="880" alt="The System page: CPU temperature, load and clock rings, usage history, each core's load and the top processes"></p>

### The small things, one hover away

**Focus** is a Pomodoro timer that keeps counting in the island. **Tasks** is a tiny to-do list. **Clipboard** keeps your recent copied text in memory only and skips anything a password manager marks private. **Today** has the date, the month and your PC's vitals. **Shelf** keeps the files you drop on the island. Scroll over the top of the island to switch pages.

<p align="center"><img src="docs/brand/readme/pages.webp" width="880" alt="Four open pages: the Focus timer, Tasks, Clipboard history and Today"></p>

### Black, Oyen or Glass

The island is black by default, like the one on a phone. **Oyen** turns it warm cream, and Oyen the cat moves in: he peeks over the edge, follows your pointer and sleeps from midnight to five. **Glass** bends and frosts whatever is behind it, after Apple's Liquid Glass.

<p align="center"><img src="docs/brand/readme/themes.webp" width="880" alt="The Music page in Classic and in the Oyen theme, and the Glass theme over a light and a dark page"></p>

[The Oyen theme](#the-oyen-theme) &nbsp;·&nbsp; [The Glass theme](#the-glass-theme)

### Your money, as it moves

If you keep your money in [Oyenza](https://oyenza.app), sign in (optional) and the island tells you when money comes in or goes out, reminds you the day before a bill is due, and lists your week by day. Sums stay hidden until you click **Show**, so a shared screen shows only that much. On Windows.

<p align="center"><img src="docs/brand/readme/oyenza.webp" width="880" alt="A Money in notice from BCA with the amount hidden, and the Oyenza tab listing today's transactions with hidden sums"></p>

[How signing in works](#oyenza-optional)

### Prayer times for your city

Today's times for your kab/kota as Kemenag RI publishes them, a countdown in the ten minutes before each prayer, the adzan recorded at Masjidil Haram, and the qibla. It's an add-on: 1.9 MB, downloaded only when you switch it on. On Windows.

<p align="center"><img src="docs/brand/readme/prayer.webp" width="880" alt="Eight minutes to Ashar on the resting island, and the Prayer tab with today's times for Kota Jakarta"></p>

[Add-ons](#add-ons)

### On a Mac, it wraps the notch

On a MacBook, the island lives around the camera notch: the album art on one side, the beat on the other, and it opens downward when you point at it. Music and Spotify with lyrics, the focus timer, the Shelf and Phone drop, tasks, the clipboard and Today are all there.

<p align="center"><img src="docs/brand/readme/mac.webp" width="880" alt="On a MacBook: music playing and a focus timer either side of the notch, and the Music page opening below it"></p>

[What's on the Mac so far](#on-a-mac)

## Light by design

<table>
<tr>
<td align="center" width="25%"><h3>6.3 MB</h3>download on Windows</td>
<td align="center" width="25%"><h3>Rust</h3>native code, no web view, no runtime</td>
<td align="center" width="25%"><h3>240 Hz</h3>drawn on the GPU, in step with your screen</td>
<td align="center" width="25%"><h3>0</h3>accounts, ads or analytics</td>
</tr>
</table>

On Windows it draws on your graphics card in step with your display's refresh rate (60, 144 or 240 Hz), and draws nothing while nothing moves. The one thing that reads at rest is the System page's last minute of history, a light reading once a second that right-click › **Keep System page history** turns off. Features you don't use don't run: Show over games only counts while a game is in front, and add-ons download only when you switch them on.

## Your data stays on your computer

- **No account, no tracking.** Tasks, the Shelf and focus history are files in your own user folder. There are no ads and no analytics.
- **Clipboard in memory.** Copied text is never written to disk, and anything a password manager marks private is skipped.
- **Files go straight between your devices.** Phone drop asks before it takes anything and uses your own Wi-Fi; paired phones connect directly, encrypted with keys made on the two devices.
- **Online only when you say so.** Lyrics lookups go to LRCLIB. Apple Music lyrics, Oyenza and add-ons connect only after you turn them on. [PRIVACY.md](PRIVACY.md) lists every request.

## Install

<img src="docs/brand/art/oyen-poni-peek.webp" width="260" align="right" alt="Oyen peeking over the island while it plays music">

### Windows

**[Download Oyen-Poni-Setup.exe](https://github.com/lucacadalora/poni/releases/latest/download/Oyen-Poni-Setup.exe)** and run it. It installs just for you and needs no admin rights. Older versions and a portable zip are on the [Releases](https://github.com/lucacadalora/poni/releases) page.

Builds aren't code-signed yet, so Windows SmartScreen may say "Windows protected your PC". Click **More info → Run anyway**.

Your data lives in `%APPDATA%\Poni\`, and cache plus log in `%LOCALAPPDATA%\Poni\`. Uninstalling removes the app, cache and log, and keeps your data.

### Mac

macOS 14 or later, Apple silicon or Intel (one app for both): **[download Oyen-Poni-Mac.dmg](https://github.com/lucacadalora/poni/releases/latest/download/Oyen-Poni-Mac.dmg)**, open it and drag Oyen Poni into Applications. It isn't signed with an Apple Developer ID yet, so the first time macOS warns you, click **Done**, then open **System Settings → Privacy & Security** and click **Open Anyway**. What's on the Mac so far: [On a Mac](#on-a-mac). Your data lives in `~/Library/Application Support/Poni/`, the log in `~/Library/Logs/Poni/`.

## FAQ

<details>
<summary><b>Is it free?</b></summary>
<br>

Yes. Oyen Poni is free to download and use, and its code is open source under the [MIT license](LICENSE). Oyenza sign-in is optional.

</details>

<details>
<summary><b>Will it slow my computer down?</b></summary>
<br>

It's built not to. It's native code with no web view, it draws nothing while nothing moves, and its features sleep until you use them. See [Light by design](#light-by-design).

</details>

<details>
<summary><b>Why does Windows say "Windows protected your PC"?</b></summary>
<br>

The builds aren't code-signed yet, so Windows SmartScreen doesn't know them. Click **More info → Run anyway**. On a Mac, the first launch needs **Open Anyway** in Privacy & Security for the same reason.

</details>

<details>
<summary><b>Does it work on Windows 10?</b></summary>
<br>

Yes, on Windows 10 and 11. Glass needs Windows 10 version 2004 or later to bend what's behind it; before that it draws smoked glass.

</details>

<details>
<summary><b>What's different on a Mac?</b></summary>
<br>

The island wraps the notch, and the Mac has music with lyrics, Focus, the Shelf with Phone drop, Tasks, Clipboard, Today, the Classic and Oyen themes and start at login. Not on the Mac yet: Glass, paired phones, Oyenza sign-in, add-ons, Show over games, headphone pop-ups and hiding in full-screen apps. [On a Mac](#on-a-mac) has the details.

</details>

<details>
<summary><b>Can it hide during games and videos?</b></summary>
<br>

Yes: right-click the island › **In full-screen apps** › **Hide**. Pick **Stay on top** instead to keep it over borderless games and videos, or turn on [Show over games](#show-over-games) for a frame rate readout.

</details>

<details>
<summary><b>Where are my files and settings?</b></summary>
<br>

On Windows in `%APPDATA%\Poni\` (cache and log in `%LOCALAPPDATA%\Poni\`); on a Mac in `~/Library/Application Support/Poni/` (log in `~/Library/Logs/Poni/`). Uninstalling keeps your data.

</details>

## Guides

The details behind each feature.

### Settings and options

The gear at the top right of the open island opens **Settings**: your Oyenza account first, then the theme, the idle look and the switches below. Right-click the island or the tray icon for every option:
- the theme: Classic, Oyen or Glass
- the idle look: clock, eyes (Oyen in his theme), minimal, or hidden until you hover
- lyrics (and the pronunciation line under Japanese, Korean and Chinese lyrics), clipboard history and sounds
- in full-screen apps: hide, or stay on top of borderless games and videos (a game in exclusive full-screen mode still covers it), and show FPS, GPU and CPU over games
- start with Windows, and quit

On a Mac, every option is in the menu bar, under Oyen's mark.

### The Oyen theme

The island stays black by default. Switch on the Oyen theme (the gear › **Theme › Oyen**, or right-click › **Theme › Oyen**) and the whole island becomes Oyen's: warm cream with dark brown text, his orange on the play button, the bars and the switches, and Outfit, the Oyen Poni typeface. Oyen, the Oyenza cat, moves in too: he peeks over the island's edge while it rests, watches your pointer when it comes near, sits beside the clock, peeks up beside the pin when the island is open and bobs along while music plays. He sleeps from midnight to five. He is drawn from the Oyenza logo, not a picture, so he stays sharp at any scaling, and he only moves for a moment now and then, so he costs no CPU at rest.

![Classic and the Oyen theme side by side: resting, with the clock, and playing music](docs/screenshots/oyen-theme.png)

### The Glass theme

Glass turns the island into liquid glass, after Apple's Liquid Glass. Its rim bends and magnifies what's under it, the middle is frosted, a light runs along the edge, and the shadow under it deepens over busy pages. The collapsed island turns light or dark with what's behind it, and the open island follows Windows' light or dark mode. Its tint goes only as deep as the text needs to stay readable (4.5:1).

To bend what's under it, Glass copies the part of the screen behind its window (the island and the space it opens into) whenever something there changes, and not while the island is hidden or the PC is locked. The copy stays in your PC's memory and on its graphics card, and each new copy replaces the last; it's never saved or sent. While Glass is on, Windows leaves Oyen Poni out of screenshots, screen recordings and screen sharing. On a display in HDR, Poni maps the copy back to ordinary colours itself. Where Poni can't copy the screen (the software renderer, Windows 10 before version 2004, Remote Desktop, a laptop display that no graphics card lets it copy), Glass draws smoked glass instead: the same shape, light and shadow over a see-through tint, and it stays in screenshots.

### Show over games

Switch on **Show over games** (System › Games, or right-click › In full-screen apps) and, while a game fills the screen in borderless full-screen, the island becomes a small readout: frames per second, the GPU's load and temperature and the CPU's load, once a second. Hover it to open the island as usual. Turning it on also keeps the island on top of full-screen apps; picking Hide there turns it off again.

The frame rate is counted the way Intel's PresentMon and Xbox Game Bar count it: Windows logs every frame a game hands to the screen, and Oyen Poni reads that log for the game in front. Nothing is loaded into the game, so there is nothing for anti-cheat to see. DirectX 9 to 12 games are counted from their own Present calls; Vulkan and OpenGL games from the graphics kernel's log of their frames. The GPU and CPU numbers come from the same readers as the System page. Nothing runs while no game is in front or while the readout is off.

Windows lets only admins and the **Performance Log Users** group read that log. The first time, System › Games asks for a one-time OK: an admin prompt adds your Windows account to that group (Xbox Game Bar's FPS counter asks for the same), and Windows applies it the next time you sign in. Until then the FPS slot shows a dash and the rest works. To undo it, remove your account from Performance Log Users in Computer Management, or run `net localgroup "Performance Log Users" <your user name> /delete` as an admin. A game in exclusive full-screen mode still covers the island, as it covers every window.

### On a Mac

On a MacBook with a camera notch, the island lives around it: what's playing sits either side of the camera (the album art on the left and the beat on the right, a timer's ring and its minutes the same way), and the island opens downward from the notch when you point at it. On a Mac without a notch, or on another display, it hangs from the top of the screen as it does on Windows.

The Mac app has the island, Music and Spotify with synced lyrics (and Apple Music's own with [your token](#apple-music-lyrics)), the focus timer, the Shelf with [Phone drop](#phone-drop) over the same Wi-Fi, tasks, clipboard history, Today with your Mac's CPU, memory, disk and battery, pop-ups for volume and charging, the Classic and Oyen themes, and start at login. Every option is in the menu bar, under Oyen's mark. Not on the Mac yet: Glass, paired phones, Oyenza sign-in, add-ons, Show over games, headphone pop-ups and hiding in full-screen apps.

The Mac pictures come from the Mac app itself (`poni --render <dir> --notch`, run by the `mac` workflow on a macOS runner with a MacBook's notch drawn in).

### Apple Music lyrics

Lyrics come from LRCLIB by default. With an Apple Music subscription, Oyen Poni can show Apple Music's own synced lyrics instead: the ones the Apple Music app shows. This works whichever app is playing. LRCLIB stays the backup for songs Apple has no synced lyrics for.

It needs your Apple Music token, copied once from the web player:

1. Open [music.apple.com](https://music.apple.com) in Chrome or Edge and sign in.
2. Press F12, open **Application** → **Cookies** → `https://music.apple.com`, and copy the value of `media-user-token`.
3. Right-click the island (or the tray icon) → **Apple Music lyrics** → **Use token from clipboard**.

Oyen Poni checks the token with Apple and says whether it worked. It stores the token encrypted for your Windows account (`%LOCALAPPDATA%\Poni\apple-music-token.bin`) and clears it from the clipboard. If Windows clipboard history (Win+V) is on, delete it there too. A token lasts for months; when Apple stops accepting it, Oyen Poni says so and you copy a fresh one. **Remove token** in the same menu turns this off.

With a token saved, Oyen Poni sends each song's title and artist to Apple to find its lyrics. It uses the Apple Music web player's own API, which Apple doesn't publish for other apps, so it can stop working when Apple changes its site. LRCLIB keeps working either way.

### Phone drop

Send photos and files from your phone to the Shelf, or save Shelf files on your phone, over your own Wi-Fi. Nothing to install on the phone, and it works with iPhones and Android phones alike.

1. Open the Shelf and click **Phone** (on an empty shelf, the **Phone drop** tile). A QR code opens under the shelf.
2. Scan it with the phone's camera. The phone opens a small page that Oyen Poni serves from your PC.
3. Pick photos or files on the phone and tap **Send**. The island asks first: **Accept** or **Decline**. Accepted files go to `Downloads\Oyen Poni` and onto the Shelf.
4. The page also lists the files on the Shelf. Tap **Save** to keep one on the phone.

| | |
|---|---|
| ![Phone drop open: the QR code waits for a phone](docs/screenshots/page-shelf-phone.png) | ![An iPhone asks to send three files](docs/screenshots/page-shelf-phone-ask.png) |

The phone and the PC need the same Wi-Fi; guest networks often keep devices apart. The first time, Windows asks whether Oyen Poni may use the network: allow it on private networks. If Windows treats your Wi-Fi as a public network, set it to private (Settings > Network & internet > Wi-Fi), or the phone can't reach the PC.

Each session gets a new secret link that only the QR code carries. It stops working when you click **Done**, and after 10 minutes without a phone. Oyen Poni reads nothing from an upload before you accept, keeps a transfer only when every file arrived, and never sends anything through the internet. Files travel over your Wi-Fi without extra encryption on top of the Wi-Fi's own, so use Phone drop on networks you trust rather than public hotspots.

This isn't AirDrop. An iPhone only AirDrops over Apple's own Wi-Fi link (AWDL), which Windows Wi-Fi drivers can't run, so Phone drop uses the phone's browser instead.

#### Paired phones: send from any network

Pair a phone once, and it finds this PC from any network, whenever Oyen Poni runs: the same Wi-Fi, another Wi-Fi or mobile data.

1. In the phone panel, click **Pair a phone** and scan the new code. The phone opens `drop.oyenza.com` and says it's paired.
2. Keep that page: on an iPhone, tap Share › **Add to Home Screen**; on Android, install it (Chrome or Samsung Internet). Android then lists **Oyen Poni** in the share sheet.
3. Send from the share sheet or from the page. Files from your own paired phones go straight onto the Shelf, with a notice on the island.

| | |
|---|---|
| ![A pairing code under the shelf](docs/screenshots/page-shelf-pairing.png) | ![The paired phones, each with Remove](docs/screenshots/page-shelf-paired-phones.png) |

![The phone page: paired, your PCs, sending, and on the Shelf](docs/screenshots/paired-phone-page.webp)

The phone and PC meet through a small server (`drop.oyenza.com`, code in [`server/`](server)); the files go straight between them over WebRTC, encrypted with keys made on the two devices, or through a relay that can't read them when both networks block a direct link. The server never sees a file, a file name, or the pairing secret: every message is signed with that secret, which only the QR code carries. **Paired phones** lists each phone with **Remove**. How it works: [docs/paired-drop.md](docs/paired-drop.md).

### Oyenza (optional)

If you keep your money in [Oyenza](https://oyenza.app), the island can show it. Signing in is optional and works like a TV: no password is typed into Oyen Poni.

1. Open the island, press the gear at the top right, and choose **Sign in with Oyenza** (the Health page has the same button; right-click the island or the tray icon → **Oyenza** → **Sign in to Oyenza…** works too). The island shows an 8-letter code.
2. Open oyenza.app/connect on your phone or this PC (**Open oyenza.app** fills the code in), check the PC's name, and choose **Connect**.

Then the island shows:

- **Your Oyenza, live, by day**: the **Oyenza** tab lists every transaction of the last 7 days, newest first, with each day's money in and out: pick Sun to Thu, Yesterday or Today on the strip of days. Something logged while you look glows for a minute. **Tomorrow** and **Later** list the subscriptions and card bills due in the next 30 days.
- **Money in and out as it happens**: a notice when Oyenza logs money by itself from your bank's emails or notifications, "Money in · BCA" or "Money out · BCA". Sums and who stay hidden until you click **Show**, there and in the tab, so a shared screen shows only that much.
- **Bills before they're due**: a reminder the day before a subscription renews or a card bill is due.
- **Note a spend**: type it in the **Oyenza** tab ("kopi 25rb") and press Enter; it goes straight to Oyenza, with Undo for 10 seconds.

This PC gets a key of its own that can do only these things: it reads the last 7 days of transactions in the space where Oyenza logs your bank's emails and notifications, never balances, account numbers, notes or receipts, and it can't change or delete anything else or touch your account. It is stored encrypted for your Windows account (`%LOCALAPPDATA%\Poni\oyenza-link.bin`). Oyen Poni asks Oyenza once a minute while you use the PC (every 3 minutes on battery, never while it's locked or asleep), and every 15 seconds while the Oyenza tab is open. **Disconnect** in Settings (or **Oyenza › Disconnect** in the menu) signs this PC out; you can also disconnect it in oyenza.app under Connected apps.

### Add-ons

The **+** at the end of the tabs lists features that aren't part of the download. Switching one on downloads its files from this project's releases (each checked against the size and SHA-256 built into the app); switching it off deletes them. While every add-on is off, nothing is downloaded and nothing of theirs runs.

**Prayer times** (1.9 MB) shows today's times for your kab/kota as Kemenag RI publishes them, with Imsak and Terbit, the next prayer and how long until it, and the qibla. Makkah, Madinah and Jeddah use Umm al-Qura. In the 10 minutes before each prayer the island counts down, and at the time it says so with a short tone, the adzan recorded at Masjidil Haram, or nothing (Subuh gets the tone). Music pauses for the adzan; **Stop** ends it.

- **Your place:** Windows' location picks the nearest of Kemenag's 517 places and checks again every few hours, so a trip moves the place. The point is used once on your PC and never saved or sent. With location off, the PC's time zone suggests a place; **Change place** lets you type yours.
- **Accuracy:** replayed against Kemenag's published 2026 schedule for all 517 places, 98.2% of 227,367 times match to the minute and none is more than a minute off (`cargo test -p poni-core prayer`; details in [addons/sholat](addons/sholat/README.md)).

## Build

Oyen Poni is a Cargo workspace:

| Crate | What |
|---|---|
| `crates/poni-core` | Pure logic (springs, lyrics parsing, Pomodoro, steps, tasks, settings…), Phone drop's small web server and paired phone drop (pairing, signed messages, the WebRTC link). Unit-tested on any OS. |
| `server` | The meeting server and phone page for paired phones (`drop.oyenza.com`); Node, no dependencies. |
| `crates/poni` | The app. On Windows: Win32 + Direct2D + WinRT through [windows-rs](https://github.com/microsoft/windows-rs). On a Mac (`src/mac/`): AppKit, Core Graphics and Core Text through [objc2](https://github.com/madsmtm/objc2), drawing the same pages. |

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

On a Mac, with Rust stable:

```
cargo build -p poni --release        # target/release/poni
target/release/poni --render shots --notch   # every island state as a picture, with a MacBook's notch
```

`tools/make_mac_app.sh` turns the Apple silicon and Intel builds into `Oyen Poni.app` and its DMG (the `mac` workflow does it for every pull request, push to main and tag). From Linux or Windows, `cargo mcheck` type-checks the Mac app (`rustup target add aarch64-apple-darwin`).

### Testing without clicking around

`poni.exe --selftest <dir>` runs a scripted tour of every state with fake data and writes a screenshot of each one, plus `report.json` with layout and timing checks. The exit code is non-zero on failure. It also works under Wine on Linux (`tools/run-selftest-wine.sh`), which is how the pictures in this README were made, so the font is a stand-in for Segoe UI.

| Variable | Effect |
|---|---|
| `PONI_SELFTEST_DPI=144` | Render at 150% scaling (also 120 and 192) |
| `PONI_SELFTEST_ANIM=1` | Also record the transitions frame by frame (plus each frame's screen position in `anim-<name>.json`) |
| `PONI_RENDERER=software` | Draw in software instead of on the GPU (also outside the self-test, for troubleshooting) |

![The expand animation, frame by frame](docs/screenshots/expand-motion.png)

See [ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces fit: threads, the rendering model, the page and service contracts, and data files.

### Website

The website is `site/index.html`, published with GitHub Pages by `.github/workflows/pages.yml` whenever `site/` or `docs/brand/` changes on main. Every release updates it too ([AGENTS.md](AGENTS.md#every-release-updates-the-website) has the checklist). A page that names a version not released yet waits until that release is published, and a release whose page wasn't updated opens an issue. Preview it with `tools/build_site.sh && python3 -m http.server -d _site 8000`. Its island screenshots come from the self-test at 200 % in both themes, and its Mac pictures from the `mac` workflow's renders (`tools/site_shots.py`).

The README's feature pictures are drawn from those same screenshots by `tools/make_readme_art.py` (from `tools/readme_art.html`) into `docs/brand/readme/`. Rerun it after retaking the screenshots.

### Download stats

[The stats page](https://github.com/lucacadalora/poni/blob/metrics/README.md) shows downloads per release and per day. Every hour `.github/workflows/metrics.yml` reads GitHub's download counters with `tools/download_metrics.py` and keeps the history on the `metrics` branch. Downloads made by our own CI (the install test fetches the 0.1.1 installer on every push) are left out. The app itself counts nothing.

## Status

Early, but in daily use on Windows 11. The Mac app is newer: it runs on GitHub's macOS runners on every change, and a real MacBook's notch, music apps and drag and drop are where it most needs reports. Most rough edges are in places a test machine can't fully exercise: specific media players, Bluetooth devices, and multi-monitor setups. Issues and pull requests are welcome. Please attach `%LOCALAPPDATA%\Poni\poni.log` (on a Mac, `~/Library/Logs/Poni/poni.log`) when something misbehaves.

## Credits

- Synced lyrics come from [LRCLIB](https://lrclib.net), a free and open lyrics database. Oyen Poni fetches them only while a song plays and caches them on your PC.
- With your token, lyrics can also come from [Apple Music](https://music.apple.com) (see [Apple Music lyrics](#apple-music-lyrics)).
- Phone drop follows the design of [Windfall](https://github.com/UvejsGj/Windfall)'s phone page (MIT). QR codes come from [qrcodegen](https://github.com/nayuki/QR-Code-generator) (MIT).
- Romaji under Japanese lyrics comes from Windows' own reading analyzer or, on PCs without Windows' Japanese data, from [mecab-ipadic](https://taku910.github.io/mecab/) read with [vibrato](https://github.com/daac-tools/vibrato) (MIT/Apache-2.0).
- Paired phones follow the idea of [PairDrop](https://github.com/schlagmichdoch/PairDrop)'s paired devices (no code taken). The WebRTC link is [str0m](https://github.com/algesten/str0m) (MIT or Apache-2.0).
- Prayer times use the sun's position from [Adhan](https://github.com/batoulapps/adhan-js) (MIT), Kemenag RI's rules, and reference points fitted to Kemenag's published schedule. The adzan is "Maghrib Adhan at the Masjid al Haram, Mecca - 25 Feb, 2012" by 3omar Faruq, from [Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Maghrib_Adhan_at_the_Masjid_al_Haram,_Mecca_-_25_Feb,_2012.webm), [CC BY 3.0](https://creativecommons.org/licenses/by/3.0/) (cut to the adzan, mono, levelled).
- Built with [windows-rs](https://github.com/microsoft/windows-rs) and [serde](https://serde.rs).

## License

The code is [MIT](LICENSE). The Oyen Poni name, its logo and the Oyen character (the files in `docs/brand`, `installer/art` and the app icon) are not covered by the MIT license, so please don't reuse them in other projects. The Japanese reading dictionary (`crates/poni-core/data/ja-readings.dic.zst`) is derived from mecab-ipadic and keeps its own license. [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt), which ships with every download, holds that license and those of the Unicode data and the Rust crates Oyen Poni is built from.
