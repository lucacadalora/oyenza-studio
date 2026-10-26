# Microsoft Store listing: Oyenza Studio

Text to paste into Partner Center. The package itself comes from the `store` workflow (artifact `oyenza-studio-msix`).

## Before the first upload

1. Create the free developer account at [storedeveloper.microsoft.com](https://storedeveloper.microsoft.com).
2. In Partner Center, reserve the name **Oyenza Studio**.
3. Open **Product identity** and copy three values into `tools/make_msix.ps1`: `Package/Identity/Name` (IdentityName), `Package/Identity/Publisher` (Publisher, starts with `CN=`) and `Package/Properties/PublisherDisplayName`. The Store refuses a package whose identity differs.
4. Push. The `store` workflow builds `Oyenza-Studio-<version>.msix` with those values.

## Product setup

| Field | Value |
|---|---|
| Pricing | Free |
| Markets | All |
| Category | Productivity |
| Website | https://github.com/lucacadalora/oyenza-studio |
| Support contact | https://github.com/lucacadalora/oyenza-studio/issues |
| Privacy policy URL | https://github.com/lucacadalora/oyenza-studio/blob/main/PRIVACY.md |
| Copyright | © 2026 Jatevo Labs |

## Store listing (English)

**Short description**

A living notch for Windows. A small black island at the top of your screen shows your music, timers and reminders, and opens into quick pages when you point at it.

**Description**

Oyenza Studio puts a small black island at the top of your screen. It shows what's happening (music, a focus timer, volume, headphones connecting), and when you point at it, it opens into a set of quick pages.

While collapsed, the island shows album art and an equalizer while music plays, a focus-timer countdown, and pop-ups for volume, charging and Bluetooth headsets with their battery level.

Point at the island or click it to open it, then scroll over the header to switch pages:
- Music: controls for whatever is playing (Spotify, browsers, any app in Windows' media controls), with time-synced lyrics from LRCLIB
- Focus: a Pomodoro timer that keeps counting in the island
- Health: today's steps against your goal and your week, from Apple Health or Health Connect on your phone through your Oyenza account (sign-in coming soon)
- Shelf: drop files on the island to keep them handy, then open them or drag them back out
- Tasks: a tiny to-do list
- Clipboard: recent copied text, kept in memory only
- Today: the date and calendar, plus CPU, memory, disk and battery
- Add-ons (the + at the end of the tabs): extra features that download only when you switch them on. The first is Prayer times: today's times for your city, from Kemenag RI in Indonesia (Umm al-Qura in Makkah, Madinah and Jeddah) and by the method each country follows elsewhere, a countdown before each prayer, and a tone or the adzan when it's time

Native and light: no web view, no account, and no CPU use while nothing moves. Your data stays on your PC.

**What's new in this version**

First release in the Microsoft Store.

**Product features** (one per line in Partner Center)

- Album art, an equalizer and the song title in the island while music plays
- Time-synced lyrics for whatever is playing, from LRCLIB
- Play, pause and skip for Spotify, browsers and any app in Windows' media controls
- A Pomodoro focus timer that keeps counting in the island
- Your steps from your phone, through Oyenza (coming soon)
- A file shelf: drop files on the island, drag them back out later
- A tiny to-do list
- Clipboard history kept in memory only
- Prayer times as an optional add-on: Kemenag RI's times for your city in Indonesia, any city's elsewhere, a countdown and the adzan
- Phone drop: send photos and files from your phone to the Shelf, or Shelf files to your phone, over your Wi-Fi
- Paired phones: pair a phone once, then send to the Shelf from any network
- Volume, charging and Bluetooth headset pop-ups with battery level
- Starts with Windows (you can turn it off from its menu or in Settings)

**Search terms** (up to 7)

dynamic island, notch, lyrics, pomodoro, now playing, steps, clipboard

**Screenshots**

Desktop screenshots must be at least 1366 × 768. Use real captures of the island at the top of a desktop: music with lyrics, the focus timer and the pages.

## Age rating (IARC questionnaire)

Category: utility/productivity. No violence, sexual content, gambling or purchases. Users can't talk to each other or share content. The app doesn't share the user's location and gives no open web access (it only looks up lyrics). Expected rating: 3+ / Everyone.

## Notes for certification

> Oyenza Studio is a desktop app that draws a small "island" at the top centre of the primary screen. No sign-in is needed. Point at the top centre of the screen (or click the island) to open it. Play music in any app that uses Windows media controls (Spotify, Edge) to see the music page and synced lyrics. Lyrics need an internet connection; everything else works offline. Phone drop (Shelf › Phone) serves a page on the local network only while its panel is open, for a phone on the same Wi-Fi. Pair a phone (same panel) lets a paired phone reach the PC from other networks through drop.oyenza.com; with no paired phone the app makes no such connection. "Start with Windows" is the package's startup task (TaskId PoniStartup); users can turn it off from the tray icon's menu or in Settings > Apps > Startup. The optional Glass theme (Settings in the island, or the tray menu: Theme > Glass) copies the part of the screen behind the island's window with DXGI Desktop Duplication so it can draw it as glass (not while the island is hidden or the PC is locked); the copy stays in memory, each new copy replaces the last, and it is never saved or sent, and while the theme is on the island's window is excluded from screen capture (SetWindowDisplayAffinity with WDA_EXCLUDEFROMCAPTURE). Add-ons (the + tab) are off until the user switches one on; then the app downloads that add-on's data files (for Prayer times: a place list in JSON and an AAC audio file, no code) from the project's GitHub releases and checks each against a SHA-256 built into the app. Prayer times looks cities outside Indonesia up as small text files from studio.oyenza.com (the project's website): the ones whose names start with the two letters typed in its place picker, or the ones in the 5-degree squares around the PC's location. It uses the location capability, only while it is on, to pick the nearest city through Windows.Devices.Geolocation; the position is used on the device and never stored or sent.

**runFullTrust justification**

> Oyenza Studio is a Win32 desktop app (Rust, Direct2D). It needs full trust for its always-on-top layered window at the top of the screen, the system volume and media session controls, the clipboard listener for clipboard history, and drag and drop of files to and from the island.
