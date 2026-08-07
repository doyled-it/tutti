# Tutti brand

The single source of truth for Tutti's visual identity. Everything downstream (app
icons, favicon, the in-app mark component) is generated or derived from the SVGs here.
Do not hand-edit the raster icons in `tutti-app/src-tauri/icons/`; regenerate them.

## The mark — "The Upbeat"

Tutti is the score direction for *all instruments, together*. There is no glyph for it
in notation (it is a written word), so the mark borrows the gesture that *causes* tutti:
a conductor's **2/4 beat pattern**. The two amber dots are the beats, numbered **1** (the
ictus, where the downbeat strikes) and **2** (the lift into the upbeat). The stroke
resolves upward as the ensemble comes in.

It is a deliberate sibling to Sotto's mark: Sotto's three amber dots are the typing/beat
dots of a hushed single voice; Tutti's two are the beats of the whole ensemble. Same
family, one instrument fewer.

- `tutti-mark.svg` — the full mark with numbered beats (large / hero / documentation).
- `tutti-icon.svg` — the mark without numbers (favicon, in-app, small sizes). Numbers
  drop below ~28px along with fine detail; the U-plus-two-dots silhouette carries.
- `tutti-appicon.svg` — the mark on a warm near-black rounded tile; the 1024px source
  for the platform icon set and the favicon.

In the **wordmark**, the mark doubles as the lowercase **u** in *Tutti* (see
`tutti-app/src/lib/components/TuttiWordmark.svelte`).

## Palette — "Warm Ensemble" (sibling to Sotto, not a twin)

Shared DNA with Sotto: the dark-only world, cream text, amber, coral, Fraunces display.
The twin-breakers are the warmth of the neutrals and keeping amber as the single lead.

| Token        | Hex        | Role                                   |
|--------------|------------|----------------------------------------|
| Canvas       | `#0E0A07`  | warm near-black background             |
| Panel        | `#17110B`  | raised surfaces                        |
| Amber (lead) | `#FFB454`  | primary accent, the beat dots          |
| Teal         | `#5FD3C4`  | secondary / done / connected           |
| Coral        | `#FF8C6B`  | sensitive / error                      |
| Cream        | `#EFE6D2`  | text, the mark stroke                  |

The app tokens live in `tutti-app/src/app.html`.

## Typography

- **Fraunces** (self-hosted, `tutti-app/static/fonts/`) — the display / wordmark face,
  shared with Sotto to reinforce the sibling relationship. Italic is used for the beat
  numbers and the wordmark.
- UI/body text stays on the system sans, which keeps Tutti distinct from Sotto's Hanken.

## Regenerating icons

```bash
brand/build-icons.sh   # needs rsvg-convert + cargo-tauri
```

Renders `tutti-appicon.svg` to 1024px, runs `cargo tauri icon` to produce the whole
`src-tauri/icons/` set, and writes `tutti-app/static/favicon.png`.
