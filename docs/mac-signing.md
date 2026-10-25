# Signing the Mac app

Without a Developer ID signature, macOS says "Apple could not verify "Oyenza Studio" is free of malware" and sends people to System Settings > Privacy & Security > Open Anyway. A DMG signed with a Developer ID, checked by Apple (notarized) and stapled opens with one ordinary question, "downloaded from the Internet... Open".

The `mac` workflow does this by itself on every release tag once five secrets exist in this repository's `release` environment. Until then nothing changes: releases keep the ad-hoc signed test DMG. Only the Apple Developer account's Account Holder can make the certificate. Never paste any of these values into a chat, an issue or a commit.

What the workflow does, in `.github/workflows/mac.yml` and `tools/sign_mac_dmg.sh`:

- **`sign` (release tags, or by hand with `sign` on):** re-signs the app with the hardened runtime, a secure timestamp and `installer/mac/poni.entitlements`, builds and signs the DMG, sends it to Apple's notary service, staples the ticket, checks it the way Gatekeeper will, and runs the smoke test on the signed app. The release job then uploads that DMG under all its names, the old `Oyen-Poni-Mac.dmg` included. If signing fails, the release still gets the ad-hoc DMG, with a warning in the run.
- **`sign-check` (everything else, pull requests included):** the same signing with a throwaway certificate and no notarization, plus the smoke run under the hardened runtime, so the path is tested before a release needs it.

## 1. The Developer ID Application certificate (about 10 minutes, on your Mac)

**A signing request**

1. Open **Keychain Access** (Applications > Utilities).
2. In the menu bar: **Keychain Access > Certificate Assistant > Request a Certificate From a Certificate Authority…**
3. User Email Address: your Apple Account's email. Common Name: `Oyenza Studio Developer ID`. Leave CA Email empty. Choose **Saved to disk**, click **Continue** and save `CertificateSigningRequest.certSigningRequest`. Keychain Access makes the private key now, on this Mac.

**The certificate**

4. Open https://developer.apple.com/account/resources/certificates/add and sign in.
5. Under **Software**, choose **Developer ID Application**, then **Continue**.
6. Profile Type: **G2 Sub-CA**. Upload the `.certSigningRequest` file and click **Continue**.
7. Click **Download**, then double-click `developerID_application.cer`. Keychain Access adds it to the **login** keychain, next to its private key.

**Export it with its key**

8. In Keychain Access, open **login > My Certificates** and find **Developer ID Application: <name> (<team ID>)**. The triangle beside it shows the private key. No key there means the request was made on another Mac: export from that one.
9. Right-click the certificate > **Export "Developer ID Application: …"**. File Format: **Personal Information Exchange (.p12)**. Save it to the Desktop as `oyenza-studio-developer-id.p12`.
10. Give it a strong password from your password manager; it becomes the secret `MACOS_CERTIFICATE_PASSWORD`. If macOS asks for your login password to allow the export, allow it.

Xcode can do steps 1 to 7 in one go: **Xcode > Settings > Accounts**, select your team, **Manage Certificates… > + > Developer ID Application**, then continue at step 8.

Developer ID certificates made since 1 October 2026 last one year. Put a reminder two weeks before it ends, make a new one the same way, and replace the two certificate secrets. Apps signed and notarized before keep opening after a certificate expires.

## 2. The App Store Connect API key (about 5 minutes, in a browser)

notarytool signs in to Apple with this key, so no password or two-factor code sits in CI.

1. Open https://appstoreconnect.apple.com > **Users and Access > Integrations > App Store Connect API**, tab **Team Keys**. A Team key is needed: Apple's notary service doesn't take Individual keys.
2. Click **+** (Generate API Key). Name: `Oyenza Studio notarization`. Access: **Developer**. Click **Generate**.
3. Click **Download** next to the new key. You get `AuthKey_XXXXXXXXXX.p8`, and Apple offers the download only once.
4. Note the **Key ID** (in the list) and the **Issuer ID** (above the list).

Make a new key for this rather than reusing one that can manage the iPhone app: this one can only do what the Developer role allows.

## 3. The secrets, in the `release` environment (about 5 minutes)

