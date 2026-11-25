//! A small client for Stalwart's management API (Stalwart 1.x: JMAP methods `x:Domain/*`,
//! `x:Account/*`, `x:AppPassword/*` and `x:ApiKey/*` on `/jmap/`, plus `GET /api/account`).
//! artcraftCloud lists and adds domains, creates or deletes its users' mailboxes, sets their
//! passwords, and gives each one an app password for its JMAP proxy.

use std::sync::OnceLock;
use std::time::Duration;

use artcraft_cloud_types::{DnsRecord, MailDomain, MailTest};
use serde_json::{json, Value};

const USING: [&str; 2] = ["urn:ietf:params:jmap:core", "urn:stalwart:jmap"];

/// One client for the process: a connection pool, no redirects (a credential is never sent on).
pub fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("artcraft-cloud/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .pool_max_idle_per_host(2)
            .build()
            .expect("http client")
    })
}

#[derive(Clone, Debug)]
pub enum Auth {
    Basic { user: String, pass: String },
    Bearer(String),
}

#[derive(Clone, Debug)]
pub struct Stalwart {
    base: String,
    auth: Auth,
}

/// `http(s)://host[:port]` without a trailing slash, or why not.
pub fn normalize_base(url: &str) -> Result<String, String> {
    let trimmed = url.trim().trim_end_matches('/');
    let parsed = url::Url::parse(trimmed).map_err(|_| "the mail server address is not a URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("the mail server address must start with http:// or https://".into());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() || !parsed.username().is_empty() {
        return Err("the mail server address takes no query, fragment or user name".into());
    }
    Ok(trimmed.to_string())
}

impl Stalwart {
    pub fn new(base: &str, auth: Auth) -> Result<Self, String> {
        Ok(Self { base: normalize_base(base)?, auth })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let builder = http().request(method, format!("{}{path}", self.base)).timeout(Duration::from_secs(20));
        match &self.auth {
            Auth::Basic { user, pass } => builder.basic_auth(user, Some(pass)),
            Auth::Bearer(token) => builder.bearer_auth(token),
        }
    }

    /// Run JMAP method calls; returns each response as `(name, arguments)`.
    async fn jmap(&self, calls: Value) -> Result<Vec<(String, Value)>, String> {
        self.jmap_body(json!({ "using": USING, "methodCalls": calls })).await
    }

