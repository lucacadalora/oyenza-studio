# The Downloader add-on

The Downloader saves the video or the sound of a link you paste. It's an add-on, off until you switch it on in Add-ons (the "+" at the end of the tab row); then a **Downloads** tab shows. Paste a link, pick **Video** or **Sound** and click **Download**. Each download gets a row with its progress and **Stop** (**Cancel** while it waits its turn, which takes it off the list), then **Open** and **Show in folder** once it's done. While something downloads, the closed island shows its progress in a small pill.

It's on Windows and the Mac (macOS 14 and later, Apple silicon and Intel). The Microsoft Store build doesn't offer it.

It's part of Studio Pro: it switches on, and its tools download, only for an Oyenza account the Oyenza server says has Pro. Signed out or without Pro, its row in Add-ons stays off and says what's missing.

[yt-dlp](https://github.com/yt-dlp/yt-dlp) does the work. This page says what the add-on downloads, what runs and when, what goes online, and how yt-dlp is kept up to date.

## What switching it on downloads

| File | Size | What it is |
|---|---|---|
| `yt-dlp.exe` | 23.6 MB | yt-dlp 2026.10.26, which reads the link and downloads the video or sound. From yt-dlp's own release on GitHub, unchanged. |
| `qjs.exe` | 2.2 MB | [QuickJS-ng](https://github.com/quickjs-ng/quickjs) 0.17.0, a small JavaScript engine that yt-dlp runs for YouTube. From QuickJS-ng's own release on GitHub, unchanged. |
| `ffmpeg.exe` | a few MB | A minimal [FFmpeg](https://ffmpeg.org) that joins a video's picture and sound into one file. Our own build, made by `tools/build-ffmpeg-min.sh` ([FFMPEG.md](../addons/downloader/FFMPEG.md)), on this project's `addon-downloader-1` pre-release. |
| `NOTICE.txt` | 33 KB | Where each one comes from, and their licenses ([addons/downloader/NOTICE.txt](../addons/downloader/NOTICE.txt)). On the same pre-release. |

On a Mac:

| File | Size | What it is |
|---|---|---|
| `yt-dlp` | 37.5 MB | The same yt-dlp, its own Mac build (`yt-dlp_macos`, one file for Apple silicon and Intel). From yt-dlp's own release on GitHub, unchanged. |
| `qjs` | 2.5 MB | The same QuickJS-ng, built by us from its source, unchanged, for Apple silicon and Intel and macOS 14 and later: QuickJS-ng's own Mac builds are one per chip and need macOS 26. |
| `ffmpeg` | 4.3 MB | The same minimal FFmpeg, built for the Mac by `tools/build-ffmpeg-min.sh` (its `darwin` target). |
| `NOTICE.txt` | 33 KB | [addons/downloader-mac/NOTICE.txt](../addons/downloader-mac/NOTICE.txt). |

`qjs`, `ffmpeg` and the notice are on this project's `addon-downloader-mac-1` pre-release. mac.yml's `downloader-tools` job builds the two tools, signed ad-hoc (a Mac with Apple silicon runs only signed code), and checks that each runs and links nothing but macOS's own libraries.

They go in `%LOCALAPPDATA%\Poni\addons\downloader\`, or `~/Library/Caches/Poni/addons/downloader/` on a Mac. Each file's size and SHA-256 are built into the app (`poni_core::addons::DOWNLOADER_WINDOWS` and `DOWNLOADER_MAC`), and a file that doesn't match is refused. On a Mac the app makes the three tools runnable as it saves them; files it writes itself carry no quarantine, so macOS doesn't ask about them. Switching the add-on off deletes them.

## What runs, and when

Nothing runs at rest. With the add-on on and nothing downloading, it has no program, thread or timer going.

- **A download** starts one `yt-dlp.exe`, without a window, at below normal priority. yt-dlp starts `qjs.exe` when YouTube's JavaScript needs answering, and `ffmpeg.exe` to join the picture and the sound once both are in. They all end when the download does. A Mac runs `yt-dlp`, `qjs` and `ffmpeg` the same way.
- **Two at a time:** a third download waits for a free place (`poni_core::downloads::AT_ONCE`).
- **Stop** ends a download's programs at once, and so does quitting Oyenza Studio: each yt-dlp runs in a Windows job object, which ends it with everything it started. On a Mac each yt-dlp leads a process group of its own, which what it starts joins; Stop asks the whole group to end, and ends what is left of it 2 seconds later. Stop then deletes what yt-dlp had written of the file (its `.part` and `.ytdl` files); after quitting, they stay in the Downloads folder.
- **The progress** comes from yt-dlp's own lines, read by one thread per running download, which ends with it.

## Where the files go

Finished files go to your Downloads folder, and onto the Shelf. **Open** opens a file in its app, and **Show in folder** shows it in File Explorer (**Show in Finder** on a Mac). The Downloads tab's list is kept in memory only: it's empty after a restart, and the links you pasted aren't saved.

## What goes online

- **The site of the link you paste.** yt-dlp asks it for the page, then downloads the video or sound from wherever that site keeps it. The requests go straight from your PC to the site, as your browser's would.
- **github.com, for the tools:** when you switch the add-on on, and when an Oyenza Studio release pins new ones.
- **api.oyenza.app, whether your account has Pro:** with the add-on on, once the sign-in is there (at start, or as you sign in), and when Add-ons opens. The request carries this PC's Oyenza key and nothing about your downloads; the answer is a yes or a no.
- **Nothing else.** Nothing about what you download goes to us or to anyone but that site. yt-dlp doesn't get your browser's cookies or sign-ins.

## How yt-dlp is kept up to date

Sites change, and yt-dlp follows them with new versions. Here it's updated with Oyenza Studio: each release pins one yt-dlp, one QuickJS-ng and one FFmpeg, by size and SHA-256, in `poni_core::addons::DOWNLOADER_WINDOWS` and `DOWNLOADER_MAC` (the same versions on both), and yt-dlp never updates itself. When a release pins new ones, PCs with the add-on on download them once that release starts, and the old ones are deleted. A site that changed and stopped working works again with the first release that pins a yt-dlp that reads it.

To move to a new yt-dlp: its size is on its release page, and its SHA-256 in that release's `SHA2-256SUMS`, which yt-dlp signs with its [key](https://github.com/yt-dlp/yt-dlp/blob/master/public.key). `tools/pin_ytdlp.py` puts the new URLs, sizes and SHA-256s of `yt-dlp.exe` and `yt-dlp_macos` in `poni_core::addons` together ([downloader-pins.md](downloader-pins.md)). A changed `NOTICE.txt` goes on a new tag (`addon-downloader-2`, or `addon-downloader-mac-2` for the Mac's) with that add-on's own files, like any add-on file, and ships with an app release. So does a new QuickJS-ng or FFmpeg for the Mac: change `QJS_COMMIT` or the script, take the two files from the `downloader-tools-mac` artifact of mac.yml's run, put them on the new tag and pin them in `DOWNLOADER_MAC`.

## What you download is up to you

The Downloader saves what a site already plays for you, and what you save with it is your responsibility. Download only what you have the right to keep, such as your own videos or ones under an open license, and respect copyright and each site's terms. It can't save what a site protects with DRM, and it doesn't try.
