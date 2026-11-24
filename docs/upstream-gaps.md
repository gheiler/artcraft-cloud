# What ArtCraft would need for full integration

artcraftCloud hosts the Crafting Apps through the `craft-web-host` contract (craft-libs
`crates/web-host`, contract version `1`): the shell loads an app in a same-origin iframe with
`?host=parent`, waits for `<app>:ready`, posts `<app>:open` with the file's bytes, and stores every
`<app>:save` it gets back. That works today for the eight apps listed in the README. The gaps below
are what keeps it from feeling like one product. Nothing has been posted upstream; links point at
issues and pull requests that already exist (found November 2026), and the rest still need a ticket.

| # | Gap | Effect in artcraftCloud today | Upstream |
|---|-----|-------------------------------|----------|
| 1 | **Save acknowledgement.** Every app calls `post_save(..., id = None)`, so the host cannot answer with `<app>:saved` or report a failed write. | The shell keeps its own "Saved." / conflict status in its bar. When a save hits an ETag conflict the app still believes it saved; the shell offers **Save a copy**. | [craft-libs#1338](https://github.com/storytold/craft-libs/issues/1338) (confirmed saves), [#713](https://github.com/storytold/craft-libs/issues/713) (saves in place), [#611](https://github.com/storytold/craft-libs/issues/611) (host asks for save / autosave) |
| 2 | **Host-controlled File tab** (`?chrome=nofiletab`) only exists in GridCraft. | The other apps show their own File menu, whose Open/Recent entries do not know about the cloud. | [craft-libs#610](https://github.com/storytold/craft-libs/issues/610) |
| 3 | **Read-only sessions.** No message tells an app the document is view-only. | View-only shares open editable; the shell refuses to store the save and says so in its bar. | [craft-libs#609](https://github.com/storytold/craft-libs/issues/609) |
| 4 | **Host file picker for File › Open, Insert › Picture and Recent.** In host mode nothing asks the host to pick: the apps click a hidden `<input type=file>` (WordCraft, GridCraft, DeckCraft, CADCraft, FilmCraft, PdfCraft, LightCraft's restore). Recent entries are paths, and opening one fails with *"…: files are opened through the browser on the web"* (WordCraft `Hola.docx`). | The shell wraps `HTMLInputElement.prototype.click`/`showPicker` and `showOpenFilePicker` inside the frame and shows its own drive picker: a document of the same app opens through the drive (new URL, save goes back to the drive), any other file (a picture, a backup) is handed to the app's input as bytes. Recent lists are emptied when options are restored. This is a workaround that depends on how the apps pick files; a real message is drafted in [the ticket below](#ticket-draft-host-file-picker). | No issue found: **needs a ticket** (related: places panel [craft-libs#49](https://github.com/storytold/craft-libs/issues/49)) |
| 5 | **User name for every app.** Only WordCraft and PdfCraft read `?author=`. | Comments and tracked changes in the other apps are not attributed to the signed-in user. | No issue found: **needs a ticket** (a `host` message field would be cleaner than a query string) |
| 6 | **WebDAV-backed cloud open/save.** craft-libs has `craft-cloud` (Files traits, "files are WebDAV"), `craft-dav` and the craft-ui `cloud` feature (Backstage Open/Save As against a CloudBackend), but no web build enables them. | artcraftCloud already serves WebDAV at `/dav/` (Basic auth, per-user home), ready for the apps to use once wired. | No issue found for enabling it in web builds: **needs a ticket** |
| 7 | **Bridge in PhotoCraft, VectorCraft, DesignCraft** (and EffectCraft, SoundCraft, TextureCraft, MaterialCraft, AniCraft). | Not shipped in artcraftCloud. | PhotoCraft [#13515](https://github.com/storytold/photocraft/pull/13515), [#10582](https://github.com/storytold/photocraft/pull/10582), issues [#9593](https://github.com/storytold/photocraft/issues/9593), [#3101](https://github.com/storytold/photocraft/issues/3101); VectorCraft [#3503](https://github.com/storytold/vectorcraft/pull/3503), [#1094](https://github.com/storytold/vectorcraft/issues/1094); DesignCraft [#1538](https://github.com/storytold/designcraft/pull/1538), [#1283](https://github.com/storytold/designcraft/issues/1283), [#2107](https://github.com/storytold/designcraft/issues/2107); EffectCraft [#2071](https://github.com/storytold/effectcraft/issues/2071), [#698](https://github.com/storytold/effectcraft/issues/698); SoundCraft [#866](https://github.com/storytold/soundcraft/issues/866), [#771](https://github.com/storytold/soundcraft/issues/771) |
| 8 | **Live collaboration** outside FigCraft. | Out of scope for the MVP; the shell relies on ETags instead. | Collaboration session messages [craft-libs#716](https://github.com/storytold/craft-libs/issues/716), host statuses [#2079](https://github.com/storytold/craft-libs/issues/2079) / [#2108](https://github.com/storytold/craft-libs/issues/2108), GridCraft roadmap [gridcraft#16](https://github.com/storytold/gridcraft/issues/16) |
| 9 | **LightCraft `ready` message** sends `version: "0.21.1"` (its app version) where the contract puts the contract version `"1"`, and omits `appVersion` and `file`. | Harmless today (the shell does not gate on the version), but a host that checks the contract version would reject LightCraft. | No issue found: **needs a ticket** |
| 10 | **LightCraft in host mode** opens the handed photo inside its demo library and reports unsaved changes right after opening. | The photo opens; the sample library is visible next to it and the shell shows the unsaved dot straight away. | No issue found: **needs a ticket** |
| 11 | **Locale, theme and name from the host.** No `host` message carries the user's language, theme or name. | The shell writes them into each app's stored options (see the README's *Settings*), which depends on each app's private `localStorage` layout and only takes effect at the next start. | [craft-libs#1194](https://github.com/storytold/craft-libs/issues/1194) |
| 12 | **Self-hosted update channel.** | artcraftCloud polls GitHub releases and checks `SHA256SUMS.txt` + `SHA256SUMS.txt.asc` itself. A stable manifest (latest version, zip name, hash, signing key fingerprint) would make this simpler and avoid the GitHub API rate limit. | [craft-libs#68](https://github.com/storytold/craft-libs/issues/68) |
| 13 | **GridCraft AutoSave in host mode.** Its AutoSave switch runs `session.run("file.save")`, which writes to the document's desktop path; a hosted document has none, so nothing reaches the host and the switch looks on but does nothing. | The shell has its own **Autosave** switch (per user, on by default): 2.5 s after the last change, at most every 8 s, it asks the app to save with `window.<app>.execute('file.save', {})`, which sends a normal `<app>:save`. Tested with GridCraft: the .xlsx on disk changes without Ctrl+S. | No issue found: **needs a ticket** (fits [craft-libs#611](https://github.com/storytold/craft-libs/issues/611), host asks for save / autosave) |
| 14 | **Options storage hook.** Apps keep options (author name, initials, view settings) in `localStorage` under their own keys (`wordcraft/…`, `wordcraft.ui`), so they are per browser, not per user. | The shell copies each app's keys to `/api/settings/<app>` (SQLite, per user) when they change and writes them back before the app loads, so options follow the user across browsers. It cannot see options an app keeps in IndexedDB. A `<app>:settings` message (or a SettingsStore backed by the host) would remove the guesswork. | No issue found: **needs a ticket** |
| 15 | **LightCraft library backups.** *Restore Library from Backup…* asks with a native `confirm()` and has no page API, so a host cannot start a restore with a given backup. | The drive recognises a LightCraft backup .zip by its contents (`library/catalog.*`, or `library/` + `originals/`), opens LightCraft with a note pointing at *File ▸ Restore Library from Backup…*, and its file picker offers that backup first. The `confirm()` remains LightCraft's own. | No issue found: **needs a ticket** |
| 16 | **Options outside `localStorage`.** LightCraft keeps its options in IndexedDB/OPFS (language from `navigator.language`), FilmCraft in OPFS. | Settings cannot reach them: LightCraft follows the browser language, FilmCraft keeps its own theme. | No issue found: **needs a ticket** (same fix as gap 14) |
| 17 | **English only: DeckCraft, FigCraft, FilmCraft.** | The language setting does not change them. | No issue found: **needs a ticket** (craft-i18n is there to use) |
| 18 | **DeckCraft stores no options.** Name only via `?author=`; no theme, units or animations. | DeckCraft gets the Settings name at launch; nothing else. | No issue found: **needs a ticket** |
| 19 | **No "unset" in some options.** GridCraft writes `language: "en"` at first start, so the shell cannot tell it from a choice; PdfCraft's *initials* are a signature image; CADCraft's units are a profile sysvar (`MEASUREINIT`) for new drawings only, in a versioned profile the shell only edits once CADCraft has written it. | First-start fill does not change GridCraft's language once it has run (a Settings change still does); initials never reach PdfCraft; CADCraft units apply to new drawings after its first start. | No issue found: **needs a ticket** |
| 20 | **Settings with different shapes per app.** Interface size (GridCraft `interfaceScale`, others none or different), GridCraft animations on/off only (no "system"), units sets differ (WordCraft has picas, CADCraft only imperial/metric). | Interface size is not offered; the closest value is written. | No issue found: **needs a ticket** (a shared craft-ui settings schema) |

## Checked against the released web builds (box, 23 Nov 2026)

| App | Release | `ready` handshake | Open from the drive | Save back |
|-----|---------|-------------------|---------------------|-----------|
| WordCraft | 0.18.1 | yes | .docx | yes (edit + Ctrl+S, file changed on disk; ETag conflict → *Save a copy*) |
| GridCraft | 0.19.1 | yes | .xlsx | yes (new workbook → Save As → reopened with the typed cell) |
| DeckCraft | 0.18.1 | yes | .deckcraft | yes (new deck → Save As) |
| PdfCraft | 0.22.0 | yes | .pdf | yes (Ctrl+S) |
| CADCraft | 0.20.1 | yes | .dxf | yes (Ctrl+S) |
| FilmCraft | 0.20.1 | yes | (new project) | yes (new project → Save As .fcproj) |
| FigCraft | 0.18.1 | yes | not tried (no sample file) | not tried: starts on its home screen |
| LightCraft | 0.21.1 | yes (see gap 9) | .jpg | not tried (LightCraft exports rather than saves) |

In-app pickers (box, 24 Nov 2026), through the shell's picker: WordCraft *File ▸ Open* and *Insert ▸
Picture*, GridCraft and FilmCraft `file.open`, DeckCraft `app.openDialog`, CADCraft `ui.open`, PdfCraft
Ctrl+O. All show the drive picker; the WordCraft document opened at its own URL and the picture was
inserted. `tests/e2e/` has the scripts.

These ran in headless Chrome with a software renderer (SwiftShader), driven by puppeteer against
the release binary.

## Ticket draft: host file picker

Not posted. For `storytold/craft-libs` (web-host crate), title: **web-host: let the host pick files
(Open, Recent, Insert picture, imports)**.

> **Problem.** In host mode (`?host=parent`) the document comes from the host and saves go back to it,
> but every *other* file an app asks for still goes through a hidden `<input type=file>`: File ▸ Open,
> Insert ▸ Picture, imports, LightCraft's Restore from Backup. Those files come from the user's disk,
> not the host's storage, and an opened document is saved back to nowhere. Recent entries are paths
> and fail with "files are opened through the browser on the web".
>
> **Proposal (contract version 2, backwards compatible).** Before falling back to the file input, an app
> in host mode whose host advertised `capabilities: ["pick"]` in `<app>:host` posts
>
> ```json
> { "type": "<app>:pick", "id": "p1", "purpose": "open" | "insert" | "import",
>   "accept": [".docx", "image/*"], "multiple": false }
> ```
>
> and the host answers with one of
>
> ```json
> { "type": "<app>:picked", "id": "p1", "files": [{ "name": "a.png", "mime": "image/png", "bytes": <ArrayBuffer> }] }
> { "type": "<app>:picked", "id": "p1", "open": true }   // host opened the document itself (a fresh <app>:open follows)
> { "type": "<app>:picked", "id": "p1", "cancelled": true }
> ```
>
> `purpose: "open"` lets the host route a document through its own storage (new URL, saves back to the
> right place); the other purposes just need bytes. Recent in host mode would either be hidden or filled
> by the host (`<app>:recent` with names and opaque ids that come back in `<app>:pick` answers).
>
> **Why.** Today a host has to monkey-patch `HTMLInputElement.prototype.click` inside the frame to
> offer its own files, which breaks as soon as an app picks files another way.
