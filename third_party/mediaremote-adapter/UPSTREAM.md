# MediaRemote Adapter

From https://github.com/ungive/mediaremote-adapter at commit
`0e82bf1c44ec39554fbfc402ec88a48c6ae2a0fd` (17 Oct 2026), unchanged: `LICENSE`
(BSD 3-Clause), `CMakeLists.txt`, `bin/`, `include/` and `src/`. Its notice is in
THIRD-PARTY-NOTICES.txt.

The Mac app reads macOS's own now playing with it (crates/poni/src/mac/services/mediaremote.rs):
since macOS 15.4 that service answers Apple's own programs only, and `/usr/bin/perl`
is one, so perl loads the adapter's framework and runs its `stream`, `send` and `seek`
commands for the app.

mac.yml builds it with CMake (`MediaRemoteAdapter.framework`, Apple Silicon and Intel,
and `MediaRemoteAdapterTestClient`, a test player for the smoke run that never goes in
the app); tools/make_mac_app.sh puts the framework in `Contents/Frameworks` and the
script in `Contents/Resources`.

To update it, copy those files from a newer commit, change the commit here and in
THIRD-PARTY-NOTICES.txt, and check the smoke run's `now-playing-service` check on
macOS 15 and 26.
