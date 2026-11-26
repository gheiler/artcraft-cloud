//! Types shared by the server and the wasm shell. The catalog of Crafting Apps is no longer built
//! in: it is the signed app registry (`registry` module, `registry/apps.json`).

use serde::{Deserialize, Serialize};

pub mod prefs;
pub mod registry;

/// Contract version of `craft-web-host`. Unknown versions are treated as this one.
pub const HOST_CONTRACT_VERSION: &str = "1";

/// A Crafting App as the shell keeps it: built from `/api/catalog` (the registry's apps that are
/// installed and enabled), with strings that live as long as the page.
#[derive(Debug, Clone, Copy)]
pub struct CraftApp {
    pub id: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
    pub extensions: &'static [&'static str],
    /// The app reads `?author=` for comments and tracked changes.
    pub author_query: bool,
    /// The app understands `?chrome=nofiletab` (ignored by the others).
    pub chrome_query: bool,
    /// The web build speaks the `craft-web-host` contract (opens and saves drive files). MailCraft
    /// does not: it is launched on its own and reaches mail over JMAP.
    pub bridge: bool,
}

impl CraftApp {
    /// Strings leaked to live as long as the page (the shell builds the catalog a few times per
    /// visit; a few kilobytes each).
    pub fn leak(id: &str, title: &str, blurb: &str, extensions: &[String], author_query: bool, chrome_query: bool, bridge: bool) -> CraftApp {
        let leak = |s: &str| -> &'static str { Box::leak(s.to_string().into_boxed_str()) };
        let extensions: Vec<&'static str> = extensions.iter().map(|e| leak(e)).collect();
        CraftApp { id: leak(id), title: leak(title), blurb: leak(blurb), extensions: Box::leak(extensions.into_boxed_slice()), author_query, chrome_query, bridge }
    }
}

/// The app that opens `name` by its extension, from `apps`.
pub fn app_for_name_in<'a>(apps: &'a [CraftApp], name: &str) -> Option<&'a CraftApp> {
    let ext = extension_of(name)?.to_ascii_lowercase();
    apps.iter().find(|app| app.extensions.iter().any(|candidate| *candidate == ext))
}

pub fn extension_of(name: &str) -> Option<&str> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, ext) = base.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    Some(ext)
}

/// `Report.docx` + 1 → `Report (copy).docx`. Further copies are numbered.
pub fn conflict_copy_name(name: &str, n: u32) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, ext) = match base.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => (stem, Some(ext)),
        _ => (base, None),
    };
    let label = if n <= 1 { "copy".to_string() } else { format!("copy {n}") };
    match ext {
        Some(ext) => format!("{stem} ({label}).{ext}"),
        None => format!("{stem} ({label})"),
    }
}

/// The revision prefix of an etag minted as `{rev}-{random hex}`.
pub fn rev_from_etag(etag: &str) -> i64 {
    etag.trim().trim_matches('"').split('-').next().and_then(|s| s.parse().ok()).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub disabled: bool,
    /// The main admin: the only account that can promote or demote admins.
    #[serde(default)]
    pub super_admin: bool,
    /// The user's email address (admins see it in People; everyone sees their own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// A mailbox on the configured mail server is linked to this user.
    #[serde(default)]
    pub mailbox: bool,
    /// Whether the mailbox's password is the user's artcraftCloud password: `synced`,
    /// `pending` (set at their next sign-in or password change) or `failed`. Empty without a mailbox.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mail_sync: String,
    /// The mailbox is suspended on the mail server (the person is disabled): no sign-in from any
    /// mail app, no sending; mail still arrives.
    #[serde(default)]
    pub mail_suspended: bool,
    /// Why the last suspend or restore did not reach the mail server (it is retried).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail_access_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordChange {
    pub current: String,
    pub next: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminPasswordReset {
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: Option<String>,
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    /// Create the mailbox on the mail server when the address's domain is served there.
    #[serde(default)]
    pub create_mailbox: bool,
}

/// `GET /api/mail/config` (super admin): the mail server connection, without its secret.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailConfig {
    /// Stalwart's base URL, e.g. `http://stalwart:8080`. Empty: not configured.
    pub base_url: String,
    /// `basic` (admin account name and password) or `bearer` (API key).
    pub auth: String,
    /// Admin account name for `basic`.
    #[serde(default)]
    pub username: String,
    /// Whether a password or API key is stored (it is never sent back).
    #[serde(default)]
    pub has_secret: bool,
    /// A base URL from the environment (`ARTCRAFT_MAIL_URL`) to offer when none is set.
    #[serde(default)]
    pub suggested_url: String,
    /// The mail host name people put in other mail apps (IMAP/SMTP); empty: the domain's MX.
    #[serde(default)]
    pub public_host: String,
}

