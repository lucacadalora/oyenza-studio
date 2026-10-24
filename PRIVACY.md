# Oyen Poni privacy

Oyen Poni is made by Jatevo Labs. It needs no account, and has no ads, no analytics and no tracking. Unless you sign in to Oyenza, it never sends us anything.

## What leaves your PC

**Lyrics lookups.** When a song plays and **Synced lyrics** is on, Oyen Poni asks [LRCLIB](https://lrclib.net), a free public lyrics library, for that song's lyrics. The request carries the song's title, artist, album and length, plus a User-Agent that names Poni and links to this project. LRCLIB is run by other people and has its own terms. Answers are cached on your PC, so a song isn't looked up again every time it plays.

To stop lookups, right-click the island or the tray icon and turn off **Synced lyrics**.

**Apple Music lyrics, only if you turn them on.** If you add your Apple Music token (right-click the island, then **Apple Music lyrics › Use token from clipboard**), Oyen Poni also asks Apple Music for lyrics the way Apple's web player does. It checks the token, searches Apple Music's catalog for the playing song's artist and title, and downloads that song's lyrics. These requests go to Apple (`music.apple.com` and `amp-api.music.apple.com`) and carry your token; the token goes nowhere else. To stop, choose **Remove token** in the same menu.

**Phone drop, only while its panel is open.** When you open Phone drop on the Shelf, Oyen Poni serves a small web page on your local network so your phone can send files to the Shelf and save Shelf files. Only a device on the same network with that session's secret link, which only the QR code carries, can open it. Files go straight between your phone and your PC: nothing passes through the internet or through us. Oyen Poni reads nothing from an upload until you click **Accept**. The page stops working when you click **Done**, or after 10 minutes without a phone.

**Oyenza, only if you sign in.** Signing in (the gear on the open island, then **Sign in with Oyenza**; or right-click the island, then **Oyenza › Sign in to Oyenza…**) connects this PC to your [oyenza.app](https://oyenza.app) account: you approve a code in oyenza.app, and Oyen Poni gets a key of its own. It then asks `api.oyenza.app`, about once a minute while you use the PC (every 3 minutes on battery, never while it's locked or asleep), for money that arrived and bills due in the next 7 days, and sends the spends you type in the island. It sends its key, its version and your PC's UTC offset with each request, and the PC's name once, when you sign in. The key can't read your history, change or delete anything else, or touch your account. Oyenza's own privacy page covers what happens there: [oyenza.com/privasi](https://oyenza.com/privasi). **Disconnect** in Settings (or **Oyenza › Disconnect** in the menu) signs this PC out and removes the key and everything Oyen Poni kept from Oyenza; you can also disconnect it in oyenza.app.

Nothing else is sent anywhere. Music, volume, Bluetooth battery levels, the date and calendar, and the system stats are all read from Windows on your PC.

**The Glass theme reads the screen behind the island, only while it's on.** To bend what's under the island, Oyen Poni copies the part of your screen behind its window (the island and the space it opens into) whenever something there changes. It doesn't copy while the island is hidden, while a full-screen app hides it, or while the PC is locked. The copy stays in your PC's memory and on its graphics card, and each new copy replaces the last. It is never saved, never shown anywhere else and never sent anywhere. While Glass is on, Windows leaves Oyen Poni out of screenshots, screen recordings and screen sharing. Switch to Classic or Oyen to stop both.

## What stays on your PC

| What | Where |
|---|---|
| Settings, tasks, focus count and the shelf's list of files (the files themselves stay where they are), and any drinks logged by the Water page of older versions | `%APPDATA%\Poni\` |
| Lyrics cache and a log file (the log names songs that were looked up, to help fix problems) | `%LOCALAPPDATA%\Poni\` |
| Your Apple Music token, if you added one, sealed so only your Windows account on this PC can read it | `%LOCALAPPDATA%\Poni\` |
| Your Oyenza sign-in (this PC's key and your account's masked email), if you signed in, sealed the same way. What Oyenza showed is kept in memory only | `%LOCALAPPDATA%\Poni\` |
| Clipboard history | Memory only, never written to disk. Text that password managers mark as private is skipped. |
| The Glass theme's copy of the screen behind the island, only while Glass is on | Memory only (the PC's and its graphics card's), replaced by each new copy, never written to disk |
| Files you accepted from your phone with Phone drop | `Downloads\Oyen Poni\`, kept when you uninstall |

Uninstalling removes the app, its cache, its log, the Apple Music token and the Oyenza sign-in (the key also stops working on Oyenza's side after 90 days unused, or at once if you disconnect it in oyenza.app). If you installed from the Microsoft Store, Windows also removes the files the app created. Data from an earlier copy installed from GitHub stays in `%APPDATA%\Poni\` until you delete it.

## Microsoft Store

If you install from the Microsoft Store, Microsoft handles the download and updates under the [Microsoft Privacy Statement](https://privacy.microsoft.com/privacystatement), and gives publishers summary reports such as install counts and crash reports.

## Questions

Open an issue at [github.com/lucacadalora/poni/issues](https://github.com/lucacadalora/poni/issues).
