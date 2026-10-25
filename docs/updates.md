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

No released version reads the file yet. The app's side comes next: it checks the file a couple of minutes after it starts and then every 12 hours, never while nobody is at the PC or on battery saver, believes it only if the signature checks out, and then tells you on the island once per version.
