# Oyen Poni brand files

Oyen Poni belongs to the Oyen family. Oyen is the orange cat of the Oyenza logo.

| File | What |
|---|---|
| `oyen-mark.svg`, `oyen-mark-white.svg` | Oyen's head in colour and in white, copied unchanged from the Oyenza brand kit |
| `oyen-poni-logo.svg`, `oyen-poni-logo-512.png` | The logo: an orange tile, the island hanging from its top edge playing music (album cover and equalizer, as on the real island), the white Oyen below |
| `art/oyen-poni-sit.webp` | Oyen sitting on the island while it plays music (the installer's welcome and finish picture) |
| `art/oyen-poni-peek.webp` | Oyen peeking over the island while it plays music (README, link preview) |
| `art/oyen-poni-asleep.webp` | Oyen asleep on the island while it plays music (the website's Oyen theme section) |
| `art/poses/oyen-*.webp` | Oyen with each page's prop: music, focus, health, shelf, tasks, clipboard, today, system (a processor chip), games (a controller), prayer (a lantern) (README pictures and website) |
| `readme-hero.webp` | The README banner: a 200 % screenshot of the website's hero (`site/index.html`), so the README shows the site's type |
| `social-preview.png` | The card GitHub shows when the repo link is shared (Settings → General → Social preview) |

`python3 tools/make_brand.py` redraws the logo, `crates/poni/res/poni.ico` and everything in `installer/art` from these files.

Drawing Oyen: the head is exactly the shape of the mark, the left ear (viewer's left) is folded forward with a darker underside, and the face has only the two eyes: no mouth, nose, whiskers or blush. Don't redraw Oyen in another style.

These files are not covered by the MIT license that covers the code.