    async fn jmap_body(&self, body: Value) -> Result<Vec<(String, Value)>, String> {
        let response = self.request(reqwest::Method::POST, "/jmap/").json(&body).send().await.map_err(reach)?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err("the mail server refused the admin credential".into());
        }
        if !status.is_success() {
            return Err(format!("the mail server answered {status}"));
        }
        let value: Value = response.json().await.map_err(|_| "the mail server's answer is not JMAP".to_string())?;
        let list = value.get("methodResponses").and_then(Value::as_array).ok_or("the mail server's answer is not JMAP")?;
        let mut out = Vec::new();
        for item in list {
            let name = item.get(0).and_then(Value::as_str).unwrap_or_default().to_string();
            let args = item.get(1).cloned().unwrap_or(Value::Null);
            if name == "error" {
                let kind = args.get("type").and_then(Value::as_str).unwrap_or("error");
                let text = args.get("description").and_then(Value::as_str).unwrap_or("");
                return Err(match kind {
                    "forbidden" => "the admin credential may not manage domains and accounts".into(),
                    "unknownMethod" | "unknownCapability" => "this mail server has no management API (Stalwart 0.16 or later is needed)".into(),
                    _ => format!("the mail server said {kind} {text}").trim().to_string(),
                });
            }
            out.push((name, args));
        }
        Ok(out)
    }

    /// Check the address and credential, and whether it may manage domains and accounts.
    pub async fn test(&self) -> MailTest {
        let response = match self.request(reqwest::Method::GET, "/api/account").send().await {
            Ok(response) => response,
            Err(err) => return MailTest { ok: false, message: reach(err), ..MailTest::default() },
        };
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return MailTest { ok: false, message: "the mail server refused the admin credential".into(), ..MailTest::default() };
        }
        if !status.is_success() {
            return MailTest { ok: false, message: format!("the mail server answered {status}; is this Stalwart?"), ..MailTest::default() };
        }
        let Ok(account) = response.json::<Value>().await else {
            return MailTest { ok: false, message: "the answer is not Stalwart's".into(), ..MailTest::default() };
        };
        let permissions: Vec<&str> = account.get("permissions").and_then(Value::as_array).map(|list| list.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let edition = account.get("edition").and_then(Value::as_str).unwrap_or_default().to_string();
        if permissions.contains(&"sysBootstrapUpdate") && !permissions.contains(&"sysDomainGet") {
            return MailTest { ok: false, message: "Stalwart is still in setup mode: finish its setup at /admin first".into(), edition, can_manage: false };
        }
        let needed = NEEDED;
        let missing: Vec<&str> = needed.iter().copied().filter(|p| !permissions.contains(p)).collect();
        if missing.is_empty() {
            MailTest { ok: true, message: "The credential can manage domains, mailboxes and their passwords.".into(), edition, can_manage: true }
        } else {
            MailTest { ok: false, message: format!("Connected, but the credential lacks {}", missing.join(", ")), edition, can_manage: false }
        }
    }

    pub async fn domains(&self) -> Result<Vec<MailDomain>, String> {
        let calls = json!([
            ["x:Domain/query", {}, "q"],
            ["x:Domain/get", {"#ids": {"resultOf": "q", "name": "x:Domain/query", "path": "/ids"}, "properties": ["name", "dnsZoneFile"]}, "g"],
        ]);
        let responses = self.jmap(calls).await?;
        let list = responses.iter().find(|(name, _)| name == "x:Domain/get").and_then(|(_, args)| args.get("list")).and_then(Value::as_array).cloned().unwrap_or_default();
        let mut domains: Vec<MailDomain> = list
            .iter()
            .filter_map(|item| {
                Some(MailDomain {
                    id: item.get("id")?.as_str()?.to_string(),
                    name: item.get("name")?.as_str()?.to_ascii_lowercase(),
                    records: parse_zone(item.get("dnsZoneFile").and_then(Value::as_str).unwrap_or_default()),
                })
            })
            .collect();
        domains.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(domains)
    }

    pub async fn domain_id(&self, name: &str) -> Result<Option<String>, String> {
        let name = name.to_ascii_lowercase();
        Ok(self.domains().await?.into_iter().find(|domain| domain.name == name).map(|domain| domain.id))
    }

    pub async fn add_domain(&self, name: &str) -> Result<String, String> {
        let responses = self.jmap(json!([["x:Domain/set", {"create": {"d": {"name": name}}}, "s"]])).await?;
        created(&responses, "x:Domain/set", "d")
    }

    /// The account `local`@domain, when there is one.
    pub async fn find_account(&self, local: &str, domain_id: &str) -> Result<Option<String>, String> {
        let calls = json!([
            ["x:Account/query", {"filter": {"name": local, "domainId": domain_id}}, "q"],
            ["x:Account/get", {"#ids": {"resultOf": "q", "name": "x:Account/query", "path": "/ids"}, "properties": ["name", "domainId"]}, "g"],
        ]);
        let responses = self.jmap(calls).await?;
        let list = responses.iter().find(|(name, _)| name == "x:Account/get").and_then(|(_, args)| args.get("list")).and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(list
            .iter()
            .find(|item| {
                item.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(local))
                    && item.get("domainId").and_then(Value::as_str) == Some(domain_id)
            })
            .and_then(|item| item.get("id").and_then(Value::as_str).map(str::to_string)))
    }

    pub async fn create_user(&self, local: &str, domain_id: &str, description: &str, password: &str) -> Result<String, String> {
        let account = json!({
            "@type": "User",
            "name": local,
            "domainId": domain_id,
            "description": description,
            "credentials": {"0": {"@type": "Password", "secret": password}},
        });
        let responses = self.jmap(json!([["x:Account/set", {"create": {"u": account}}, "s"]])).await?;
        created(&responses, "x:Account/set", "u")
    }

    /// Make `password` the account's sign-in password (IMAP, SMTP, JMAP, the web UI).
    pub async fn set_password(&self, account_id: &str, password: &str) -> Result<(), String> {
        let responses = self.jmap(json!([["x:Account/get", {"ids": [account_id], "properties": ["credentials"]}, "g"]])).await?;
        let account = responses
            .iter()
            .find(|(name, _)| name == "x:Account/get")
            .and_then(|(_, args)| args.get("list"))
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .ok_or("the mailbox is not on the mail server any more")?;
        // Credentials are a map; the password is the entry of type Password (app passwords sit
        // beside it and are kept).
        let key = account
            .get("credentials")
            .and_then(Value::as_object)
            .and_then(|map| map.iter().find(|(_, c)| c.get("@type").and_then(Value::as_str) == Some("Password")).map(|(k, _)| k.clone()));
        let patch = match key {
            Some(key) => json!({ format!("credentials/{key}/secret"): password }),
            None => json!({ "credentials/p": {"@type": "Password", "secret": password} }),
        };
        let responses = self.jmap(json!([["x:Account/set", {"update": {account_id: patch}}, "s"]])).await?;
        let args = responses.iter().find(|(name, _)| name == "x:Account/set").map(|(_, args)| args).ok_or("no answer from the mail server")?;
        if let Some(err) = args.get("notUpdated").and_then(|m| m.get(account_id)) {
            return Err(set_error(err));
        }
        Ok(())
    }

    /// Suspend the account (`suspend`) or lift artcraftCloud's suspension. Stalwart has no
    /// enabled flag; a suspension disables permissions on the account, which beats every role.
    /// `authenticate` stops every sign-in (password, app passwords, API keys) and `emailSend`
    /// stops sending; mail keeps arriving. Returns the permissions this call disabled, so that
    /// lifting removes only those and keeps anything an admin disabled in Stalwart.
    pub async fn set_suspended(&self, account_id: &str, suspend: bool, added: &[String]) -> Result<Vec<String>, String> {
        let responses = self.jmap(json!([["x:Account/get", {"ids": [account_id], "properties": ["permissions"]}, "g"]])).await?;
        let permissions = responses
            .iter()
            .find(|(name, _)| name == "x:Account/get")
            .and_then(|(_, args)| args.get("list"))
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .map(|account| account.get("permissions").cloned().unwrap_or(Value::Null))
            .ok_or("the mailbox is not on the mail server any more")?;
        let mode = permissions.get("@type").and_then(Value::as_str).unwrap_or("Inherit").to_string();
        let disabled: Vec<String> = permissions.get("disabledPermissions").and_then(Value::as_object).map(|m| m.keys().cloned().collect()).unwrap_or_default();
        let enabled_empty = permissions.get("enabledPermissions").and_then(Value::as_object).is_none_or(|m| m.is_empty());
        let (patch, result) = if suspend {
            let add: Vec<String> = SUSPEND.iter().map(|p| p.to_string()).filter(|p| !disabled.contains(p)).collect();
            if add.is_empty() {
                return Ok(Vec::new());
            }
            let patch = if mode == "Inherit" {
                let list: serde_json::Map<String, Value> = add.iter().map(|p| (p.clone(), Value::Bool(true))).collect();
                json!({"permissions": {"@type": "Merge", "enabledPermissions": {}, "disabledPermissions": list}})
            } else {
                Value::Object(add.iter().map(|p| (format!("permissions/disabledPermissions/{p}"), Value::Bool(true))).collect())
            };
            (patch, add)
        } else {
            let remove: Vec<&String> = added.iter().filter(|p| disabled.contains(p)).collect();
            if remove.is_empty() {
                return Ok(Vec::new());
            }
            // Our own Merge with nothing else in it goes back to Inherit, as it was.
            let patch = if mode == "Merge" && enabled_empty && disabled.iter().all(|p| added.contains(p)) {
                json!({"permissions": {"@type": "Inherit"}})
            } else {
                Value::Object(remove.iter().map(|p| (format!("permissions/disabledPermissions/{p}"), Value::Null)).collect())
            };
            (patch, Vec::new())
        };
        let responses = self.jmap(json!([["x:Account/set", {"update": {account_id: patch}}, "s"]])).await?;
        let args = responses.iter().find(|(name, _)| name == "x:Account/set").map(|(_, args)| args).ok_or("no answer from the mail server")?;
        if let Some(err) = args.get("notUpdated").and_then(|m| m.get(account_id)) {
            return Err(set_error(err));
        }
        Ok(result)
    }

    /// A new app password of the account (needs `impersonate` and `sysAppPasswordCreate`):
    /// `(credential id, secret)`. The server shows the secret only now.
    pub async fn create_app_password(&self, account_id: &str, description: &str) -> Result<(String, String), String> {
        let body = json!({
            "using": USING,
            "methodCalls": [["x:AppPassword/set", {"accountId": account_id, "create": {"p": {"description": description, "permissions": {"@type": "Inherit"}, "allowedIps": {}}}}, "s"]],
        });
        let responses = self.jmap_body(body).await?;
        created_secret(&responses, "x:AppPassword/set", "p")
    }

    pub async fn destroy_app_password(&self, account_id: &str, id: &str) -> Result<(), String> {
        let body = json!({ "using": USING, "methodCalls": [["x:AppPassword/set", {"accountId": account_id, "destroy": [id]}, "s"]] });
        let responses = self.jmap_body(body).await?;
        let args = responses.iter().find(|(name, _)| name == "x:AppPassword/set").map(|(_, args)| args).ok_or("no answer")?;
        match args.get("notDestroyed").and_then(|m| m.get(id)) {
            Some(err) if err.get("type").and_then(Value::as_str) != Some("notFound") => Err(set_error(err)),
            _ => Ok(()),
        }
    }

    /// An API key on the signed-in (admin) account, with that account's permissions.
    pub async fn create_api_key(&self, description: &str) -> Result<String, String> {
        let responses = self.jmap(json!([["x:ApiKey/set", {"create": {"k": {"description": description, "permissions": {"@type": "Inherit"}, "allowedIps": {}}}}, "s"]])).await?;
        created_secret(&responses, "x:ApiKey/set", "k").map(|(_, secret)| secret)
    }

    pub async fn destroy_account(&self, id: &str) -> Result<(), String> {
        let responses = self.jmap(json!([["x:Account/set", {"destroy": [id]}, "s"]])).await?;
        let args = responses.iter().find(|(name, _)| name == "x:Account/set").map(|(_, args)| args).ok_or("no answer")?;
        if let Some(err) = args.get("notDestroyed").and_then(|m| m.get(id)) {
            if err.get("type").and_then(Value::as_str) == Some("notFound") {
                return Ok(());
            }
            return Err(set_error(err));
        }
        Ok(())
    }
}

