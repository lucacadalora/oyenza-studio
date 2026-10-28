# The update file

Every copy of Oyenza Studio learns that a new version is out from one small file, <https://studio.oyenza.com/latest.json>. This page says what is in it, where it is made, and how it stays trustworthy.

## What's in it

```json
{
  "about": "Oyenza Studio's update file. …",
  "payload": "{\"schema\":1,\"version\":\"0.2.3\",\"date\":\"2026-10-25\",\"headline\":\"Sing along from the closed island\",…}",
  "signature": "base64 of r and s, 64 bytes",
  "key": "4cb3280eaea65b50"
}
```

`payload` is JSON kept as a string, so its exact bytes are the ones that were signed. Inside it:

| Field | What it holds |
|---|---|
| `schema` | `1`. A copy that doesn't know a newer schema ignores the file. |
| `version` | The latest release, such as `0.2.3`. |
| `date` | The day it was published (UTC), such as `2026-10-25`. |
| `headline` | The first bold line of its release notes, at most 90 characters. The island shows it. |
| `notes` | The release's page on GitHub. |
| `windows` | `url`, `size` and `sha256` of `Oyenza-Studio-Setup-<version>.exe`, from that release. |
| `mac` | The same for `Oyenza-Studio-<version>-mac.dmg`. It's left out until the DMG is on the release. |

`signature` is ECDSA P-256 over the SHA-256 of the payload's UTF-8 bytes: r and s, 32 bytes each, big-endian, then base64. `key` names the key that made it.

## Where it is made

The Website workflow (`.github/workflows/pages.yml`) runs `tools/make_update_feed.py` in its `feed` job and puts the file into every deploy. A Pages deploy replaces the whole site, so each deploy needs a fresh copy. If the `feed` job fails, nothing is deployed, and the site and file already live stay live.

The file always describes GitHub's latest release, so a release updates it without a step of yours: the tag's `ci` run starts the Website workflow once the release is published, and the `mac` workflow starts it again once the DMG is on the release, which is when Macs get told.

To see what the file would say without a key: `python3 tools/make_update_feed.py --check`.

## The keys

`crates/poni-core/data/update-keys.txt` lists the public keys the app trusts, and the app is built with that list. The script refuses to sign with a key that isn't on it.

- **The first key signs.** Its private half is the `UPDATE_FEED_KEY` secret in the `release` environment, which only `main` and `v*` tags can use. Never print it or move it out of that environment.
- **The second key is the spare.** Its private half is kept offline by the maintainer. Copies already out there trust both keys.
- **If the first key leaks or is lost,** put the spare in `UPDATE_FEED_KEY`, and in the next release replace the old first key in `update-keys.txt` with a new spare. Every copy keeps hearing about updates.
- GitHub never shows a secret again after it's saved. If both private halves were lost, copies that trust only those keys could never be told about an update again; they would need the website, as before.

## Taking a release back

The file follows GitHub's latest release. If a release turns out bad, mark the previous release as the latest in the release list, then run the Website workflow (Actions › Website › Run workflow). Copies that haven't updated stop hearing about the bad one; copies already on it wait for the next release.

## What the app does with it

Copies from 0.2.6 on read the file; 0.2.5 and older don't, so they need one last update by hand. The checks are always on. Earlier versions had a **Tell me about new versions** switch in Settings (on by default); it is gone, and the `update_check` it left in settings files is ignored, so a copy that had it off looks again.

