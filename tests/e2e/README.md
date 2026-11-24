# Headless browser checks

Not run in CI (they need the Crafting Apps installed and Chrome). They drive the real shell and apps
in headless Chrome with a software renderer.

```sh
npm install                 # puppeteer-core only; uses the system Chrome (CHROME=/path/to/chrome)
# server on 127.0.0.1:8090 with ARTCRAFT_DATA=data, admin / $ARTCRAFT_ADMIN_PASSWORD, apps installed,
# and hello.docx, Book1.xlsx, sample.jpg, sample.pdf, sample.dxf, Presentation1.deckcraft in admin's files
node ui-check.js            # drive, People on first open (regression), share dialog, new-file cards, docs
node features.js all        # in-app Open, picture picker, discard, GridCraft autosave, options, zips, new doc, dialogs
node features.js others     # in-app Open in DeckCraft and CADCraft; `pdf` for PdfCraft (Ctrl+O)
node settings.js            # Settings: light/dark, shell language (German), WordCraft opened in German under the
                            # Settings name with nothing stored (user jonas; E2E_USER / E2E_PASS to change)
```

Screenshots go to `shots/` (`OUT=` to change). A run passes when every line looks right and
`errors []` is printed: page errors, native `alert`/`confirm`/`prompt` dialogs and console errors
are collected there.