/// What a suspension disables on the mail server.
pub const SUSPEND: [&str; 2] = ["authenticate", "emailSend"];

/// What the credential artcraftCloud uses must be allowed to do.
pub const NEEDED: [&str; 11] = [
    "sysDomainGet",
    "sysDomainQuery",
    "sysDomainCreate",
    "sysAccountGet",
    "sysAccountQuery",
    "sysAccountCreate",
    "sysAccountUpdate",
    "sysAccountDestroy",
    "impersonate",
    "sysAppPasswordCreate",
    "sysAppPasswordDestroy",
];

fn created_secret(responses: &[(String, Value)], method: &str, key: &str) -> Result<(String, String), String> {
    let args = responses.iter().find(|(name, _)| name == method).map(|(_, args)| args).ok_or("no answer from the mail server")?;
    if let Some(made) = args.get("created").and_then(|m| m.get(key)) {
        let id = made.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let secret = made.get("secret").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or("the mail server returned no secret")?;
        return Ok((id, secret.to_string()));
    }
    Err(args.get("notCreated").and_then(|m| m.get(key)).map(set_error).unwrap_or_else(|| "the mail server created nothing".into()))
}

fn reach(err: reqwest::Error) -> String {
    if err.is_timeout() {
        "the mail server did not answer in time".into()
    } else if err.is_connect() {
        "cannot reach the mail server at that address".into()
    } else {
        "the mail server could not be asked".into()
    }
}

