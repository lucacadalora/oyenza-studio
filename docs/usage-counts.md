# Usage counts

With the person's yes, a copy of Oyenza Studio sends one small report a week: which tabs and features it used, and on how many days. This page is the contract between the app and the server. [PRIVACY.md](../PRIVACY.md) is what people are told, so change it first and keep the two in step.

## The rules

- **Off until yes.** `share_counts` in Settings starts false, and nothing is counted, kept or sent while it is false. Saying no deletes `counts.txt`, a report that hasn't gone and the preview.
- **One ask.** Three days or more after this copy first ran, after a look at the update file, the closed island shows **Share usage counts?** for 30 seconds if nothing else is showing and no update is on offer. Clicking it opens the question in Settings; **No thanks** closes it. It doesn't come back, whatever the answer. The switch stays in the Settings window under About.
- **No ID.** A report has nothing that ties it to a person or to another report. Don't add one, however useful it looks: a random install id, a hash of anything, a time of day or a list of what happened in order all break this. Retention comes from `installed`: how many reports from November's copies arrive in January.
- **Totals only.** The server adds a report to the week's totals and throws it away. Reports, addresses and User-Agents are never written down. The totals have one number per name, never a combination of two, so nothing can narrow down to one person.
- **A closed vocabulary.** The server keeps only names on its lists in `server/counts.mjs`, so no free text can reach the disk. A report of the wrong shape is refused whole.
- **No cost at rest.** No thread and no timer. See below.

## The report

```json
{
  "schema": 1,
  "version": "0.2.27",
  "os": "windows",
  "installed": "2026-11",
  "days_used": 4,
  "tabs": { "media": 4, "shelf": 1 },
  "addons": ["sholat"],
  "features": ["phone_drop"]
}
```

| Field | What it holds |
|---|---|
| `schema` | `1`. |
| `version` | This copy's version. |
| `os` | `windows` or `mac`. |
| `installed` | The month this copy first ran, from when its data folder was made, or `null`. |
| `days_used` | Days of the week (Monday to Sunday, UTC) the island was opened or a counted feature used: 1 to 7. A week with none sends nothing. |
| `tabs` | For each tab opened, on how many of those days. Keys are `PageKind::key`. |
| `addons` | The add-ons on when the week ended (`poni_core::addons`). |
| `features` | Which of `poni_core::counts::FEATURES` were used at least once. |

To count something new, add it to `FEATURES` and `Command::counted_feature`, to the lists in `server/counts.mjs`, and to the Usage counts paragraph in PRIVACY.md, then deploy the server first. Tests in `poni_core::counts` and `crate::counts` fail if the server doesn't list a name.

## How it works

- **Counting** (`crates/poni/src/counts.rs`, the rules in `poni_core::counts`): `pages::draw_expanded` calls `counts::page` and `AppState::cmd` asks `Command::counted_feature`, so both apps share one path. A page already marked isn't looked at again for a minute. A write happens only when a number changes, so at most once per tab per day.
- **Sending:** at the first count or hourly look of a new week, the finished week's report is written to `counts-report.json`. The next hourly look at the update file posts it on the add-on thread (`services::update::check_job`), and an answer deletes it. A 429, a 5xx or no connection leaves it for the next look. A later week replaces one that never got out. The look rests while nobody is at the computer, so reports do too.
- **Microsoft Store copies** don't look at the update file, so they don't ask or send.
- **The server:** `POST https://drop.oyenza.com/api/counts`, in `server/counts.mjs`. At most 30 reports an hour from one address, kept in memory only, and 4 KB each. Totals are in `counts/<ISO week>.json` in the server's data folder, by the week a report arrived, so a report covers the week before.

## Reading the totals

On the server:

```bash
node server/counts-view.mjs --data <the server's data folder>
```

`--week 2026-W46` picks a week. Any number under 20 reads "under 20", and a week with fewer than 20 reports shows only that. A group that small could be one person.

People who say yes aren't everyone, so read these as shares of that group. How many copies there are in all comes from the running count, not from here.
