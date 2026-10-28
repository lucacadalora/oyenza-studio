# AI usage

The **AI** tab shows how much is left of the AI coding plans on your computer, and your Oyenza Inference balance once you connect it. This page says, for each service, what Oyenza Studio reads on your computer, which server it asks, how often, what the card shows, and whether it needs a key you type. [PRIVACY.md](../PRIVACY.md) has the short version.

## When it reads

- **Nothing at rest.** Nothing is read until the AI tab opens or you switch on heads-up notices. From then on, one worker thread does the reading, and it sleeps whenever nothing needs it.
- **While the tab is open:** this computer's files right away (at most once every 15 seconds if you keep opening it), then every minute; each service's server at most as often as the table below says.
- **With heads-up notices on** (**Tell me before I run out**, off until you switch it on): while the tab is closed, the files every 5 minutes and each server at most every 10 minutes, so the island can say when a limit is nearly used up (80% by default), when it runs out, when it's back after its reset, and when a balance drops under $1.
- **It rests** while the computer is locked or the screen is off. The notices also rest on battery saver.
- **When a service pushes back:** after "too many requests" it waits as long as the service asks (5 minutes if it doesn't say); after no answer at all, 1, 2, 5, then 10 minutes.
- **A refresh makes at most 4 requests** to one service.

## What every reader keeps to

- **Read only.** It never writes another app's files and never renews another app's sign-in: a renewal can sign you out of that app. When a sign-in has run out, the card keeps the last numbers it had and asks you to open that app once.
- **A sign-in goes only to its own service's server.** Never to Oyenza or anyone else, and never into the log, an error message or the numbers kept for the card.
- **It asks as Oyenza Studio.** Every request carries the User-Agent `OyenzaStudio/<version> (+https://github.com/lucacadalora/oyenza-studio)`, never one that pretends to be the service's own app. A service that answers only its own app shows as refused; Oyenza Studio doesn't work around it.
- **Unknown is not zero.** A number the service doesn't give stays off the card.
- **Nothing about your usage goes to Oyenza.** The numbers come from each service to your computer and stay in Oyenza Studio's memory.

## The services

`~` is your user folder: `C:\Users\<you>` on Windows, `/Users/<you>` on a Mac. Paths are the same on both unless the service's section says otherwise.

| Service | Read on this computer | Asks | Online at most every (tab open) | Typed key |
|---|---|---|---|---|
| [Codex](#codex) | `~/.codex/auth.json`, the end of its newest session logs | `chatgpt.com` | 2 minutes | No |
| [Claude Code](#claude-code) | `~/.claude/.credentials.json` (on a Mac, the keychain first), `~/.claude.json` | `api.anthropic.com` | 2 minutes | No |
| [OpenCode Go](#opencode-go) | `~/.local/share/opencode/auth.json`, `OPENCODE_API_KEY` | `opencode.ai` | 2 minutes | Optional |
| [Gemini CLI](#gemini-cli) | `~/.gemini/oauth_creds.json`, `~/.gemini/settings.json` | `cloudcode-pa.googleapis.com` | 5 minutes | No |
| [Cursor](#cursor) | The sign-in's pieces of `state.vscdb` | `cursor.com` | 5 minutes | No |
| [GitHub Copilot](#github-copilot) | Copilot's `apps.json` or `hosts.json` | `api.github.com` | 5 minutes | No |
| [Kimi Code](#kimi-code) | `~/.kimi-code/credentials/` | `api.kimi.com` or `api.kimi.ai` | 5 minutes | Optional |
| [Z.ai GLM](#zai-glm) | `Z_AI_API_KEY` | `api.z.ai` | 5 minutes | Needed, or the variable |
| [MiniMax](#minimax) | `MINIMAX_CODING_API_KEY`, `MINIMAX_API_KEY` | `api.minimax.io` | 5 minutes | Needed, or a variable |
| [OpenRouter](#openrouter) | `OPENROUTER_API_KEY` | `openrouter.ai` | 2 minutes | Needed, or the variable |
| [Oyenza Inference](#oyenza-inference) | The key from **Connect** | `inference.oyenza.com` | 1 minute | No: **Connect** |

With the tab closed and notices on, every server is asked at most every 10 minutes. A service that isn't on this computer and has no key gets no card and no request.

### Codex

- **Reads:** Codex's sign-in, `~/.codex/auth.json` (its ChatGPT access token, account id, and the email and plan inside its id token), and the last 128 KB of the four newest session logs in `~/.codex/sessions/`, for the latest limits Codex itself wrote there. Those logs hold your Codex sessions too: only the limit lines are used, and nothing else in them is kept or sent. With `CODEX_HOME` set, both are read under it.
- **Asks:** `GET https://chatgpt.com/backend-api/wham/usage`, with the access token and the ChatGPT account id, as Codex does. One request a refresh. Codex's `chatgpt_base_url` setting isn't followed: the token only ever goes to chatgpt.com.
- **How often:** the logs every minute while the tab is open, chatgpt.com at most every 2 minutes.
- **The card:** the plan (Free, Plus, Pro, Team, Business, Enterprise and the rest), the account's email (masked), **5-hour** and **Weekly** with when each resets, and credits left on plans that have them. Offline, or once its sign-in has run out, it shows the numbers from the logs, with how old they are.
- **Typed key:** none.
- **Not shown:** a Codex signed in with an API key, a personal access token or Amazon Bedrock (paid per use, no plan limits). Codex inside WSL isn't seen unless Windows' `CODEX_HOME` points at it (`\\wsl.localhost\<distro>\home\<you>\.codex`).

### Claude Code

- **Reads:** Claude Code's claude.ai sign-in (its access token, when that runs out, and the plan): on a Mac from the `Claude Code-credentials` item in your keychain, else from `~/.claude/.credentials.json`, in the order Claude Code itself looks; on Windows, only the file. Also its settings, `~/.claude.json` (or `~/.claude/.config.json` while an older version's copy is there), for two things only: your account's email, shown masked, and whether Claude Code is installed, so the card can say you're signed out. With `CLAUDE_CONFIG_DIR` set, the files are read there and the keychain item is the one Claude Code names after that folder (`Claude Code-credentials-` and 8 letters and digits). The sign-in's refresh token is never read.
- **Asks:** `GET https://api.anthropic.com/api/oauth/usage`, the address Claude Code's own `/usage` reads, with the access token. One request a refresh.
- **How often:** at most every 2 minutes while the tab is open. There is nothing to read offline.
- **The card:** the plan (Pro, Max 5x, Max 20x, Team, Enterprise), the account's email (masked), **5-hour** and **Weekly** with when each resets, a weekly bar for each model your plan limits on its own (**Weekly · Opus**, **Weekly · Sonnet** and the like), extra usage this month against its cap when you've turned it on, and a credit balance for Claude's cloud sessions when Anthropic has given you one.
- **Typed key:** none.
- **Not read:** Claude Code's logs (they carry no running total), and a sign-in kept in Windows' Credential Manager, which newer Claude Code versions can use instead of the file: such a computer reads as signed out for now.

### OpenCode Go

- **Reads:** a key, the first one found: the one you typed in the AI tab, the OpenCode Go key in OpenCode's `~/.local/share/opencode/auth.json` (the same path on Windows; under `XDG_DATA_HOME` if you set it), then the `OPENCODE_API_KEY` variable.
- **Asks:** `opencode.ai`, with the key: `GET /zen/go/v1/usage` for Go's limits, then `GET /console/api/billing/status` for your Zen balance. Two requests a refresh.
- **How often:** at most every 2 minutes while the tab is open. There is nothing to read offline.
- **The card:** the plan (Go), **5-hour**, **Weekly** and **Monthly** with when each resets (a limit that has stopped you reads 100% until it resets), and your Zen balance, when the key may read billing.
- **Typed key:** optional, made in the [OpenCode console](https://opencode.ai/console). OpenCode 2 keeps its keys in its own database, which isn't read, so with OpenCode 2 type the key (a copy upgraded from OpenCode 1 still has its old `auth.json`).

### Gemini CLI

- **Reads:** Gemini CLI's Google sign-in, `~/.gemini/oauth_creds.json`, and `~/.gemini/settings.json`, to see how you signed in (both under `GEMINI_CLI_HOME` if you set it). Also the `GOOGLE_CLOUD_PROJECT` (or `GOOGLE_CLOUD_PROJECT_ID`) variable, the Cloud project Gemini CLI itself sends.
- **Asks:** Google's Code Assist server, `cloudcode-pa.googleapis.com`, with the sign-in's access token: `loadCodeAssist` for your plan and project, then `retrieveUserQuota` for what's left. Two requests a refresh. The sign-in's refresh token is never used or sent.
- **How often:** at most every 5 minutes while the tab is open.
- **The card:** the plan (Free, Standard, AI Pro and so on), the account's email (masked), and a bar each for **Pro**, **Flash** and **Flash Lite**, grouped the way Gemini CLI's `/stats` groups them, with when each resets.
- **Typed key:** none.
- **Not shown:** Gemini CLI signed in with a Gemini API key, Vertex AI or a cloud machine's own sign-in (paid per call, no plan limits). A sign-in Gemini CLI keeps in the system's keychain (only if you ask it to) isn't read, so the card says you're signed out. Google's sign-in lasts about an hour and Gemini CLI renews it only while it runs, so the card often asks you to open Gemini CLI once. For accounts Google no longer serves in Gemini CLI (personal accounts since June 2026), the card says so.

### Cursor

- **Reads:** Cursor's sign-in from its state database, `state.vscdb`, in `%APPDATA%\Cursor\User\globalStorage\` on Windows and `~/Library/Application Support/Cursor/User/globalStorage/` on a Mac (under `VSCODE_APPDATA` if you set it): the access token, the cached email and the plan. That file also keeps your Cursor chats and can grow to many gigabytes, so Oyenza Studio reads only the few small pieces of it (about ten, of 4 KB each) that lead to those entries, never the whole file or the chats, and without locking or writing it, so Cursor carries on undisturbed.
- **Asks:** `cursor.com`, with the sign-in as the cookie Cursor's own site uses: `GET /api/usage-summary`, then `GET /api/usage?user=<your id>` for the older plans counted in requests. Two requests a refresh.
- **How often:** at most every 5 minutes while the tab is open.
- **The card:** the plan (Free, Hobby, Pro, Pro+, Ultra, Team, Business, Enterprise and the rest), the account's email (masked), **Monthly** (with **Monthly · Auto** and **Monthly · API** where Cursor splits it) with when the billing month resets, or **Requests** used of how many on the older plans, and extra spend this month against its cap.
- **Typed key:** none.
- **Not read:** browser cookies, and the sign-in of Cursor's command line tool.

### GitHub Copilot

- **Reads:** the sign-in that GitHub's Copilot plugins for editors other than VS Code (JetBrains, Vim and Neovim, Xcode and others) keep in `apps.json`, or the older `hosts.json`, in Copilot's folder: `~\AppData\Local\github-copilot\` on Windows, `~/.config/github-copilot/` on a Mac (or `github-copilot` under `XDG_CONFIG_HOME` when that is set to a full path). It also checks whether an `auth.db` is in that folder, without opening it.
- **Asks:** `GET https://api.github.com/copilot_internal/user` with that sign-in (for a GitHub Enterprise Cloud sign-in, the same at `api.<name>.ghe.com`). One request a refresh. A GitHub Enterprise Server is never asked.
- **How often:** at most every 5 minutes while the tab is open.
- **The card:** the plan (Free, Pro, Pro+, Max, Student, Business, Enterprise), your GitHub login (masked), and for each limit your plan has (**Premium requests**, **Chat**, **Code completions**) how many you've used of how many, with the day it resets. Extra premium requests you pay for take the bar past 100%. A plan without limits shows the plan only.
- **Typed key:** none.
- **Not read:** VS Code's GitHub sign-in (kept in VS Code's own secret storage), the GitHub CLI's token or tokens in variables. Editors on Copilot's newer language server (from June 2026) keep new sign-ins in `auth.db`, which isn't read yet, so such a sign-in shows only while `apps.json` still holds the older one.

### Kimi Code

- **Reads:** the first one found: a Kimi Code API key you typed, or Kimi Code's own sign-in in `~/.kimi-code/credentials/` (`kimi-code.json` for a kimi.com account, `kimi-code-env-0e4f99c69cc27850.json` for a kimi.ai account; under `KIMI_CODE_HOME` if you set it), which its terminal app, VS Code extension and desktop app share. Whether `~/.kimi-code/device_id` is there tells that Kimi Code is installed, so the card can say you're signed out; what it says isn't used.
- **Asks:** the Kimi Code server the sign-in belongs to, `api.kimi.com` or `api.kimi.ai`: `GET /coding/v1/usages`, then `GET /coding/v1/me` for your plan's name and your email. Two requests a refresh (three when a typed key is refused by the first server). A sign-in goes only to its own server. A typed key goes to this computer's Kimi Code server (or api.kimi.ai if there is none) and, while it is refused there, to the other one too: both are Kimi's own, and api.kimi.com is its server in mainland China.
- **How often:** at most every 5 minutes while the tab is open.
- **The card:** your plan's name, the account's email (masked), **5-hour**, **Weekly** and **Monthly** with when each resets, and with Extra Usage on, its balance and this month's spend against your cap.
- **Typed key:** optional, made in the [Kimi Code console](https://www.kimi.ai/code/console). Kimi Code's own sign-in lasts about 15 minutes and is renewed only while Kimi Code runs, so without a key the card often asks you to open Kimi Code once.
- **Not read:** the older Python Kimi CLI (`~/.kimi/`) and browser cookies.

### Z.ai GLM

- **Reads:** nothing on disk: a key you typed, or the `Z_AI_API_KEY` variable.
- **Asks:** `GET https://api.z.ai/api/monitor/usage/quota/limit`, the address z.ai's own usage page reads, with the key. One request a refresh. A key that api.z.ai refuses (one made on bigmodel.cn, Zhipu's platform in mainland China for the same plan) is asked about once more, at `open.bigmodel.cn`, on each refresh while api.z.ai refuses it. The key goes to those two servers only.
- **How often:** at most every 5 minutes while the tab is open.
- **The card:** the plan, **5-hour** and **Weekly** (the token limits) and **Tool calls** (the month's), with when each resets.
- **Typed key:** needed, unless `Z_AI_API_KEY` is set. Make one on [Z.ai's API keys page](https://z.ai/manage-apikey/apikey-list).

### MiniMax

- **Reads:** nothing on disk: a key you typed, else the `MINIMAX_CODING_API_KEY` variable, else `MINIMAX_API_KEY`. Only a plan key can read the plan, so a standard API key (`sk-api-…`) is never sent: in a variable it is skipped, and typed in it shows as refused.
- **Asks:** `api.minimax.io`, with the key: `GET /v1/token_plan/remains`, and for an older Coding Plan `GET /v1/api/openplatform/coding_plan/remains`. A key that api.minimax.io refuses is asked about the same way at `api.minimaxi.com`, MiniMax's server in mainland China, on each refresh while it is refused. Usually one request a refresh, at most four. The requests name Oyenza Studio (`MM-API-Source: Oyenza Studio`).
- **How often:** at most every 5 minutes while the tab is open.
- **The card:** the plan (Token Plan, Plus and so on, when MiniMax names it), **5-hour** and **Weekly** for text, and a bar for each other allowance your plan has (**Daily · Video**, **Daily · Speech**, **Daily · Images**, **Daily · Music**), counted where the plan counts.
- **Typed key:** needed, unless one of the variables is set. Make one on [MiniMax's Coding Plan page](https://platform.minimax.io/user-center/payment/coding-plan).

### OpenRouter

- **Reads:** nothing on disk: a key you typed, or the `OPENROUTER_API_KEY` variable (the typed one wins).
- **Asks:** `openrouter.ai`, with the key: `GET /api/v1/key`, then `GET /api/v1/credits`. Two requests a refresh.
- **How often:** at most every 2 minutes while the tab is open.
- **The card:** your credits left, what the key spent today, this week and this month (OpenRouter's days and weeks, in UTC) against its limit if it has one, and on the free tier, the free model requests used today. OpenRouter shows the credits left only to keys allowed to read them, so with an ordinary key the card has the spend without the balance.
- **Typed key:** needed, unless `OPENROUTER_API_KEY` is set. Make one on [OpenRouter's keys page](https://openrouter.ai/settings/keys).

### Oyenza Inference

- **Reads:** the key this computer got when you connected it (below). It is never typed.
- **Asks:** `GET https://inference.oyenza.com/v1/studio/summary` with that key. One request a refresh. A test server can be named in `PONI_INFERENCE_URL` instead: an `https://` address anywhere, or plain `http://` only on this computer (`localhost`, `127.0.0.1`, `[::1]`); anything else is ignored, so the key never crosses the network unencrypted.
- **How often:** at most once a minute while the tab is open.
- **The card:** your plan (Trial or Pay as you go), the account's email (masked), your balance, what you spent today and over the last 7 and 30 days, and the last 14 days as a small bar line.
- **Typed key:** none: **Connect** instead.

**Connecting.** **Connect** on the card shows 8 letters, such as `KPJX-TMNQ`, and **Open** takes you to inference.oyenza.com's page for them. Signed in there, check the computer's name and the letters, and choose **Approve**: the card fills in a few seconds later. The code works once, for 10 minutes. Approve only the letters your own Oyenza Studio shows: whoever made a code you approve can read your balance and spend until you revoke it. Connecting tells inference.oyenza.com this computer's name, whether it's Windows or a Mac, and Oyenza Studio's version.

**What the key can do.** It reads your account's summary and nothing else: balance, plan, spend and number of requests today and over the last 7 and 30 days, each of the last 14 days, your top three models by spend, and the account's email, masked. It can't call the models, spend credit, or see your prompts, your API keys, your payments or your account's settings. inference.oyenza.com keeps only a fingerprint of it (SHA-256), never the key itself; each computer has its own, and an account has at most 10 (connecting an 11th disconnects the oldest). Nothing about your other AI services goes to inference.oyenza.com.

**Disconnecting.** **Disconnect** on the card revokes the key on inference.oyenza.com and deletes it here (if inference.oyenza.com can't be reached just then, revoke the computer in the dashboard too). You can also revoke the computer in your inference.oyenza.com dashboard, under Connected apps, and changing your password there disconnects every computer. A disconnected computer's card says so, with **Connect again**.

## Keys and variables

- **Typed keys** go in the AI tab: open **Services**, choose **Add key** beside the service, paste the key and **Save**. Oyenza Studio keeps each one sealed with Windows' own protection (DPAPI), so only your Windows account on this PC can read it, in `%LOCALAPPDATA%\Poni\`; on a Mac, in a file only your Mac account can read, in `~/Library/Application Support/Poni/`. Never in your settings or the log. Uninstalling on Windows deletes them.
- **A key with a space or line break inside it** (a paste that caught one) is refused before it is sent; spaces around it are trimmed.
- **Variables** such as `OPENROUTER_API_KEY` or `CODEX_HOME` are the ones Oyenza Studio started with, so restart it after setting one. On a Mac, apps opened from the Dock or Finder don't see variables set in a shell profile such as `~/.zshrc`, so type the key in the AI tab there instead.
- **On a Mac, a sign-in kept in the keychain** is read with macOS's own `security` tool, only while the AI tab is open, and at most once each time you open it (the notices use that copy). The item normally trusts that tool, so macOS doesn't ask. If it does, its prompt names "security": say no to keep the item closed, and the card says so until you press **Refresh**, which asks once more.
- **What you chose** (which services are hidden or added, the notices switch) is kept in your settings, `settings.json`, under `usage`.

## Removing it all

- **Hide** a service with its switch under **Services**, and it is no longer read.
- **Remove** a key you typed and its sealed copy is deleted. To make the key useless everywhere, delete it on the service's own site too.
- **Disconnect** Oyenza Inference, or revoke the computer in the dashboard.
- **Switch off Tell me before I run out** and nothing is read while the tab is closed.
- **Signing out** of a service in its own app works too: Oyenza Studio then has no sign-in to send.

## Where this comes from

The readers port logic from [CodexBar](https://github.com/steipete/CodexBar) by Peter Steinberger (MIT; its notice is in [THIRD-PARTY-NOTICES.txt](../THIRD-PARTY-NOTICES.txt)): where each tool keeps its sign-in, which of its service's servers to ask and how to read the answers. File names and requests were also checked against the services' own open-source tools (Codex, Gemini CLI, Kimi Code, OpenCode and VS Code's Copilot code), with no code taken from them. Each reader is one file in `crates/studio-usage/src/`, tested against hand-made answers in `crates/studio-usage/src/fixtures/`; none of them came from a real account. How the pieces fit: [ARCHITECTURE.md](../ARCHITECTURE.md#ai-usage).