- **When it looks:** about two minutes after start, then every hour (`poni_core::update::CHECK_EVERY_S`), remembered across starts in `update.json` in the data folder, so starting the app often costs nothing. Releases often come out several a day; an hourly look tells everyone at their computer the same hour. A look that comes due while nobody is at the computer (locked, display off) or on battery saver waits until someone is back. A failed look tries again in an hour.
- **Check for updates…** in the menu (right-click and tray on Windows, the menu bar on a Mac, under About) looks at once, even on battery saver, and says what it found on the island: **Oyenza Studio is up to date** with the version, **Couldn't check for updates**, or, for a new version, the Update sheet. The sheet waits for the download (the Setup on Windows; on a Mac that can replace itself, the app out of the DMG), with **Downloading x** on the island meanwhile, and opens by itself if it is in within two minutes; otherwise the usual notice tells. A version found this way gets no notice afterwards. The line is there while nothing is on offer (then the menu starts with **Update to Oyenza Studio x…**).
- **Check now** in Settings, at the right of the **Version** row, does the same while the page is open, and answers in its place: **Checking…**, **Up to date**, or **Try again** (orange) when it couldn't look. A new version gets Settings' last row with **Update** at once, and the sheet opens as from the menu. It is words with a wash on hover, not a pill. It shows while nothing is on offer, and not in a Microsoft Store copy.
- **How:** one request for the file, about 1 KB, on the add-on downloader's thread (`services/update.rs`), which sleeps the rest of the time. The signature is checked with the system's own crypto (CNG on Windows; on a Mac the P-256 code phone drop already builds in), then `poni_core::update::parse_payload` refuses anything that isn't this repository's own release of that version.
- **Windows downloads first, then tells:** once a look finds a later version, the add-on thread downloads its Setup (about 2.5 MB, once per release) into `update` in the cache folder (`%LOCALAPPDATA%\Poni\update`) and keeps it only if its size and SHA-256 match the signed file's. Older versions' Setups there are deleted, and all of them once the running version is the latest. A download that fails doesn't hold the news back: the island tells anyway, the sheet offers the release page, and the next look tries again.
- **A Mac downloads first too, where it can replace itself:** the add-on thread downloads the DMG (about 4.3 MB, once per release) into `update` in the cache folder (`~/Library/Caches/Poni/update`), keeps it only if its size and SHA-256 match, opens it out of sight (`hdiutil attach -nobrowse`), copies the app out next to it (`ditto`) and checks that copy: the bundle id `com.oyenza.poni`, the offered version, an intact signature (`codesign --verify --deep --strict`), and the running copy's developer team once it is signed with a Developer ID (`services::update::stage`). Older versions' DMGs and apps there are deleted like the Setups. A copy that can't be replaced where it is (opened straight from the DMG, opened from a temporary place macOS moved it to, or in a folder this account can't change; `services::update::can_install`) downloads nothing ahead, and tells at once with the DMG to download, as before.
- **The island:** a later version than the one running, with a download for this system (a Mac waits for the DMG to be on the release), shows as **Update available x** (the version dimmer), over the release's headline, with **Hide** (`crate::updates`). It waits until the island is closed and free, then stays until it is clicked (the Update sheet) or put away with **Hide**. If another notice takes its place, or the app quits while it is up, it comes back. A volume or brightness change and a game's readout show over it while they last (`Notice::stays`), and hovering doesn't open the island while it shows, so Hide can be clicked. Opening the Update sheet any other way (the menu's **Update to** line, Settings' **Update**) counts as seen too.
- **The Update sheet** (in Settings, in place of its rows): what's new, what gets the new version, and **See what's new in Oyenza Studio x** (the release page). **Not now** closes the island. With the new version in (the Setup on Windows, the checked app on a Mac), it reads **Update ready to install**, with two cards:
  - **Install now**, Windows: checks the Setup again and runs it silently (`/S /reopen`); the app quits as on any quit, saving first. The Setup replaces `poni.exe`, leaves Start with Windows as it was, and opens the new version.
  - **Install now**, Mac: checks the app again, copies it next to the running one (a hidden `.Oyenza Studio update.app` in the same folder, so on the same disk), swaps the two in one step (`renamex_np` with `RENAME_SWAP`; two renames on a disk that can't swap) and quits, saving first. A small shell, started out of the app's process group, waits for it to be gone (at most 30 seconds), deletes the old copy and opens the new one. If the swap fails, nothing has changed: the sheet opens the downloaded DMG (no second download, and no Open Anyway, since it never came through a browser) and the island says to drag Oyenza Studio to Applications.
  - **Install on next start** remembers the version (`install_on_start` in `update.json`). The next start installs it the same way before any window opens, once: an install that fails doesn't keep the app from opening. A Mac takes the app out of the DMG then if it isn't out yet.
- **Until a Mac copy is signed with a Developer ID** ([mac-signing.md](mac-signing.md)), macOS sees every version as a different app: after an update it may ask again for what it asked before (controlling Music or Spotify, the location for Prayer times), as after an update by hand. macOS 13 and later also guard apps against changes by other apps, a rule written for developer teams; how it treats an ad-hoc signed copy replacing itself is for a real Mac to show, and if it says no, the DMG opens as above. `tools/mac-update-test.sh` runs the whole install on CI's Mac (an installed copy, a newer version in a release-style DMG, Install on next start).
- **Until it is installed:** the right-click menu starts with **Update to Oyenza Studio x…**, which opens the sheet, and Settings has a last row, **Update available x**, with **Update**.
- **Without the download** (it failed, or a Mac copy can't replace itself): the card opens the release's page on Windows. A Mac downloads the DMG to drag by hand.
- **The installer's side** (`installer/poni.nsi`): in silent mode it leaves the Start with Windows component out, so the Run value stays as the person set it, and `/reopen` starts `poni.exe` once it's done (the finish page's "Launch Oyenza Studio now" never shows in silent mode). The CI installer check runs exactly this over a running copy.
- **After updating:** the first start of a newer version says **Updated to x** once, with **What's new**.
- A Microsoft Store copy doesn't look: the Store updates it.
