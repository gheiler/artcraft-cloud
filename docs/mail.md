# Mail (optional)

artcraftCloud does not include a mail server. It connects to a [Stalwart](https://stalw.art)
server (1.x, the JMAP management API) over HTTP, creates mailboxes there for the people you give an
email address, and lets [MailCraft](https://github.com/UCS-Care-Group/mailcraft) open each person's
mailbox without a password. The compose file has an opt-in `mail` profile that runs Stalwart in its
own container (`docker compose --profile mail up -d`). Stalwart is AGPL-3.0; it is never linked into
artcraftCloud, which stays MIT OR Apache-2.0.

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
   with `STALWART_RECOVERY_ADMIN=admin:<password>`; see the comment in `compose.yaml`.)
2. **Finish Stalwart's own setup** in its admin UI at `http://127.0.0.1:8081` (bound to localhost
   only; tunnel over SSH): host name, first domain, the permanent administrator, TLS and the relay.
   Until that is done Stalwart only answers its setup calls, and *Test connection* says so.
3. **Connect it in artcraftCloud.** Sign in as the super admin, open **Mail** in the sidebar:
   - *Server address*: Stalwart's HTTP address **as the artcraftCloud server reaches it**. With the
     compose profile that is `http://stalwart:8080`, which the page offers (it comes from
     `ARTCRAFT_MAIL_URL`; Helm: `mailUrl`). Plain HTTP is fine on the internal network; use
     `https://` when the mail server is elsewhere.
   - *Sign in with*: an **API key** (Stalwart › Account › API keys; give it the domain and account
     permissions below) or an **admin account** and its password. An API key is the better choice:
     it can be scoped and revoked without touching the admin login.
   - *Save*, then *Test connection*. It checks that the credential may list, create and delete
     domains and accounts (`sysDomainGet`, `sysDomainQuery`, `sysDomainCreate`, `sysAccountGet`,
     `sysAccountQuery`, `sysAccountCreate`, `sysAccountDestroy`).
4. **Domains.** The same page lists Stalwart's domains and adds new ones. Each domain shows the DNS
   records Stalwart generated for it: MX, SPF, DKIM and DMARC first, then the optional ones
   (autoconfig, SRV, MTA-STS, TLS reporting). Publish them at your DNS provider.
5. **People.** In *People*, give someone an email address when you create them (or later with the
   *Email* button). When the address's domain is on the mail server, *Create the mailbox* is offered
   and on by default; artcraftCloud then creates the account in Stalwart and links it to the person.
   An address on another domain is kept as the person's email only.

The credential is stored **encrypted** (ChaCha20-Poly1305 with a key derived from the data volume's
`secret.key`, which is made `0600`); the API never returns it, the page only shows that one is
stored. *Disconnect* forgets it; nothing is deleted on the mail server.

## How MailCraft gets in

MailCraft is a static web app that speaks JMAP. artcraftCloud serves it from the same origin and
answers both things it needs:

- **Which account.** artcraftCloud adds one script in front of MailCraft's own
  (`/api/mail/mailcraft.js`, per user, `no-store`). It sets `window.mailcraftJmap`, MailCraft's
  documented hook for sites that sign their users in themselves, naming the person's address and the
  session URL `<this origin>/jmap/session`.
- **Which credential.** Each mailbox gets a **40-character random password** that artcraftCloud
  generates, gives to Stalwart once, and keeps sealed in its database. It never goes to a browser.
  When MailCraft needs to sign in it calls the hook's `secret()`, which asks
  `POST /api/mail/token` (the person's normal session cookie) for a **15-minute token** bound to the
  user (HMAC with the server secret). MailCraft keeps none of it beyond the job.
- **The proxy.** MailCraft sends its JMAP requests with that token to `/jmap/…` on artcraftCloud. The
  proxy checks the token, that the person is still enabled and still has the mailbox, and forwards
  the request to Stalwart with the mailbox's own address and password. Only `/jmap/…` is forwarded;
  the session's URLs are rewritten to this origin and the WebSocket capability is removed, so
  MailCraft talks plain HTTP through the proxy. No CORS setup is needed on Stalwart, and Stalwart
  never needs to be reachable from the internet over HTTP.

Why not hand MailCraft the password, or a Stalwart app password? Either would sit in the browser
(MailCraft keeps accounts in `localStorage`) and outlive a disabled account; the token expires in
minutes and is checked against the account on every request. Why not log in to Stalwart with
artcraftCloud's own passwords (single sign-on)? Stalwart would have to trust artcraftCloud as an
OIDC provider, which artcraftCloud is not yet; the sealed generated password works with any Stalwart
and can be replaced by OIDC later without changing MailCraft's side.

People still get mail into other clients (phones, Thunderbird) by setting a password or app password
for their mailbox in Stalwart's web UI; artcraftCloud does not manage those.

## Disabling, unlinking and deleting

- **Disabling a person** keeps their mailbox and its mail; mail keeps arriving. The proxy refuses
  them, so MailCraft in artcraftCloud stops working at once. Enabling them again restores it.
  (To stop delivery or other clients too, suspend the account in Stalwart.)
- **Email › Unlink mailbox** (the default) only removes the link in artcraftCloud. The mailbox stays
  on the mail server with all its mail. Linking the same address again is refused, so an existing
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

## API

| Call | Who | What |
|---|---|---|
| `GET/PUT/DELETE /api/mail/config` | super admin | base URL, `auth` (`basic`/`bearer`), user name, `secret` (write only; empty keeps it) |
| `POST /api/mail/test` | super admin | `{ok, message, edition, can_manage}` |
| `GET /api/mail/domains` | admins | domains; with DNS records for the super admin |
| `POST /api/mail/domains` | super admin | `{name}` |
| `GET /api/users/{id}/mail` | admins | `{email, mailbox, configured, domain_served}` |
| `PUT /api/users/{id}/email` | admins | `{email, create_mailbox}` |
| `DELETE /api/users/{id}/mailbox` | admins | `{delete, confirm}` |
| `GET /api/mail/me` | everyone | their own address and mailbox |
| `POST /api/mail/token` | everyone with a mailbox | `{token, expiresIn}` for the proxy |
| `/jmap/…` | bearer token | the JMAP proxy |

Admins follow the same rules as elsewhere: only the super admin touches another admin's account.

## Testing against a real Stalwart

```sh
podman run -d --name acc-stalwart -p 127.0.0.1:8091:8080 docker.io/stalwartlabs/stalwart:latest
podman logs acc-stalwart          # the bootstrap admin; finish setup at http://127.0.0.1:8091/admin
cd tests/e2e && MAIL_URL=http://127.0.0.1:8091 MAIL_ADMIN=admin@example.test MAIL_PASS=… node mail.js
```

`tests/e2e/mail.js` connects the server, checks the DNS records, creates a person with a mailbox,
unlinks and deletes, changes a password, and opens MailCraft on the inbox through the proxy. The
Rust tests (`cargo test -p artcraft-cloud-server mail`) run the same API against a stand-in Stalwart.