fn created(responses: &[(String, Value)], method: &str, key: &str) -> Result<String, String> {
    let args = responses.iter().find(|(name, _)| name == method).map(|(_, args)| args).ok_or("no answer from the mail server")?;
    if let Some(id) = args.get("created").and_then(|m| m.get(key)).and_then(|c| c.get("id")).and_then(Value::as_str) {
        return Ok(id.to_string());
    }
    Err(args.get("notCreated").and_then(|m| m.get(key)).map(set_error).unwrap_or_else(|| "the mail server created nothing".into()))
}

fn set_error(err: &Value) -> String {
    let kind = err.get("type").and_then(Value::as_str).unwrap_or("error");
    let text = err.get("description").and_then(Value::as_str).unwrap_or("");
    match kind {
        "alreadyExists" | "primaryKeyViolation" => "it already exists on the mail server".into(),
        "forbidden" => "the admin credential may not do that".into(),
        _ => format!("the mail server refused it ({kind}{}{text})", if text.is_empty() { "" } else { ": " }),
    }
}

/// The records of a zone file as Stalwart writes it (`name IN TYPE value`, TXT strings quoted,
/// long ones split over lines in parentheses).
pub fn parse_zone(zone: &str) -> Vec<DnsRecord> {
    let mut joined = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for line in zone.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(line);
        depth += line.matches('(').count() as i32 - line.matches(')').count() as i32;
        if depth <= 0 {
            joined.push(std::mem::take(&mut current));
            depth = 0;
        }
    }
    if !current.is_empty() {
        joined.push(current);
    }
    let mut records = joined
        .iter()
        .filter_map(|record| {
            let record = record.replace(['(', ')'], " ");
            let mut parts = record.split_whitespace();
            let name = parts.next()?.trim_end_matches('.').to_string();
            let mut kind = parts.next()?.to_string();
            if kind.eq_ignore_ascii_case("IN") {
                kind = parts.next()?.to_string();
            }
            let rest: Vec<&str> = parts.collect();
            let rest = rest.join(" ");
            let value = if kind.eq_ignore_ascii_case("TXT") {
                rest.split('"').skip(1).step_by(2).collect::<Vec<_>>().concat()
            } else {
                rest.trim_end_matches('.').to_string()
            };
            let purpose = if kind.eq_ignore_ascii_case("MX") {
                "MX"
            } else if name.contains("._domainkey.") {
                "DKIM"
            } else if name.starts_with("_dmarc.") {
                "DMARC"
            } else if value.starts_with("v=spf1") {
                "SPF"
            } else {
                ""
            };
            Some(DnsRecord { name, kind: kind.to_ascii_uppercase(), value, purpose: purpose.into() })
        })
        .collect::<Vec<_>>();
    // The four that decide whether mail arrives and is trusted come first, in that order.
    let rank = |r: &DnsRecord| ["MX", "SPF", "DKIM", "DMARC"].iter().position(|p| *p == r.purpose).unwrap_or(4);
    records.sort_by_key(rank);
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalwart_zone_files_become_records() {
        let zone = "v1-rsa-20261124._domainkey.example.test. IN TXT (\n    \"v=DKIM1; k=rsa; p=MIIB\"\n    \"ltZM\"\n)\nexample.test. IN TXT \"v=spf1 mx -all\"\nexample.test. IN MX 10 mail.example.test.\n_dmarc.example.test. IN TXT \"v=DMARC1; p=reject\"\n_imaps._tcp.example.test. IN SRV 0 1 993 mail.example.test.\n";
        let records = parse_zone(zone);
        assert_eq!(records.len(), 5);
        assert_eq!(records.iter().map(|r| r.purpose.as_str()).collect::<Vec<_>>(), ["MX", "SPF", "DKIM", "DMARC", ""]);
        assert_eq!(records[2], DnsRecord { name: "v1-rsa-20261124._domainkey.example.test".into(), kind: "TXT".into(), value: "v=DKIM1; k=rsa; p=MIIBltZM".into(), purpose: "DKIM".into() });
        assert_eq!((records[1].purpose.as_str(), records[1].value.as_str()), ("SPF", "v=spf1 mx -all"));
        assert_eq!((records[0].kind.as_str(), records[0].value.as_str(), records[0].purpose.as_str()), ("MX", "10 mail.example.test", "MX"));
        assert_eq!(records[3].purpose, "DMARC");
        assert_eq!((records[4].kind.as_str(), records[4].purpose.as_str()), ("SRV", ""));
    }

    #[test]
    fn base_urls_are_checked() {
        assert_eq!(normalize_base(" http://stalwart:8080/ ").unwrap(), "http://stalwart:8080");
        assert!(normalize_base("ftp://x").is_err());
        assert!(normalize_base("http://u:p@x").is_err());
        assert!(normalize_base("nonsense").is_err());
    }
}