/// `PUT /api/mail/config`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MailConfigUpdate {
    pub base_url: String,
    pub auth: String,
    #[serde(default)]
    pub username: String,
    /// New password or API key; `None` or empty keeps the stored one.
    #[serde(default)]
    pub secret: Option<String>,
    /// With `basic`: use the admin login once to create an API key for artcraftCloud, and store
    /// only the key.
    #[serde(default)]
    pub create_api_key: bool,
    #[serde(default)]
    pub public_host: String,
}

/// `POST /api/mail/test`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailTest {
    pub ok: bool,
    pub message: String,
    #[serde(default)]
    pub edition: String,
    /// The credential may list domains and create accounts.
    #[serde(default)]
    pub can_manage: bool,
}

/// A DNS record a mail domain needs, from the server's zone file.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DnsRecord {
    pub name: String,
    /// `MX`, `TXT`, `CNAME`, `SRV`…
    pub kind: String,
    pub value: String,
    /// `MX`, `SPF`, `DKIM`, `DMARC` or empty for the others.
    #[serde(default)]
    pub purpose: String,
}

/// `GET /api/mail/domains`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailDomain {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub records: Vec<DnsRecord>,
}

/// `POST /api/mail/domains`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NewMailDomain {
    pub name: String,
}

/// `PUT /api/users/{id}/email`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SetUserEmail {
    /// `None` or empty clears it (only while no mailbox is linked).
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub create_mailbox: bool,
}

/// `DELETE /api/users/{id}/mailbox`: unlink, or delete the mailbox on the server too.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoveMailbox {
    #[serde(default)]
    pub delete: bool,
    /// The mailbox address typed again, required to delete.
    #[serde(default)]
    pub confirm: String,
}

/// `GET /api/users/{id}/mail` and `GET /api/mail/me`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserMail {
    #[serde(default)]
    pub email: Option<String>,
    /// The linked mailbox address.
    #[serde(default)]
    pub mailbox: Option<String>,
    /// A mail server is configured.
    #[serde(default)]
    pub configured: bool,
    /// The email's domain is one the mail server serves.
    #[serde(default)]
    pub domain_served: bool,
    /// `synced`, `pending` or `failed` (see [`User::mail_sync`]); empty without a mailbox.
    #[serde(default)]
    pub password_sync: String,
    /// Why the last update of the mailbox password failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_error: Option<String>,
    /// How to set up another mail app (only in `GET /api/mail/me`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<MailClient>,
}

/// IMAP/SMTP settings for other mail apps. The password is the artcraftCloud password.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MailClient {
    /// Empty when the server cannot tell (no MX, no public host set).
    pub host: String,
    pub username: String,
    /// IMAP over TLS.
    pub imap_port: u16,
    /// SMTP submission over TLS.
    pub smtp_port: u16,
    /// SMTP submission with STARTTLS.
    pub smtp_starttls_port: u16,
}