1. Open https://github.com/lucacadalora/oyenza-studio (this repository; GitHub forwards the old lucacadalora/poni address) > **Settings > Environments > New environment**. Name it `release` and click **Configure environment**.
2. **Deployment branches and tags:** choose **Selected branches and tags**. Add a tag rule `v*`, and a branch rule `main` for the test in step 4. Other branches and pull requests then can't reach these secrets.
3. **Required reviewers** (optional): add yourself if every signed release should wait for your click on the run. Releases often come several times a day, so leaving it off is reasonable: only you, or a session you told to tag, push tags.
4. Under **Environment secrets**, click **Add secret** five times:

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE_P12_BASE64` | In Terminal: `base64 -i ~/Desktop/oyenza-studio-developer-id.p12 \| pbcopy`, then paste |
| `MACOS_CERTIFICATE_PASSWORD` | The password from step 1.10 |
| `APP_STORE_CONNECT_API_KEY_P8` | In Terminal: `pbcopy < ~/Downloads/AuthKey_XXXXXXXXXX.p8`, then paste (the whole file, BEGIN and END lines included) |
| `APP_STORE_CONNECT_KEY_ID` | The Key ID |
| `APP_STORE_CONNECT_ISSUER_ID` | The Issuer ID |

With the GitHub CLI on your Mac, once the environment exists (each command without `<` asks for the value):

```bash
base64 -i ~/Desktop/oyenza-studio-developer-id.p12 | gh secret set MACOS_CERTIFICATE_P12_BASE64 --env release -R lucacadalora/oyenza-studio
gh secret set MACOS_CERTIFICATE_PASSWORD --env release -R lucacadalora/oyenza-studio
gh secret set APP_STORE_CONNECT_API_KEY_P8 --env release -R lucacadalora/oyenza-studio < ~/Downloads/AuthKey_XXXXXXXXXX.p8
gh secret set APP_STORE_CONNECT_KEY_ID --env release -R lucacadalora/oyenza-studio
gh secret set APP_STORE_CONNECT_ISSUER_ID --env release -R lucacadalora/oyenza-studio
```

5. Then delete `oyenza-studio-developer-id.p12` and the `.p8` from the Desktop and Downloads, and empty the Trash. Keep copies in your password manager only if you want to reuse them for another app.

These are the same five names Oyenza Bar uses, and one certificate can sign both apps, but each repository needs its own copy of the secrets.

## 4. Try it before a release (about 10 minutes)

On main, run the workflow by hand with signing on: **Actions > mac > Run workflow**, tick **sign**, and run it. Or:

```bash
gh workflow run mac.yml -R lucacadalora/oyenza-studio --ref main -f sign=true
```

The **Sign, notarize and staple the DMG** job usually takes a few minutes. Apple sometimes holds a new team's first uploads for much longer; the job waits up to 45 minutes. Its artifact `oyenza-studio-mac-dmg-signed` holds the DMG. Download it on your MacBook, open the DMG, drag the app to Applications and open it. It should ask only "downloaded from the Internet", with no trip to Privacy & Security.

To check a DMG in Terminal:

```bash
xcrun stapler validate Oyenza-Studio-*-mac.dmg
spctl --assess --type open --context context:primary-signature -vvv Oyenza-Studio-*-mac.dmg
```

The second command should say `accepted` and `source=Notarized Developer ID`.

## What changes for people

- **First open:** one "downloaded from the Internet" question, then the app opens.
- **Permissions:** Music, Spotify and the other permissions are asked once more, on the first signed version. macOS ties them to the app's signature, and the ad-hoc test builds already lost them at every update. From then on, updates keep them.
- **First signed release:** that release also updates the README's Mac install note and the website's Mac section, since "Open Anyway" is no longer needed (AGENTS.md: every release updates the website).

## When the Apple team becomes the company's

Once Apple has moved the account to JATEVO LABS LIMITED, make a new certificate under the company's team (section 1) and replace the two certificate secrets. If Apple keeps the team ID, people keep their permissions. If it doesn't, they grant them once more.

## When something goes wrong

- **Apple refuses the DMG:** the sign job's log prints Apple's notarization log, which names the file and the problem. Fix it, then re-run the failed jobs in that run.
- **No valid identity:** the `.p12` holds no Developer ID Application certificate with its private key. Export again from step 1.8.
- **The certificate leaks:** revoking it at developer.apple.com stops every copy signed with it from opening. Make a new certificate and ship a release with it first, then revoke the old one.
