# Calendars: CalendarCraft with Stalwart

[CalendarCraft](https://github.com/gheiler/calendarcraft) is a Fantastical-style calendar (Rust,
egui, WebAssembly). In artcraftCloud it shows each person's calendar on the Stalwart server that
holds their mailbox, opens `.ics` files from the drive, and can add the person's own calendar
accounts and feeds.

## How it reaches the person's calendar

Stalwart keeps a CalDAV home for every account at `/dav/cal/<address>/`. artcraftCloud already
makes that account (People › email address) and keeps an app password for it, sealed on the
server; MailCraft's JMAP proxy uses the same one.

- `GET /api/calendar/calendarcraft.js`, put first in CalendarCraft's `index.html` by the server,
  sets `window.calendarcraftHost = { accounts: [{ id: "artcraft", url: "/caldav/<address>/", … }],
  remoteProxy: "/api/calendar/remote" }`. No mailbox, no account (the app still works with local
  calendars, files and feeds).
- `/caldav/*` → `<stalwart>/dav/cal/*` as the signed-in person (session cookie; disabled people
  are refused), with Basic auth `address:app password`. Only CalendarCraft's own requests
  (`X-Requested-With: artcraft`), only `GET HEAD OPTIONS PROPFIND REPORT PUT DELETE`, no path that
  leaves the CalDAV space. `Depth`, `If-Match`, `If-None-Match`, `Content-Type` go up; `ETag`,
  `Location` come back; `/dav/cal/` in replies becomes `/caldav/` (artcraftCloud's drive is `/dav/`).
  A `401` from Stalwart becomes a `502` (no browser password prompt).
- Nothing to set up on Stalwart: no CORS, no extra credentials.

Other apps (Apple Calendar, Thunderbird, DAVx⁵) use the same calendar directly: server
`https://<mail host>/dav/cal/<address>/` (or the host, through `/.well-known/caldav`), user the
email address, password the artcraftCloud password.

## The person's own calendars

CalendarCraft › Settings › Accounts adds a CalDAV account (iCloud, Fastmail, Nextcloud…) or an
`.ics` feed (Google's secret iCal address, Outlook's published calendar, holidays). The page may
only talk to this origin (CSP `connect-src 'self'`), so those requests go through
`/api/calendar/remote`:

- the target is `x-calendar-url`;
- the password is stored with `PUT /api/calendar/remote/credentials/:account` (and removed with
  `DELETE`), sealed in the vault under `calendar-remote:<user_id>:<account_id>`. The proxy
  attaches it when the request has `x-calendar-account`. Older CalendarCraft builds may still
  send `x-calendar-auth` (passed on as `Authorization`); new builds do not send the password;
- signed-in people only, CalendarCraft's own requests only, the same methods as `/caldav/`
  (the credentials URL is PUT/DELETE);
- `http`/`https` only, no user name in the URL, **public addresses only** (loopback, private,
  link-local, CGNAT, multicast and reserved ranges are refused; `ARTCRAFT_CALENDAR_ALLOW_PRIVATE=1`
  for a lab), and the request goes to the address that was checked (no second lookup);
- redirects come back to the app (which follows them through the proxy again, so each hop is
  checked); `WWW-Authenticate` is dropped; replies up to 32 MB.

Passwords for those accounts are sealed on the server (see above) and are not kept in the browser.
The shell still treats `calendarcraft.secrets` and `calendarcraft.cache` as local-only (a leftover
plaintext map from an older CalendarCraft is migrated into the vault and deleted on start; the
cache is a copy of server events). Accounts and local calendars (`calendarcraft.store`) are options
and follow the person.

## Files and settings

- `.ics`, `.ical`, `.ifb` open in CalendarCraft; the file shows as its own calendar, and Save /
  Autosave write it back (only that calendar's events).
- Settings › theme, language (en, de, es, fr, it, pt-BR) and animations go into `calendarcraft.ui`.

## Installing

CalendarCraft is in the [app registry](registry.md) (owner `gheiler`) with release 0.2.2 (and 0.2.1, 0.2.0), signed
with CalendarCraft's ed25519 release keys (the same format as MailCraft). It installs like the
other apps (Apps page or `artcraft-cloud apps update calendarcraft`). To try a build of your own:

```sh
# in the CalendarCraft repo
sh scripts/package-web.sh            # dist-release/calendarcraft-web-0.2.0.zip
# on the server
artcraft-cloud apps install calendarcraft calendarcraft-web-0.2.0.zip --version 0.2.0
```

`apps install` checks the archive like a download (safe names, the host bridge) and records it
without a signature.

## Testing

- `cargo test -p artcraft-cloud-server mail_server` covers the hook, `/caldav/` (as the mailbox, with
  the app password, paths rewritten, methods and paths refused) and the remote proxy's refusals,
  against the in-process fake Stalwart. `calendar_remote_credentials_are_sealed_and_not_returned`
  covers storing a remote password (sealed, bound to the person and account, never returned).
- `tests/e2e/calendar.js` (real Stalwart): CalendarCraft finds the calendar; an event typed in the
  app is on Stalwart (`curl` REPORT and PROPFIND as the person); an event `PUT` with `curl` shows in
  the app; a public `.ics` feed comes in through `/api/calendar/remote`; an `.ics` in the drive opens
  and is saved back with a new event.

```sh
BASE=http://127.0.0.1:8097 STALWART=http://127.0.0.1:8091 E2E_USER=cara E2E_PASS=… node tests/e2e/calendar.js
```
