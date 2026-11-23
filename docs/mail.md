# Mail (optional, later phase)

artcraftCloud does not include a mail server. The compose file has an opt-in `mail` profile
that runs [Stalwart](https://stalw.art) in its own container (`docker compose --profile mail up -d`).
Stalwart is AGPL-3.0; it is never linked into artcraftCloud, which stays MIT OR Apache-2.0.

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

## Wiring it up

1. `docker compose --profile mail up -d`, then open the Stalwart admin UI at
   `http://127.0.0.1:8081` (the compose file only binds it to localhost; tunnel over SSH).
2. Follow Stalwart's setup for your domain, DNS records and relay.
3. artcraftCloud accounts and Stalwart accounts are separate for now. Single sign-on through OIDC
   is on the roadmap.
