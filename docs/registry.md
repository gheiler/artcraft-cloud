# The app registry

artcraftCloud learns which apps exist, and which of their versions may run, from a public signed
list: [`registry/apps.json`](../registry/apps.json) in this repository, with its signature in
`registry/apps.json.sig`. A running server fetches it every 6 hours (and when the super admin
presses **Check now** on Apps, or runs `artcraft-cloud apps registry refresh`), so a new app, a new
version or a blocked version reaches every server without a new Docker image.

## Trust model

| Piece | Where it lives | What it is trusted for |
|---|---|---|
| Registry keys (ed25519) | Public keys built into the binary (`crates/server/src/registry.rs`, `BUILTIN_KEYS`: a primary and a spare); the primary private key is the `ARTCRAFT_REGISTRY_SIGNING_KEY` secret of this repository | Which apps and versions exist, their checksums, their publishers' release keys, which versions are blocked |
| Registry file | `https://raw.githubusercontent.com/gheiler/artcraft-cloud/main/registry/apps.json` + `.sig` (`ARTCRAFT_REGISTRY_URL` for a mirror) | Nothing until its signature verifies |
| Publisher release keys | Pinned per app in the registry (`keys.openpgp` fingerprints, `keys.ed25519` public keys) | That a release really comes from the app's publisher |
| Checksums | Pinned per version in the registry (`sha256`, `size`) | That the downloaded archive is the one that was reviewed |

A fetched registry is used only when:

- `apps.json.sig` is a valid ed25519 signature over the exact bytes of `apps.json`, by one of the
  trusted keys;
- it has not expired (`expires`; the tool signs for 120 days, the workflow renews monthly) and was
  not issued more than a day in the future;
- its `serial` is not lower than the highest one this server ever accepted from that source (kept in
  `data/registry/state.json`), and a copy with the same serial is byte-identical. An attacker who
  can serve an older, validly signed registry (one that did not block a bad version yet) is refused.

The last good copy is kept in `data/registry/` and used across restarts and outages. Each binary
also embeds the registry it was built with, so a fresh server without network still knows the
apps; it is used only while no newer copy was ever fetched. While the registry in use is expired,
nothing new is installed (what is installed keeps running).

Installing a version (Apps page, auto-update or `apps update`):

1. The version must be listed with status `ok` and fit this artcraftCloud (`min_cloud`/`max_cloud`).
2. The archive is downloaded (https only; GitHub release assets by default) and must match the
   pinned SHA-256.
3. The publisher's signature over `SHA256SUMS.txt` is checked with the pinned key: OpenPGP
   (`SHA256SUMS.txt.asc`, the fingerprint must be pinned; a key shipped in the release only helps
   gpg find it) or ed25519 (`SHA256SUMS.txt.sig`). The signed checksum must equal the registry's.
   A version marked `"signed": "none"` relies on the registry checksum alone; the Apps page says so
   ("Checksum from the registry"). CalendarCraft 0.2.0 and 0.2.1 are signed with ed25519, like MailCraft.
4. The archive must carry the host bridge (for bridge apps) and safe paths; then it is unpacked,
   precompressed and switched in atomically. The previous version stays for rollback.

The server makes no GitHub API calls, so there are no rate limits and no token.

### Blocked versions

A version with `"status": "blocked"` is never installed. When it is already installed, the super
admin sees a warning on every page; with `"security": true` the server also acts on its own
(`ARTCRAFT_BLOCKED_APPS`, default `security`): it rolls the app back to its previous version if
that is on disk and allowed, or disables the app (it stops being served) when there is none. `all`
does the same for every block, `warn` never acts. `"status": "incompatible"` with `min_cloud` /
`max_cloud` marks versions that need a newer or older artcraftCloud.

## Format

```json
{
  "schema": 1,
  "serial": 12,
  "issued": 1795556161,
  "expires": 1805924161,
  "apps": [{
    "id": "mailcraft", "title": "MailCraft", "publisher": "UCS Care Group",
    "owner": "UCS-Care-Group", "repo": "mailcraft", "blurb": "Mail, calendar and contacts",
    "icon": {"url": "https://raw.githubusercontent.com/gheiler/artcraft-cloud/main/registry/icons/mailcraft.png", "sha256": "…"},
    "extensions": [], "author_query": false, "chrome_query": false, "bridge": false,
    "default_install": true,
    "keys": {"ed25519": ["Mu5RF5oJ…", "XV6WRgW0…"]},
    "recommended": "0.15.0",
    "versions": [{
      "version": "0.15.0", "tag": "v0.15.0", "asset": "mailcraft-web-0.15.0.zip",
      "sha256": "9c9cf994…", "size": 14579305, "signed": "ed25519", "status": "ok"
    }]
  }]
}
```

- `extensions`: file types the app opens (lower case, unique across apps). `bridge`: the web build
  speaks the host contract (opens and saves drive files); MailCraft does not.
- `default_install`: installed on a new server. Other apps appear under **Available** on Apps.
- `versions[].url`: optional; by default
  `https://github.com/<owner>/<repo>/releases/download/<tag>/<asset>`. `SHA256SUMS.txt` and its
  signature are fetched from the same directory.
- `versions[].status`: `ok`, `blocked` (with `reason`, `security`) or `incompatible` (with
  `min_cloud` / `max_cloud`, inclusive).
