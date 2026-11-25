# Sideloaded apps

Super admins can install an app that is not on the signed artcraftCloud trust list (`registry/apps.json`). That happens from the **Danger zone** at the bottom of Apps, or with `artcraft-cloud apps sideload`. The build is marked **Unverified**. It is never auto-updated unless the admin later turns that on.

This is allowed only when the app cannot act as the signed-in person.

## Isolation

Sideloaded apps are served from a **second origin** (`ARTCRAFT_APPS_URL`), for example `https://apps.office.example.com` or, on a laptop, `http://apps.localhost:8080`. The session cookie is host-only (no `Domain` attribute), so it is not sent to the apps host. The apps host answers only `/apps/…` (and health checks). It never answers `/api`, `/dav`, `/jmap` or `/caldav`, and it never sets a cookie.

The shell talks to the iframe only over `postMessage`, as it already does for official document apps. MailCraft and CalendarCraft hooks are never injected into a sideload, even one that reuses those ids.

Official document apps stay on the main origin so in-app file pickers and stored settings keep working. Isolating them too would need a larger change to those hooks.

### Deployment

The same artcraftCloud process serves both hostnames. You need DNS (or `*.localhost` on a development machine) and, in production, a TLS certificate that covers the apps host.

**Docker Compose** defaults to:

- `ARTCRAFT_PUBLIC_URL=http://localhost:8080`
- `ARTCRAFT_APPS_URL=http://apps.localhost:8080`

Open the UI at `http://localhost:8080`. Modern browsers resolve `apps.localhost` to `127.0.0.1`, so the iframe loads from the same container without extra DNS.

**Helm:** set `appsUrl` (for example `https://apps.office.example.com`) and `ingress.appsHost` to the same hostname. The chart adds a second ingress rule to this service. Include that name on the existing certificate, or set `ingress.tls.appsSecretName` to a second secret.

If `ARTCRAFT_APPS_URL` is unset in production (a public URL that is not loopback), the Danger zone explains that sideload is refused until the apps origin is configured. Setting `ARTCRAFT_APPS_URL=` (empty) or `off` disables it on purpose.

## Installing a zip

On Apps, below Available, the Danger zone asks for:

- an id, name, version, optional publisher and description
- the file types the app should open (the zip does not declare them; see below)
- whether it speaks the host bridge (opens and saves the document you chose)
- an optional source (GitHub releases or a https manifest) so later versions can be checked
- the zip, a confirmation that this build is not on the trust list, and an extra confirmation if a file type (or an official app id) already exists

The SHA-256 of that zip is recorded and pinned. An update must match a hash the admin accepted, or be accepted as a new version. Zip-slip paths, the unpack size cap (800 MiB) and `ARTCRAFT_MAX_UPLOAD_MB` apply as they do for official installs. A bridge app must still contain the host-bridge markers.

Disable and Remove work as they do for official apps. Disabled apps are not served (403) and leave the launcher.

The official **blocklist still applies** by app id, version and checksum. A sideload cannot run a zip whose SHA-256 (or id+version) the registry has blocked.

## File types

Apps do not declare which suffixes they open. Official types come from the signed registry. For a sideload, the admin types them (and can edit them later). An extension is meant to belong to one default handler. If the type is already used:

- an official app stays the default unless the admin ticks “make this the default”
- two sideloads need the same kind of confirmation; the first one installed stays the default until that is changed

Open with… still lists every installed bridge app, so a conflict is never a dead end.

## Sources

A sideload can be tied to:

- **GitHub releases**, same layout as official apps: `https://github.com/<owner>/<repo>/releases/download/<tag>/<id>-web-<version>.zip` plus `SHA256SUMS.txt` (and an optional signature). The admin stores `owner/repo`.
- A **https manifest** (loopback http is allowed in a lab): JSON with `id` and `versions` (each `version`, `url`, `sha256`, optional `signature`).

“Check for versions” on the same Versions dialog used for official apps looks at the source. Auto-update stays off by default. The 6-hour loop records what the source offers, and installs it only when auto-update is on.

If the official registry later lists the same app id, the official entry wins for versions and file types, and Apps offers **Switch to official**. The sideloaded copy is not replaced until the admin accepts that.

## Grants, warnings and the audit log

A sideloaded app starts with the opened document only (`document` if it speaks the bridge). The admin can also grant `drive` (pick other files), `clipboard` and `fullscreen`. Mail and calendar access cannot be granted to a sideload.

Ordinary users see an **Unverified** badge on the launcher and on the document, and a warning the first time they open the app.

The Apps page shows a short audit log of install, update, rollback, disable, enable, remove, settings and source changes (who, when, which version and checksum). `artcraft-cloud apps audit` prints the same list from the data directory.

## Command line

On the server (the data directory already used by the process):

```
artcraft-cloud apps sideload ./myapp-web-0.1.0.zip --id myapp --name "My App" --ext md,txt --bridge --confirm
artcraft-cloud apps set-source myapp --github owner/repo
artcraft-cloud apps check myapp
artcraft-cloud apps update myapp --version 0.2.0
artcraft-cloud apps set-types myapp --ext md,txt
artcraft-cloud apps set-grants myapp --grants document,clipboard
artcraft-cloud apps disable myapp
artcraft-cloud apps audit
```

`artcraft-cloud apps install` remains the path for a local build of an app **already in the registry**. Use `apps sideload` for an unknown id.

## Configuration

| Variable | Default | Role |
| --- | --- | --- |
| `ARTCRAFT_APPS_URL` | `http://apps.localhost:<port>` when the public URL is local or unset; otherwise unset | Origin isolated apps are served from. Empty or `off` disables sideload. |
| `ARTCRAFT_PUBLIC_URL` | | Used for Secure cookies and as the `frame-ancestors` parent of the apps origin. |
| `ARTCRAFT_MAX_UPLOAD_MB` | `512` | Cap for a sideload zip (and other uploads). |
| `ARTCRAFT_BLOCKED_APPS` | `security` | How a registry block is applied, including to sideloads (by id, version or checksum). |
