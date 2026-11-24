//! Types shared by the server and the wasm shell, plus the catalog of Crafting
//! Apps whose released web builds speak the `craft-web-host` contract.

use serde::{Deserialize, Serialize};

pub mod prefs;

/// Contract version of `craft-web-host`. Unknown versions are treated as this one.
pub const HOST_CONTRACT_VERSION: &str = "1";

/// A Crafting App whose web build can be framed with `?host=parent`.
#[derive(Debug, Clone, Copy)]
pub struct CraftApp {
    pub id: &'static str,
    pub title: &'static str,
    pub repo: &'static str,
    pub blurb: &'static str,
    pub extensions: &'static [&'static str],
    /// The app reads `?author=` for comments and tracked changes.
    pub author_query: bool,
    /// The app understands `?chrome=nofiletab` (ignored by the others).
    pub chrome_query: bool,
    /// GitHub owner of `repo`.
    pub owner: &'static str,
    /// The web build speaks the `craft-web-host` contract (opens and saves drive files). MailCraft
    /// does not: it is launched on its own and reaches mail over JMAP.
    pub bridge: bool,
    /// ed25519 public keys (base64) that sign the release's `SHA256SUMS.txt` as
    /// `SHA256SUMS.txt.sig`. Empty: the release is checked with its OpenPGP `.asc`, if any.
    pub release_keys: &'static [&'static str],
}

/// Apps verified against the latest GitHub release tag (November 2026).
/// PhotoCraft, VectorCraft, DesignCraft and the rest are intentionally absent:
/// their released web builds do not include the host bridge.
pub static CATALOG: &[CraftApp] = &[
    CraftApp {
        id: "wordcraft",
        title: "WordCraft",
        repo: "wordcraft",
        blurb: "Word processor",
        extensions: &["docx", "odt", "rtf", "txt", "md", "markdown"],
        author_query: true,
        chrome_query: false,
        owner: "storytold",
        bridge: true,
        release_keys: &[],
    },
    CraftApp {
        id: "gridcraft",
        title: "GridCraft",
        repo: "gridcraft",
        blurb: "Spreadsheets",
        extensions: &["xlsx"],
        author_query: true,
        chrome_query: true,
        owner: "storytold",
        bridge: true,
        release_keys: &[],
    },
    CraftApp {
        id: "deckcraft",
        title: "DeckCraft",
        repo: "deckcraft",
        blurb: "Presentations",
        extensions: &["pptx", "potx", "ppsx", "deckcraft"],
        author_query: true,
        chrome_query: false,
        owner: "storytold",
        bridge: true,
        release_keys: &[],
    },
    CraftApp {
        id: "pdfcraft",
        title: "PdfCraft",
        repo: "pdfcraft",
        blurb: "PDF editor",
        extensions: &["pdf"],
        author_query: true,
        chrome_query: false,
        owner: "storytold",
        bridge: true,
        release_keys: &[],
    },
    CraftApp {
        id: "cadcraft",
        title: "CADCraft",
        repo: "cadcraft",
        blurb: "Drawings",
        extensions: &["dxf"],
        author_query: false,
        chrome_query: false,
        owner: "storytold",
        bridge: true,
        release_keys: &[],
    },
    CraftApp {
        id: "figcraft",
        title: "FigCraft",
        repo: "figcraft",
        blurb: "Interface design",
        extensions: &["figcraft", "fig"],
        author_query: true,
        chrome_query: false,
        owner: "storytold",
        bridge: true,
        release_keys: &[],
    },
    CraftApp {
        id: "lightcraft",
        title: "LightCraft",
        repo: "lightcraft",
        blurb: "Photo library and raw development",
        extensions: &["jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff", "dng"],
        author_query: false,
        chrome_query: false,
        owner: "storytold",
        bridge: true,
        release_keys: &[],
    },
    CraftApp {
        id: "filmcraft",
        title: "FilmCraft",
        repo: "filmcraft",
        blurb: "Video editor",
        extensions: &["fcproj"],
        author_query: false,
        chrome_query: false,
        owner: "storytold",
        bridge: true,
        release_keys: &[],
    },
    CraftApp {
        id: "mailcraft",
        title: "MailCraft",
        repo: "mailcraft",
        blurb: "Mail, calendar and contacts",
        extensions: &[],
        author_query: false,
        chrome_query: false,
        owner: "UCS-Care-Group",
        bridge: false,
        // `RELEASE_KEYS` in MailCraft's crates/net/src/update.rs: the key in use and the spare.
        release_keys: &["Mu5RF5oJpFhDGfa5wpgznC39tDC8vbUbyNkiAMdluPQ=", "XV6WRgW06/mGhZCHhFVKMIEZDmLevQduZPLSj/UW5zI="],
    },
];

pub fn catalog() -> &'static [CraftApp] {
    CATALOG
}

pub fn app_by_id(id: &str) -> Option<&'static CraftApp> {
    CATALOG.iter().find(|app| app.id == id)
}

pub fn app_for_name(name: &str) -> Option<&'static CraftApp> {
    let ext = extension_of(name)?.to_ascii_lowercase();
    CATALOG
        .iter()
        .find(|app| app.extensions.iter().any(|candidate| *candidate == ext))
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

/// `POST /api/me/password` and `POST /api/users/{id}/password`: what happened to the mailbox.
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppStatus {
    pub id: String,
    pub title: String,
    pub blurb: String,
    pub extensions: Vec<String>,
    pub installed: bool,
    pub version: Option<String>,
    pub previous: Option<String>,
    pub signature: Option<String>,
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

    #[test]
    fn extensions_belong_to_one_app() {
        let mut seen = std::collections::BTreeSet::new();
        for app in catalog() {
            // An app opens documents exactly when it speaks the host bridge (MailCraft opens none).
            assert_eq!(app.extensions.is_empty(), !app.bridge, "{}", app.id);
            for ext in app.extensions {
                assert!(seen.insert(*ext), "{ext} is claimed twice");
                assert_eq!(ext, &ext.to_ascii_lowercase());
            }
        }
    }

    #[test]
    fn names_pick_the_app() {
        assert_eq!(app_for_name("Notes.DOCX").map(|a| a.id), Some("wordcraft"));
        assert_eq!(app_for_name("sheet.xlsx").map(|a| a.id), Some("gridcraft"));
        assert_eq!(app_for_name("deck.deckcraft").map(|a| a.id), Some("deckcraft"));
        assert_eq!(app_for_name("a.pdf").map(|a| a.id), Some("pdfcraft"));
        assert_eq!(app_for_name("plan.dxf").map(|a| a.id), Some("cadcraft"));
        assert_eq!(app_for_name("ui.figcraft").map(|a| a.id), Some("figcraft"));
        assert_eq!(app_for_name("photo.JPEG").map(|a| a.id), Some("lightcraft"));
        assert_eq!(app_for_name("cut.fcproj").map(|a| a.id), Some("filmcraft"));
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
