# MediaRemote Adapter (vendored)

How the Mac app reads macOS's Now Playing, the player Control Center shows (Spotify, Music, YouTube in
any browser, and every other app that reports there). Since macOS 15.4 MediaRemote answers only
Apple's own programs, so `/usr/bin/perl` (which macOS counts as one) runs this project's script, and
the script loads its framework and prints each change as a line of JSON.

- Source: https://github.com/ungive/mediaremote-adapter, tag `v0.7.7` (commit `e3ff502`), last tested
  by its author on macOS 27.0. BSD-3-Clause, see [LICENSE](LICENSE) (also in THIRD-PARTY-NOTICES.txt).
- Copied unchanged: `CMakeLists.txt`, `LICENSE`, `bin/mediaremote-adapter.pl`, `include/` and `src/`
  from that tag (`git archive v0.7.7`). Only this README is ours.
- `tools/make_mac_app.sh` builds the framework (Apple Silicon and Intel, macOS 14 and up) and puts it
  in `Oyenza Studio.app/Contents/Frameworks`, with the script in `Contents/Resources`;
  `tools/sign_mac_dmg.sh` signs it before the app.
- The app runs `stream` through `installer/mac/now-playing.pl`, which ends it when the app quits or
  crashes, and `send` / `seek` for the island's buttons
  (`crates/poni/src/mac/services/media.rs`; the stream's lines are read by
  `crates/poni-core/src/mediaremote.rs`).

To update: replace those files with the new tag's (`git archive <tag> src include bin/mediaremote-adapter.pl
CMakeLists.txt LICENSE`), read the upstream changes to `stream`'s output, update the version here and in
THIRD-PARTY-NOTICES.txt, and let the `mac` workflow's smoke run check it (its `nowplaying-*` steps).