- `recommended`: what Add and Update install; when unset or not allowed, the newest allowed version.

## Maintaining it

Every command below edits `registry/apps.json`, bumps `serial`, renews `issued`/`expires` and
validates the result. It signs too when the key is at hand (`--key-file` or
`ARTCRAFT_REGISTRY_SIGNING_KEY`); otherwise the registry workflow signs after the change is merged.

### Add a version

```sh
artcraft-cloud registry add-version wordcraft 0.19.0 --recommend
```

It downloads the release archive, records its SHA-256 and size, checks `SHA256SUMS.txt` lists the
same hash and that the publisher's signature verifies with the key the registry already pins. It
refuses an unsigned release of an app that has pinned keys, and a release signed by a key that is
not pinned. Open a pull request with the change; CI checks the file and that the serial grew.

### Add an app

```sh
artcraft-cloud registry add-app notecraft --title NoteCraft --publisher "Example" \
  --owner example --repo notecraft --blurb "Notes" --ext note,md --bridge \
  --icon-url https://raw.githubusercontent.com/gheiler/artcraft-cloud/main/registry/icons/notecraft.png \
  --icon-file registry/icons/notecraft.png
artcraft-cloud registry add-version notecraft 1.0.0 --trust-key --recommend
```

`--ed25519-key <base64>` (repeatable; first is the key in use) pins an ed25519 release key; for
OpenPGP, the first `add-version` with `--trust-key` pins the fingerprint of the key that signed
that release. Compare the fingerprint it prints with one the publisher states elsewhere (their
website, another repository, a signed tag) before you merge. To add or replace keys on an app
that is already listed:

```sh
artcraft-cloud registry set-keys calendarcraft \
  --ed25519-key 'Hv8y6mNjnmRqTlAcnJl1v/jCiRyCVsHUi9/fT1T629E=' \
  --ed25519-key 'jF5sjrTxNhzH0nffbxO2SXoeITH3UsofUPTEu9IKPEk='
```

Then `registry add-version <app> <version> --replace` if a listed version should now be checked
against those keys. Without `--default-install` the app is offered on Apps but not installed on
its own. Put a 256×256 PNG icon in `registry/icons/`.

### Block a version

```sh
artcraft-cloud registry block wordcraft 0.18.0 --reason "loses comments on save"
artcraft-cloud registry block wordcraft 0.18.0 --reason "CVE-2026-1234" --security
artcraft-cloud registry set-status filmcraft 0.21.0 incompatible --min-cloud 0.2.0
artcraft-cloud registry set-status wordcraft 0.18.0 ok             # unblock
```

Merge it to `main`; servers pick it up within 6 hours (or at once with **Check now**).

### Sign, renew, verify

```sh
artcraft-cloud registry sign --key-file /path/registry-primary.key
artcraft-cloud registry renew                      # new serial and expiry, same content
artcraft-cloud registry verify [--key <pub>] [--against old-apps.json]
artcraft-cloud registry keygen --out-dir ~/registry-keys --name registry-primary
```

### The workflow

[`.github/workflows/registry.yml`](../.github/workflows/registry.yml) signs and renews the registry.
`main` is protected (a pull request with one approving review, squash only; GitHub Actions is not
a bypass actor), so the workflow never pushes to `main`:

- On a pull request that touches `registry/`, it signs the branch so the signature is reviewed
  and merged with the change.
- On the 1st of every month, or by hand, it renews the expiry (a new serial, still well within
  the 120 days) and opens a pull request.
- After a merge to `main` it only verifies. If the copy is unsigned, the job fails and says to
  sign in the pull request, or to add the GitHub Actions app as a bypass actor on the main
  ruleset (Settings › Rules).

It needs the repository secret **`ARTCRAFT_REGISTRY_SIGNING_KEY`** (Settings › Secrets and
variables › Actions › New repository secret): the base64 seed in `registry-primary.key`, one line.
You can also sign locally with `artcraft-cloud registry sign --key-file …` before opening the
pull request.

Release builds (tags) must embed a signed registry: CI fails a tag whose `apps.json.sig` does not
match.

### Rotating keys

The binary trusts two keys: the primary (in the secret) and a spare (kept offline). To rotate:

1. Sign with the spare: put `registry-spare.key` in the secret, run the workflow.
2. Generate a new spare (`registry keygen`), and release an artcraftCloud with
   `BUILTIN_KEYS = [old spare (now primary), new spare]`.
3. Once servers run that release, the old primary is no longer trusted anywhere.

If the primary key leaks, do the same at once and block nothing with it: servers that are not
updated yet still accept the leaked key, so the anti-rollback serial matters. Sign the next
registry with a serial far above the current one (for example `+1000`), which raises the floor on
every server that fetches it.

## A mirror or your own registry

Point `ARTCRAFT_REGISTRY_URL` at a copy of `apps.json` (with `apps.json.sig` beside it), for an
air-gapped network or a curated list. To sign it yourself, set `ARTCRAFT_REGISTRY_KEYS` to your
public key(s), comma-separated; they replace the built-in keys (the embedded snapshot is then
ignored, since it is signed with the built-in key). Changing the URL or keys starts the serial
floor over for that source.
