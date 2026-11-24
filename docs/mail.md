# Mail (optional)

artcraftCloud does not include a mail server. It connects to a [Stalwart](https://stalw.art)
server (1.x, the JMAP management API) over HTTP, creates mailboxes there for the people you give an
email address, and lets [MailCraft](https://github.com/UCS-Care-Group/mailcraft) open each person's
mailbox without a password. Each mailbox's password is the person's artcraftCloud password, so
they can also use it in any other mail app (IMAP/SMTP). The compose file has an opt-in `mail` profile that runs Stalwart in its
own container (`docker compose --profile mail up -d`). Stalwart is AGPL-3.0; it is never linked into
artcraftCloud, which stays MIT OR Apache-2.0.

**Trust.** MailCraft comes from UCS-Care-Group, not from the ArtCraft project, and runs on the same
origin as the rest of artcraftCloud (it can call the API as the signed-in person, like every app
here). Trusting it is a deliberate, approved choice: its web builds are installed only when they
carry a valid ed25519 signature from MailCraft's release key, pinned in artcraftCloud's catalog
(`crates/types/src/lib.rs`), exactly like the ArtCraft apps.

## Will my mail land in spam?

It will if you send straight from a home or dynamic IP. Those ranges are on Spamhaus PBL, most ISPs
block outbound port 25, and you cannot set the reverse DNS. Before you switch the profile on, check
you have all of these:

- **A static IP with reverse DNS (PTR)** that matches the mail host name (a VPS, not a home line).
- **Port 25 open both ways** at the provider (many clouds block it until you ask).
- **SPF**, **DKIM** (Stalwart generates the key) and **DMARC** records in DNS.
- **TLS** on SMTP (Stalwart can get a Let's Encrypt certificate).
- **A smarthost for outbound mail** (an EU transactional relay, for example) while the new IP has
  no reputation. Microsoft in particular rejects new IPs. Stalwart can route outbound mail through
  the relay and still receive mail directly.

Memory: Stalwart idles around 55 MB, but its default caches can grow past 2 GB. Set the
`mem_limit` in compose and lower the cache sizes in Stalwart's settings on small hosts.

## Setting it up

1. **Start Stalwart.** `docker compose --profile mail up -d`. On first start Stalwart is in
   *bootstrap mode* and prints a one-time admin password: `docker compose logs stalwart`. (Or pin one
   with `STALWART_RECOVERY_ADMIN=admin:<password>`; see the comment in `compose.yaml`. Remove it
   again once the permanent administrator exists: while it is set it is a full-access login.)
2. **Finish Stalwart's own setup** in its admin UI at `http://127.0.0.1:8081` (bound to localhost
   only; tunnel over SSH): host name, first domain, the permanent administrator, TLS and the relay.
   Until that is done Stalwart only answers its setup calls, and *Test connection* says so.
3. **Connect it in artcraftCloud.** Sign in as the super admin, open **Mail** in the sidebar:
   - *Server address*: Stalwart's HTTP address **as the artcraftCloud server reaches it**. With the
     compose profile that is `http://stalwart:8080`, which the page offers (it comes from
     `ARTCRAFT_MAIL_URL`; Helm: `mailUrl`, or the chart's own Stalwart). Plain HTTP is fine on the
     internal network; use `https://` when the mail server is elsewhere.
   - *Sign in with*: **API key (recommended, the default).** Create one in Stalwart's web UI as the
     administrator (*Account › Credentials › API Keys*, permissions *Inherit*) and paste it. A key
     can be revoked on its own and never exposes the admin password.
   - Or **Admin account (fallback)**: the administrator's name and password. With *Use this login
     once to create an API key* (on by default) artcraftCloud asks Stalwart for a key with that
     login, stores **only the key**, and forgets the password. Turn it off only if you must keep
     the admin login itself (it is then stored encrypted).
   - *Mail host for other apps* (optional): the host name people type into Thunderbird or a phone,
     e.g. `mail.example.com`. Empty means the MX host of their domain.
   - *Save*; the page tests the connection. The credential must be allowed to list, create and
     delete domains and accounts, change account passwords and give accounts app passwords:
     `sysDomainGet`, `sysDomainQuery`, `sysDomainCreate`, `sysAccountGet`, `sysAccountQuery`,
     `sysAccountCreate`, `sysAccountUpdate`, `sysAccountDestroy`, `impersonate`,
     `sysAppPasswordCreate`, `sysAppPasswordDestroy`. Stalwart's administrator role has them all;
     an API key with *Inherit* gets them from that account. (A key with a *Replace* list cannot
     manage ordinary accounts unless it holds every permission they have, so keep *Inherit*; for a
     narrower blast radius make a dedicated administrator account for artcraftCloud and create the
     key there.)
4. **Domains.** The same page lists Stalwart's domains and adds new ones. Each domain shows the DNS
   records Stalwart generated for it: MX, SPF, DKIM and DMARC first, then the optional ones
   (autoconfig, SRV, MTA-STS, TLS reporting). Publish them at your DNS provider.
5. **People.** In *People*, give someone an email address when you create them (or later with the
   *Email* button). When the address's domain is on the mail server, *Create the mailbox* is offered
   and on by default; artcraftCloud then creates the account in Stalwart, with the password you
   just typed for the person, and links it. An address on another domain is kept as the person's
   email only.

### Connecting from the command line (compose, Helm)

The same connection can be made without the browser, e.g. right after Stalwart's setup in a
provisioning script. The admin password is read from standard input, used once to create an API
key, and only the key is stored:

```sh
# compose
printf '%s\n' "$STALWART_ADMIN_PASSWORD" | docker compose exec -T artcraft-cloud \
  artcraft-cloud mail connect --url http://stalwart:8080 --admin admin --public-host mail.example.com
# Helm
printf '%s\n' "$STALWART_ADMIN_PASSWORD" | kubectl exec -i deploy/<release>-artcraft-cloud -- \
  artcraft-cloud mail connect --url http://<release>-artcraft-cloud-stalwart:8080 --admin admin
# or with a key you created yourself
STALWART_KEY=API_… artcraft-cloud mail connect --url http://stalwart:8080 --api-key-env STALWART_KEY
artcraft-cloud mail test
```

`mail connect` tests the stored credential and exits non-zero when it cannot manage mailboxes.

The credential is stored **encrypted** (ChaCha20-Poly1305 with a key derived from the data volume's
`secret.key`, which is made `0600`); the API never returns it, the page only shows that one is
stored. *Disconnect* forgets it; nothing is deleted on the mail server.

## Passwords: one for artcraftCloud and the mailbox

A person's mailbox password **is their artcraftCloud password**. They sign in to any IMAP/SMTP mail
app with their email address and that password; *Settings › Account › Other mail apps* shows the
host, the ports (IMAP 993 and SMTP 465 over TLS; 587 with STARTTLS where it is turned on) and the
user name.

artcraftCloud keeps it in step by passing every change to Stalwart:

- **New person with a mailbox:** the account is created with the password the admin typed.
- **Mailbox added later** (*Email* button) or **mailboxes from before this version:** artcraftCloud
  does not know the plaintext password (it keeps only an Argon2 hash), so the account starts with a
  generated password and is marked *pending*. People shows "mail password at next sign-in". The
  next time the person signs in (or changes their password) the password they typed is set on the
  mailbox, in the background; sign-in does not wait for the mail server.
- **Changing your own password** (*Settings › Account*) and **an admin setting someone's
  password** (*People › Set password*) change artcraftCloud first and then Stalwart, and the answer
  says whether the mailbox followed.
- **If Stalwart cannot be reached** the artcraftCloud password still changes (you are not locked
  out of artcraftCloud because mail is down), the mailbox is marked *failed* with the reason, and
  the new password is kept **sealed** (ChaCha20-Poly1305, like the other secrets) only until the
  retry gets through. artcraftCloud retries every 5 minutes for a day and at every sign-in, and a
  retry that would set an outdated password (the person changed it again meanwhile) is dropped.
  The page tells the person that other mail apps still use the previous password; People shows
  "mail password not updated". The two never drift silently.
- There is no forced password change at first sign-in in artcraftCloud, so there is nothing else to
  pass on. `artcraft-cloud user create` makes accounts without email.

A password changed **in Stalwart's own web UI** is not seen by artcraftCloud; the two then differ
until the next change in artcraftCloud. Ask people to change it in artcraftCloud.

## How MailCraft gets in

MailCraft is a static web app that speaks JMAP. artcraftCloud serves it from the same origin and
answers both things it needs:

- **Which account.** artcraftCloud adds one script in front of MailCraft's own
  (`/api/mail/mailcraft.js`, per user, `no-store`). It sets `window.mailcraftJmap`, MailCraft's
  documented hook for sites that sign their users in themselves, naming the person's address and the
  session URL `<this origin>/jmap/session`.
- **Which credential.** Each mailbox gets a **Stalwart app password** named "artcraftCloud
  MailCraft" that artcraftCloud creates when it makes the mailbox (older mailboxes get one at the
  person's next sign-in), seals in its database, and never sends to a browser. The person's own
  password is never stored in plaintext, and the app password does not change when they change
  theirs. When MailCraft needs to sign in it calls the hook's `secret()`, which asks
  `POST /api/mail/token` (the person's normal session cookie) for a **15-minute token** bound to the
  user (HMAC with the server secret).
- **The proxy.** MailCraft sends its JMAP requests with that token to `/jmap/…` on artcraftCloud. The
  proxy checks the token, that the person is still enabled and still has the mailbox, and forwards
  the request to Stalwart with the mailbox's address and app password. Only `/jmap/…` is forwarded;
  the session's URLs are rewritten to this origin and the WebSocket capability is removed, so
  MailCraft talks plain HTTP through the proxy. No CORS setup is needed on Stalwart, and Stalwart
  never needs to be reachable from the internet over HTTP.

Why an app password, not the person's password? artcraftCloud would have to keep the plaintext
password to sign in with it, and every password change would briefly break MailCraft. The app
password is separate, can be seen and revoked in Stalwart (*Account › Credentials*), and is revoked
by artcraftCloud when the mailbox is unlinked. Why not hand it to MailCraft? It would sit in the
browser (MailCraft keeps accounts in `localStorage`) and outlive a disabled account; the token
expires in minutes and is checked against the account on every request.

## Disabling, unlinking and deleting

- **Disabling a person** keeps their mailbox and its mail; mail keeps arriving. The proxy refuses
  them, so MailCraft in artcraftCloud stops working at once. Enabling them again restores it.
  Other mail apps keep working with their password: to stop them (or delivery), suspend the
  account in Stalwart too.
- **Email › Unlink mailbox** (the default) only removes the link in artcraftCloud and revokes
  artcraftCloud's app password. The mailbox stays on the mail server with all its mail, and the
  person's password keeps working there. Linking the same address again is refused, so an existing
  mailbox is never taken over silently; delete or rename it in Stalwart first.
- **Email › Delete mailbox…** deletes the account and all its mail in Stalwart. It asks you to type the
  address, and the server checks it (`DELETE /api/users/{id}/mailbox` with `{"delete": true,
  "confirm": "<address>"}`).
- **Deleting the person** is not offered by artcraftCloud today; when it is, it will unlink, never
  delete mail.
- An address cannot be changed while a mailbox is linked; unlink first.

## Shared browsers

MailCraft keeps a copy of the mail it has fetched in the browser's `localStorage`
(`mailcraft.mailbox.v1`). artcraftCloud never uploads that key with the app's options, removes it
when a different person opens MailCraft in the same browser, and removes it on *Sign out*. Anyone
with the browser profile in between can still read it, as with any webmail cache.

## Upgrading Stalwart

The image is **pinned** (`docker.io/stalwartlabs/stalwart:v1.2.0` in `compose.yaml`;
`stalwart.image.tag` in the Helm values), so nothing changes under you on a restart. To upgrade:

1. Read the release notes between your version and the new one
   (<https://github.com/stalwartlabs/stalwart/releases>), especially for a new minor or major
   version, which may migrate the data store.
2. Back up both volumes (`stalwart-config` → `/etc/stalwart`, `stalwart-data` →
   `/var/lib/stalwart`), e.g. `docker run --rm -v artcraft-cloud_stalwart-data:/d -v "$PWD":/b alpine
   tar czf /b/stalwart-data.tgz -C /d .` with Stalwart stopped.
3. Change the tag: `STALWART_TAG=v1.x.y` in `.env` (compose) or `--set stalwart.image.tag=v1.x.y`
   (Helm).
4. `docker compose --profile mail pull stalwart && docker compose --profile mail up -d stalwart`
   (Helm: `helm upgrade`).
5. In artcraftCloud, **Mail › Test connection**, then open MailCraft once.

Earlier versions of `compose.yaml` mounted the volume at `/opt/stalwart`, which Stalwart 1.x does
not use: its configuration and data lived in the container and were lost when it was recreated.
The file now mounts `/etc/stalwart` and `/var/lib/stalwart`. If you ran the old file, export what you
need from Stalwart before recreating the container, or copy `/etc/stalwart` and `/var/lib/stalwart`
out of the running container (`docker cp`) into the new volumes.

## API

| Call | Who | What |
|---|---|---|
| `GET/PUT/DELETE /api/mail/config` | super admin | base URL, `auth` (`bearer`/`basic`), user name, `secret` (write only; empty keeps it), `create_api_key`, `public_host` |
| `POST /api/mail/test` | super admin | `{ok, message, edition, can_manage}` |
| `GET /api/mail/domains` | admins | domains; with DNS records for the super admin |
| `POST /api/mail/domains` | super admin | `{name}` |
| `GET /api/users/{id}/mail` | admins | `{email, mailbox, configured, domain_served, password_sync, sync_error}` |
| `PUT /api/users/{id}/email` | admins | `{email, create_mailbox}` |
| `DELETE /api/users/{id}/mailbox` | admins | `{delete, confirm}` |
| `GET /api/mail/me` | everyone | their own address, mailbox, `password_sync`, and `client` (host, user name, ports) |
| `POST /api/me/password`, `POST /api/users/{id}/password` | the person / admins | `{mail: none\|synced\|failed, error}` |
| `POST /api/mail/token` | everyone with a mailbox | `{token, expiresIn}` for the proxy |
| `/jmap/…` | bearer token | the JMAP proxy |

Admins follow the same rules as elsewhere: only the super admin touches another admin's account.

## Testing against a real Stalwart

```sh
podman run -d --name acc-stalwart -p 127.0.0.1:8091:8080 -p 127.0.0.1:9993:993 \
  -v acc-stalwart-etc:/etc/stalwart -v acc-stalwart-data:/var/lib/stalwart docker.io/stalwartlabs/stalwart:v1.2.0
podman logs acc-stalwart          # the bootstrap admin; finish setup at http://127.0.0.1:8091/admin
cd tests/e2e && MAIL_URL=http://127.0.0.1:8091 MAIL_ADMIN=admin@example.test MAIL_PASS=… node mail.js
# a mailbox signs in over IMAP with the artcraftCloud password:
curl -sk --url imaps://127.0.0.1:9993/INBOX --user 'ada@example.test:<password>' -X 'EXAMINE INBOX'
```

`tests/e2e/mail.js` connects the server (making an API key from the admin login), checks the DNS
records, creates a person with a mailbox, changes a password, and opens MailCraft on the inbox
through the proxy; with `IMAP_URL` set it also checks IMAP with the old and the new password. The
Rust tests (`cargo test -p artcraft-cloud-server mail`) run the same API against a stand-in Stalwart.
