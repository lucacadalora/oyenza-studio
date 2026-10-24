# Japanese readings add-on

Japanese readings is off by default and most people never need it. It gives
romaji under the kanji in Japanese lyrics on PCs where Windows can't read
them: Windows keeps its Japanese reading data in the Japanese typing
feature, which most PCs outside Japan don't have. Kana, Korean and Chinese
read without it.

When a Japanese song's kanji go unread, the Music page offers it under the
lyrics ("Romaji for kanji: get Japanese readings"). One click switches it on,
like its switch in the Add-ons tab (the "+" at the end of the tab row); the
song is read again once the file is in place. Switching it off deletes it.
The code that reads with it (`crates/poni-core/src/romanize/ja_dict.rs`,
`crates/poni/src/services/romanize.rs`) is part of poni.exe and loads the
dictionary only for a Japanese song, about 30 MB for that song.

The files are published, byte for byte, on the pre-release
[`addon-ja-1`](https://github.com/lucacadalora/poni/releases/tag/addon-ja-1).
Each one's size and SHA-256 are built into the app (`poni_core::addons::JAPANESE_READINGS`),
and `cargo test -p poni-core addons` checks that they match the files here.

- `ja-readings.dic.zst` is `crates/poni-core/data/ja-readings.dic.zst`:
  mecab-ipadic trimmed for lyrics, with Oyen Poni's fixes
  (`tools/ja_dict_fixes.csv`), compiled for vibrato and packed with zstd by
  `tools/make_ja_dict.py`. The `ja-dict` workflow checks that the file is
  exactly what the script builds.
- `NOTICE.txt` (here): where it comes from and mecab-ipadic's licence, which
  every copy must carry.

Changing the dictionary (a rebuild, a new fix) means a new tag
(`addon-ja-2`) holding the new file and this NOTICE.txt, the new size and
SHA-256 in `poni_core::addons`, and an app release. PCs with the old file
download the new one when that release starts; the old tag's folder is
deleted.

The Mac app has no Add-ons yet, so it still carries the dictionary built in
(the `ja-dict` feature of poni-core, macOS only).