/// `POST /api/me/password`, `POST /api/users/{id}/password`, `POST /api/users/{id}/disable` and
/// `/enable`: what happened to the mailbox.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PasswordChanged {
    /// `none` (no mailbox), `synced` or `failed`.
    pub mail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub etag: String,
    pub modified: i64,
    /// Monotonic revision from which `etag` is minted (`{rev}-…`).
    #[serde(default)]
    pub rev: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteResult {
    pub path: String,
    pub etag: String,
    pub size: u64,
    pub is_dir: bool,
    /// Monotonic revision from which `etag` is minted (`{rev}-…`).
    #[serde(default)]
    pub rev: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathBody {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameBody {
    pub path: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferBody {
    pub from: String,
    pub to_dir: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrashEntry {
    pub id: String,
    pub name: String,
    pub original_path: String,
    pub is_dir: bool,
    pub size: u64,
    pub deleted_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateShareRequest {
    pub path: String,
    /// Username of another account. Empty when `public` is set.
    pub grantee: Option<String>,
    /// `view` or `edit`.
    pub permission: String,
    pub public: bool,
    pub password: Option<String>,
    /// Unix seconds. `None` means the link does not expire.
    pub expires_at: Option<i64>,
    /// Name of a group to share with (instead of `grantee`).
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Share {
    pub id: i64,
    pub path: String,
    pub owner: String,
    pub grantee: Option<String>,
    pub permission: String,
    pub public: bool,
    pub token: Option<String>,
    pub has_password: bool,
    pub expires_at: Option<i64>,
    pub is_dir: bool,
    /// Set when the share is with a group rather than one person.
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlockRequest {
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicShareView {
    pub token: String,
    pub permission: String,
    pub needs_password: bool,
    pub expired: bool,
    pub is_dir: bool,
    pub name: String,
    pub listing: Option<DirListing>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppStatus {
    pub id: String,
    pub title: String,
    pub blurb: String,
    pub extensions: Vec<String>,
    pub installed: bool,
    pub version: Option<String>,
    pub previous: Option<String>,
    pub signature: Option<String>,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub bridge: bool,
    #[serde(default)]
    pub author_query: bool,
    #[serde(default)]
    pub chrome_query: bool,
}

/// `GET /api/admin/apps` (super admin): the registry, every app in it and what is installed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminApps {
    pub registry: RegistryStatus,
    /// The running artcraftCloud version, which versions are checked against.
    pub cloud_version: String,
    pub apps: Vec<AdminApp>,
    /// Installed versions the registry blocks, and what was done about them.
    pub alerts: Vec<AppAlert>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistryStatus {
    /// Where the registry is fetched from.
    pub url: String,
    /// What is in use: `network` (fetched this run), `cache` (fetched before), `embedded` (the copy
    /// built into this artcraftCloud) or `none`.
    pub source: String,
    pub serial: u64,
    pub issued: i64,
    pub expires: i64,
    pub expired: bool,
    /// The registry in use carries a valid signature (always true when one is in use).
    pub signature_ok: bool,
    /// A self-hosted registry with its own keys (`ARTCRAFT_REGISTRY_KEYS`).
    pub custom_keys: bool,
    pub last_fetch: Option<i64>,
    pub last_attempt: Option<i64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminApp {
    pub id: String,
    pub title: String,
    pub publisher: String,
    pub blurb: String,
    pub repo_url: String,
    pub in_registry: bool,
    pub installed: Option<String>,
    pub previous: Option<String>,
    /// The previous version is still on disk and allowed: Roll back is offered.
    pub can_roll_back: bool,
    /// `verified` (the app's own release signature), `registry` (checksum from the registry),
    /// `local` (installed by hand) or what an older artcraftCloud wrote.
    pub signature: Option<String>,
    /// The installed version: `ok`, `blocked`, `incompatible`, `unlisted`; `absent` when not installed.
    pub state: String,
    pub reason: Option<String>,
    /// Why the app is disabled (hidden and not served), when it is.
    pub disabled: Option<String>,
    pub auto_update: bool,
    /// Something done automatically that the admin should know about.
    pub notice: Option<String>,
    pub recommended: Option<String>,
    pub latest: Option<String>,
    /// The version Update installs, when it is newer than the installed one.
    pub update: Option<String>,
    pub versions: Vec<AdminVersion>,
    pub job: Option<AppJob>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminVersion {
    pub version: String,
    /// `ok`, `blocked` or `incompatible`.
    pub status: String,
    pub reason: Option<String>,
    pub installable: bool,
    pub recommended: bool,
    /// `ed25519`, `openpgp` or `none`.
    pub signed: String,
}

/// An install, update or rollback running (or just finished) on the server.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppJob {
    pub action: String,
    pub version: Option<String>,
    /// `downloading`, `verifying`, `installing`, `done` or `failed`.
    pub phase: String,
    pub percent: Option<u8>,
    pub error: Option<String>,
    pub done: bool,
    pub started: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppAlert {
    pub app: String,
    pub title: String,
    pub version: String,
    pub reason: String,
    pub security: bool,
    /// `rolled_back` (to `to`), `disabled`, or none (a warning only).
    pub action: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
}

/// `POST /api/admin/apps/{id}/install`: add or update; no version means the recommended one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppInstallRequest {
    #[serde(default)]
    pub version: Option<String>,
}

/// `PUT /api/admin/apps/{id}/settings`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppSettingsRequest {
    #[serde(default)]
    pub auto_update: Option<bool>,
    /// `true` disables the app (hidden, not served); `false` enables it again.
    #[serde(default)]
    pub disabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthBody {
    pub status: String,
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> registry::Registry {
        serde_json::from_str(include_str!("../../../registry/apps.json")).expect("registry/apps.json parses")
    }

    #[test]
    fn the_registry_snapshot_is_valid_and_extensions_belong_to_one_app() {
        let reg = snapshot();
        reg.validate().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for app in &reg.apps {
            // An app opens documents exactly when it speaks the host bridge (MailCraft opens none).
            assert_eq!(app.extensions.is_empty(), !app.bridge, "{}", app.id);
            for ext in &app.extensions {
                assert!(seen.insert(ext.clone()), "{ext} is claimed twice");
                assert_eq!(ext, &ext.to_ascii_lowercase());
            }
            assert!(app.target("0.1.0").is_some(), "{} has an installable version", app.id);
        }
    }

    #[test]
    fn names_pick_the_app() {
        let apps: Vec<CraftApp> = snapshot().apps.iter().map(|a| CraftApp::leak(&a.id, &a.title, &a.blurb, &a.extensions, a.author_query, a.chrome_query, a.bridge)).collect();
        let app_for_name = |name: &str| app_for_name_in(&apps, name).map(|a| a.id);
        assert_eq!(app_for_name("Notes.DOCX"), Some("wordcraft"));
        assert_eq!(app_for_name("sheet.xlsx"), Some("gridcraft"));
        assert_eq!(app_for_name("deck.deckcraft"), Some("deckcraft"));
        assert_eq!(app_for_name("a.pdf"), Some("pdfcraft"));
        assert_eq!(app_for_name("plan.dxf"), Some("cadcraft"));
        assert_eq!(app_for_name("ui.figcraft"), Some("figcraft"));
        assert_eq!(app_for_name("photo.JPEG"), Some("lightcraft"));
        assert_eq!(app_for_name("cut.fcproj"), Some("filmcraft"));
        assert_eq!(app_for_name("Holidays.ICS"), Some("calendarcraft"));
        assert!(app_for_name("archive.zip").is_none());
        assert!(app_for_name(".docx").is_none());
    }

    #[test]
    fn conflict_names_keep_the_extension() {
        assert_eq!(conflict_copy_name("Report.docx", 1), "Report (copy).docx");
        assert_eq!(conflict_copy_name("dir/Report.docx", 2), "Report (copy 2).docx");
        assert_eq!(conflict_copy_name("README", 1), "README (copy)");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetRoleRequest {
    /// `admin` or `user`.
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Group {
    pub id: i64,
    pub name: String,
    /// Usernames of the members.
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateGroupRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMemberRequest {
    pub username: String,
}

/// Who a file can be shared with: every other enabled account, and every group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Directory {
    pub users: Vec<DirectoryUser>,
    pub groups: Vec<DirectoryGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryUser {
    pub username: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryGroup {
    pub name: String,
    pub members: usize,
}

/// What `GET /api/files/kind` found out about a file from its content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FileKind {
    /// App id that should open it, when the content says so (for example a LightCraft library backup zip).
    pub app: Option<String>,
    /// Short label of what the file is, for example `LightCraft library backup`.
    pub label: Option<String>,
}
