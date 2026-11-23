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
| 4 | **Host file picker for File › Open and Recent.** In host mode the app's own picker is off (`can_pick()` false) but nothing asks the host to pick instead, and Recent lists browser-local files. | Users open files from the drive, not from inside the app. CADCraft's start page still shows its own *Open…* and *Recent*. | No issue found: **needs a ticket** (related: places panel [craft-libs#49](https://github.com/storytold/craft-libs/issues/49)) |
| 5 | **User name for every app.** Only WordCraft and PdfCraft read `?author=`. | Comments and tracked changes in the other apps are not attributed to the signed-in user. | No issue found: **needs a ticket** (a `host` message field would be cleaner than a query string) |
| 6 | **WebDAV-backed cloud open/save.** craft-libs has `craft-cloud` (Files traits, "files are WebDAV"), `craft-dav` and the craft-ui `cloud` feature (Backstage Open/Save As against a CloudBackend), but no web build enables them. | artcraftCloud already serves WebDAV at `/dav/` (Basic auth, per-user home), ready for the apps to use once wired. | No issue found for enabling it in web builds: **needs a ticket** |
| 7 | **Bridge in PhotoCraft, VectorCraft, DesignCraft** (and EffectCraft, SoundCraft, TextureCraft, MaterialCraft, AniCraft). | Not shipped in artcraftCloud. | PhotoCraft [#13515](https://github.com/storytold/photocraft/pull/13515), [#10582](https://github.com/storytold/photocraft/pull/10582), issues [#9593](https://github.com/storytold/photocraft/issues/9593), [#3101](https://github.com/storytold/photocraft/issues/3101); VectorCraft [#3503](https://github.com/storytold/vectorcraft/pull/3503), [#1094](https://github.com/storytold/vectorcraft/issues/1094); DesignCraft [#1538](https://github.com/storytold/designcraft/pull/1538), [#1283](https://github.com/storytold/designcraft/issues/1283), [#2107](https://github.com/storytold/designcraft/issues/2107); EffectCraft [#2071](https://github.com/storytold/effectcraft/issues/2071), [#698](https://github.com/storytold/effectcraft/issues/698); SoundCraft [#866](https://github.com/storytold/soundcraft/issues/866), [#771](https://github.com/storytold/soundcraft/issues/771) |
| 8 | **Live collaboration** outside FigCraft. | Out of scope for the MVP; the shell relies on ETags instead. | Collaboration session messages [craft-libs#716](https://github.com/storytold/craft-libs/issues/716), host statuses [#2079](https://github.com/storytold/craft-libs/issues/2079) / [#2108](https://github.com/storytold/craft-libs/issues/2108), GridCraft roadmap [gridcraft#16](https://github.com/storytold/gridcraft/issues/16) |
| 9 | **LightCraft `ready` message** sends `version: "0.21.1"` (its app version) where the contract puts the contract version `"1"`, and omits `appVersion` and `file`. | Harmless today (the shell does not gate on the version), but a host that checks the contract version would reject LightCraft. | No issue found: **needs a ticket** |
| 10 | **LightCraft in host mode** opens the handed photo inside its demo library and reports unsaved changes right after opening. | The photo opens; the sample library is visible next to it and the shell shows the unsaved dot straight away. | No issue found: **needs a ticket** |
| 11 | **Locale from the host.** | Apps pick their language from the browser, not from the user's artcraftCloud profile. | [craft-libs#1194](https://github.com/storytold/craft-libs/issues/1194) |
| 12 | **Self-hosted update channel.** | artcraftCloud polls GitHub releases and checks `SHA256SUMS.txt` + `SHA256SUMS.txt.asc` itself. A stable manifest (latest version, zip name, hash, signing key fingerprint) would make this simpler and avoid the GitHub API rate limit. | [craft-libs#68](https://github.com/storytold/craft-libs/issues/68) |

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

These ran in headless Chrome with a software renderer (SwiftShader), driven by puppeteer against
the release binary.
