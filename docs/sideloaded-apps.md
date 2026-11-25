# Sideloaded apps

Super admins can install an app that is not on the signed artcraftCloud trust list (`registry/apps.json`). That happens from the **Danger zone** at the bottom of Apps, or with `artcraft-cloud apps sideload`. The build is marked **Unverified**. It is never auto-updated unless the admin later turns that on.

The super admin chooses an **isolation mode** when installing the app (and can change it later). The default does not let the app use the signed-in session.

## Isolation

The mode is stored with the install (in the apps manifest) and shown in the audit log. The Danger zone form compares the three options (storage, session access, setup, risk):

1. **Sandboxed iframe** (default). The app is served on the same host, in an iframe with `sandbox="allow-scripts"` and **without** `allow-same-origin`. That gives it a unique opaque origin: it cannot read `parent.document`, `document.cookie`, or the shell's `localStorage`. Every sideloaded-app asset also carries `Content-Security-Policy: sandbox allow-scripts`, so opening `/apps/<id>` directly is sandboxed the same way. There is no lasting browser storage; settings go through the host bridge. The app cannot use the signed-in session.

2. **Regular (same origin)**. The app runs like an official app: same origin, full browser storage (`localStorage`, IndexedDB, cookies), and it **can act as the signed-in user**. This is high risk. The form shows a danger warning and requires an extra confirmation (`confirm_regular`, or `--confirm-regular` on the CLI). Capability tokens are still issued for the opened file (or a create-only token when launched without one).

3. **Separate domain**. The app is served from `ARTCRAFT_APPS_URL`, so it has a real origin with working browser storage, and no session access (that host never answers `/api` and never sets the session cookie). This option is disabled in the form until the apps URL is configured, with a hint on how to set it. In Helm, set `appsUrl` and `ingress.appsHost`.

The session cookie is `HttpOnly` and `SameSite=Lax`, so script cannot read it. Requests from a sandboxed iframe send `Origin: null`. The API **rejects** `Origin: null` unless the request carries a short-lived **capability token** the shell issued and passed over the postMessage bridge:

- opened file: a token scoped to that path, with read and write;
- new document: a create-only token; the app creates a file only through the shell's save dialog (the user picks the folder and the name).

Tokens expire after 30 minutes and are bound to the app id and the signed-in user. They never grant mail, calendar, or other people's files. They are issued in every isolation mode.

A **subpath** (`/apps/<id>`) is enough for the default sandbox; a second domain is not required. A wildcard or per-app subdomain would isolate apps from each other as well, at the cost of extra DNS and certificates, and is not used here.

Official document apps stay unsandboxed on the main origin. Moving them would mean rewriting the same-origin file-picker hooks and the settings sync that reads the app's `localStorage`. A shim could cover today's APIs, but a later official app that assumed same-origin for anything else would break, so they stay where they are.

### Deployment

The default Docker Compose, Helm and local `cargo run` setups need no second hostname. The default sandbox (and regular isolation) work out of the box.

**Optional second origin** (needed only for the separate-domain mode): set `ARTCRAFT_APPS_URL` (for example `https://apps.office.example.com` or, on a laptop, `http://apps.localhost:8080`). In Helm, set `appsUrl` and `ingress.appsHost` to the same hostname, and include that name on the existing certificate or set `ingress.tls.appsSecretName`. Setting `ARTCRAFT_APPS_URL=` (empty) or `off` leaves sandbox and regular on the main host, and hides the separate-domain choice.

## Installing a zip

On Apps, below Available, the Danger zone asks for:

- an id, name, version, optional publisher and description
- the file types the app should open (the zip does not declare them; see below)
- whether it speaks the host bridge (opens and saves the document you chose)
- isolation (sandboxed iframe by default; regular needs an extra confirmation; separate domain needs `ARTCRAFT_APPS_URL`)
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

A sideloaded app that speaks the host bridge starts with the **`document`** grant: it can open and **save the file the user chose**, and it can **create a new file** through the shell's save dialog (the user picks the folder and the name). It does not get the rest of the drive. The admin can also grant `drive` (pick or insert other files the user chooses in the shell's file dialog), `clipboard` and `fullscreen`. Mail and calendar access cannot be granted to a sideload.

Ordinary users see an **Unverified** badge on the launcher and on the document, and a warning the first time they open the app.

The Apps page shows a short audit log of install, update, rollback, disable, enable, remove, settings, isolation and source changes (who, when, which version and checksum, and the isolation mode). `artcraft-cloud apps audit` prints the same list from the data directory.

## Command line

On the server (the data directory already used by the process):

```
artcraft-cloud apps sideload ./myapp-web-0.1.0.zip --id myapp --name "My App" --ext md,txt --bridge --confirm
artcraft-cloud apps set-source myapp --github owner/repo
artcraft-cloud apps check myapp
artcraft-cloud apps update myapp --version 0.2.0
artcraft-cloud apps set-types myapp --ext md,txt
artcraft-cloud apps set-grants myapp --grants document,clipboard
artcraft-cloud apps set-isolation myapp sandbox
artcraft-cloud apps set-isolation myapp regular --confirm-regular
artcraft-cloud apps set-isolation myapp domain
artcraft-cloud apps disable myapp
artcraft-cloud apps audit
```

`artcraft-cloud apps install` remains the path for a local build of an app **already in the registry**. Use `apps sideload` for an unknown id.

## Configuration

| Variable | Default | Role |
| --- | --- | --- |
| `ARTCRAFT_APPS_URL` | unset | Optional second origin, used when a sideload is set to separate-domain isolation. Empty or `off` leaves sandbox and regular on the main host. |
| `ARTCRAFT_PUBLIC_URL` | | Used for Secure cookies and as the `frame-ancestors` parent of the apps origin. |
| `ARTCRAFT_MAX_UPLOAD_MB` | `512` | Cap for a sideload zip (and other uploads). |
| `ARTCRAFT_BLOCKED_APPS` | `security` | How a registry block is applied, including to sideloads (by id, version or checksum). |
