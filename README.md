# artcraftCloud

A self-hosted office suite made only of the [ArtCraft Crafting Apps](https://getartcraft.com/apps)
(Rust compiled to WebAssembly, by [storytold](https://github.com/storytold)). Every user keeps their
files in their own folder on your server; WordCraft, GridCraft, DeckCraft and the other apps open and
save straight into it.

- **One Rust binary** (axum + SQLite), files stored as plain folders on disk, about **8–12 MB of RAM** idle.
- **A drive in Rust/wasm** (Leptos, no JavaScript framework): folders, upload, download, rename,
  move, copy, trash, sharing, and an app launcher.
- **Multi-user**: argon2id passwords, cookie sessions, an admin who creates and disables accounts.
- **Sharing**: with other users (view or edit), a *Shared* view, and public links (view or edit,
  optional password and expiry).
- **WebDAV** at `/dav/` for desktop clients and, later, the apps' own cloud pages.
- **Safe saves**: every write carries an ETag. If someone else changed the file meanwhile, the
  shell keeps theirs and offers *Save a copy* for yours.
- **App updates** with SHA-256 and GPG signature checks, atomic switch-over and one-step rollback.
- Runs as **one container** (Docker / Podman), with **Compose**, a **Helm chart** or **one
  Kubernetes manifest**.

## Quick start

```sh
docker run -d --name artcraft-cloud -p 8080:8080 -v artcraft-data:/data ghcr.io/gheiler/artcraft-cloud:latest
docker logs artcraft-cloud | grep -A2 "admin password"     # the generated admin password, shown once
```

Open <http://localhost:8080>. The first start downloads the eight apps (about 550 MB) into the volume
in the background; they show up in the launcher as each one finishes.

> Anywhere but localhost, put it behind HTTPS (a reverse proxy or the Ingress below) and set
> `ARTCRAFT_PUBLIC_URL=https://…`. The apps need a secure context for WebGPU and the clipboard.

The image is published by CI from `main`. To build it yourself: `docker build -t artcraft-cloud .`

### Docker Compose

```sh
ARTCRAFT_ADMIN_PASSWORD='choose-a-long-one' docker compose up -d
docker compose --profile mail up -d       # optional Stalwart mail server, read docs/mail.md first
```

### Kubernetes, one file

```sh
kubectl apply -f https://raw.githubusercontent.com/gheiler/artcraft-cloud/main/deploy/kubernetes/artcraft-cloud.yaml
kubectl -n artcraft-cloud logs deploy/artcraft-cloud | grep -A2 "admin password"
kubectl -n artcraft-cloud port-forward svc/artcraft-cloud 8080:80
```

Uncomment the Ingress at the bottom of the file and set your host for HTTPS.

### Helm

```sh
helm install office ./deploy/helm/artcraft-cloud \
  --set publicUrl=https://office.example.com \
  --set ingress.enabled=true --set ingress.host=office.example.com \
  --set ingress.annotations."cert-manager\.io/cluster-issuer"=letsencrypt
```

One replica with `Recreate` (SQLite has one writer), a 10 Gi PVC, startup/liveness/readiness probes on
`/healthz` and `/readyz`, non-root, read-only root filesystem. See `deploy/helm/artcraft-cloud/values.yaml`.

## Apps

| App | Opens | Release tested | Status |
|-----|-------|----------------|--------|
| WordCraft | docx, odt, rtf, txt, md | 0.18.1 | open, edit, save, conflict copy: tested end to end |
| GridCraft | xlsx | 0.19.1 | new, Save As, reopen: tested |
| DeckCraft | pptx, potx, ppsx, deckcraft | 0.18.1 | new, Save As: tested |
| PdfCraft | pdf | 0.22.0 | open, save: tested |
| CADCraft | dxf | 0.20.1 | open, save: tested |
| FilmCraft | fcproj | 0.20.1 | new, Save As: tested |
| FigCraft | figcraft, fig | 0.18.1 | handshake tested |
| LightCraft | jpg, png, webp, gif, bmp, tif, dng | 0.21.1 | handshake and open tested |

PhotoCraft, VectorCraft, DesignCraft and the other Crafting Apps are not included yet: their web
builds do not speak the host bridge. `docs/upstream-gaps.md` lists what is missing upstream, with the
existing issues and pull requests.

## Updating the apps

```sh
docker exec artcraft-cloud artcraft-cloud apps list
docker exec artcraft-cloud artcraft-cloud apps update               # all apps, latest release
docker exec artcraft-cloud artcraft-cloud apps update wordcraft     # one app
docker exec artcraft-cloud artcraft-cloud apps update wordcraft --version 0.18.0
docker exec artcraft-cloud artcraft-cloud apps rollback wordcraft   # back to the version before
```

`scripts/update-apps.sh` wraps the same commands for the host (`ARTCRAFT_CONTAINER=artcraft-cloud` or
`ARTCRAFT_K8S=deploy/artcraft-cloud`), for example from cron. Each update downloads the release's
`<app>-web-<version>.zip` and `SHA256SUMS.txt`, checks the hash, verifies `SHA256SUMS.txt.asc` against
the release's signing key with gpg (a release with half a signature is refused), checks the build
contains the host bridge, unpacks into a staging folder, precompresses it (brotli + gzip), renames it
into place and only then switches `apps/manifest.json`. The previous version stays on disk for
`rollback`. The running server picks up the switch on the next request; no restart.
Set `ARTCRAFT_GITHUB_TOKEN` if you hit GitHub's anonymous limit of 60 API calls an hour.

## Configuration

| Variable | Default | |
|----------|---------|---|
| `ARTCRAFT_DATA` | `/data` (image), `data` | Everything persistent: `db.sqlite`, `files/<user id>/`, `trash/`, `apps/`, `secret.key` |
| `ARTCRAFT_LISTEN` | `0.0.0.0:8080` | |
| `ARTCRAFT_PUBLIC_URL` | | `https://…` turns on Secure cookies (with `ARTCRAFT_COOKIE_SECURE=auto`) |
| `ARTCRAFT_ADMIN_USER` / `ARTCRAFT_ADMIN_PASSWORD` | `admin` / generated | Only used when the database has no users yet |
| `ARTCRAFT_COOKIE_SECURE` | `auto` | `1` / `0` to force |
| `ARTCRAFT_SESSION_HOURS` | `336` | |
| `ARTCRAFT_MAX_UPLOAD_MB` | `512` | |
| `ARTCRAFT_SKIP_APP_SYNC` | | `1` skips the first-start download |
| `ARTCRAFT_GITHUB_TOKEN` | | For app updates |
| `ARTCRAFT_SHELL_DIR` | `/opt/artcraft-cloud/shell` (image) | The built drive UI |
| `ARTCRAFT_OIDC_ISSUER` | | Reserved: the server refuses to start while OIDC is not implemented, rather than fall back to passwords silently |

Accounts from the command line: `artcraft-cloud user create --username ana --password '…' [--admin]`,
`artcraft-cloud user disable ana`, `artcraft-cloud user enable ana`.

## Memory

Measured on Linux x86_64 with the release binary (23 Nov 2026):

| | RSS |
|---|---|
| Idle after start | **7.7 MB** |
| After a login | 8.1 MB |
| Peak under load: 200 downloads of the 61 MB WordCraft wasm (32 at a time), 30 uploads of 20 MB, 20 logins, a WordCraft open/edit/save in Chrome | 52 MB (argon2 takes 19 MiB per login in flight) |
| Settled after that load | 12 MB |
| In the container (rootless Podman, `--read-only`, 128 MB limit): idle | 7.8 MB |
| In the container: during the first-start download of all 8 apps | 11.4 MB (peak 26 MB) |
| In the container: after an end-to-end WordCraft open, edit and save | 8.6 MB (peak 28 MB) |

Image: about 100 MB (Debian slim + gpg + a 9 MB binary). The drive UI is a 780 KB wasm file (250 KB gzipped).

The image sets `TOKIO_WORKER_THREADS=2`, `MALLOC_ARENA_MAX=2` and `MALLOC_MMAP_THRESHOLD_=131072`. Without
them the same binary idles at about 28 MB and holds about 48 MB after the first login, because glibc
keeps the argon2 buffer. The apps themselves are static files; their memory use is in the browser.

## Architecture

```
browser ── one origin ──────────────────────────────────────────────┐
  │  /               drive (Leptos → wasm)                           │
  │  /apps/<app>/    Crafting App web builds, iframed ?host=parent   │   artcraft-cloud (one process)
  │  /api/…          JSON API (cookie session + X-Requested-With)    │── axum ── SQLite (users, sessions, shares, trash)
  │  /dav/…          WebDAV (Basic auth)                             │        └─ data/files/<user>/…  plain files
  └──────────────────────────────────────────────────────────────────┘
```

Cargo workspace (a modular monolith, one binary):

- `crates/server`: HTTP API, sessions, permissions, file store, WebDAV, static app serving.
- `crates/cli`: the `artcraft-cloud` binary: `serve`, `healthcheck`, `apps`, `user`.
- `crates/shell`: the drive and the app host, compiled to wasm with trunk.
- `crates/types`: API types and the app catalog, shared by server and shell.

The shell is the parent side of `craft-web-host`: it waits for `<app>:ready`, answers `<app>:host`, sends
`<app>:open` with the bytes, stores each `<app>:save` with `If-Match`, asks for a name and folder when an
app sends a Save As or a new document, warns before leaving with unsaved changes (`<app>:dirty`), and
follows `<app>:handoff` to the right app. Apps and shell share one origin because the bridge only talks
to a same-origin parent.

## Security notes

- Passwords: argon2id (19 MiB, 2 passes). Logins are rate limited per user name (8 per 10 minutes, 20 for WebDAV).
- Sessions: random tokens, `HttpOnly`, `SameSite=Lax`, `Secure` behind HTTPS. API writes need the
  `X-Requested-With: artcraft` header (CSRF guard).
- Paths are resolved inside the user's home; `..`, absolute paths and symlinks out are rejected.
  App zips are checked for path traversal before unpacking.
- Public links are random 192-bit tokens, optionally with a password and an expiry.
- The apps run in same-origin iframes, so they are trusted code: only install builds from ArtCraft's
  signed releases (the updater refuses anything else).
- The content security policy allows `wasm-unsafe-eval` and `unsafe-eval`, which the apps' wasm
  loaders need.

## Roadmap

- Single sign-on via OIDC (the seam is in `crates/server/src/oidc.rs`).
- More apps as their web builds gain the bridge (PhotoCraft, VectorCraft, DesignCraft…).
- Live collaboration once ArtCraft has it beyond FigCraft (`figcraft-server` could run beside us).
- WebDAV-backed Open/Save inside the apps, a host file picker, read-only sessions (see `docs/upstream-gaps.md`).
- Mail through Stalwart as an optional module (`docs/mail.md`).
- S3-compatible storage, quotas, app passwords for WebDAV.

## Development

```sh
rustup target add wasm32-unknown-unknown        # and install trunk: https://trunkrs.dev
(cd crates/shell && trunk build --release)
ARTCRAFT_ADMIN_PASSWORD=dev-password-123 ARTCRAFT_COOKIE_SECURE=0 cargo run --release
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p artcraft-cloud-shell --target wasm32-unknown-unknown -- -D warnings
```

## License

MIT OR Apache-2.0, at your option, like ArtCraft. The optional Stalwart container is AGPL-3.0 and runs
as a separate process; it is never linked into artcraftCloud.
