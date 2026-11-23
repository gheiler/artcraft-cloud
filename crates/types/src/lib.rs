//! Types shared by the server and the wasm shell, plus the catalog of Crafting
//! Apps whose released web builds speak the `craft-web-host` contract.

use serde::{Deserialize, Serialize};

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
    },
    CraftApp {
        id: "gridcraft",
        title: "GridCraft",
        repo: "gridcraft",
        blurb: "Spreadsheets",
        extensions: &["xlsx"],
        author_query: true,
        chrome_query: true,
    },
    CraftApp {
        id: "deckcraft",
        title: "DeckCraft",
        repo: "deckcraft",
        blurb: "Presentations",
        extensions: &["pptx", "potx", "ppsx", "deckcraft"],
        author_query: true,
        chrome_query: false,
    },
    CraftApp {
        id: "pdfcraft",
        title: "PdfCraft",
        repo: "pdfcraft",
        blurb: "PDF editor",
        extensions: &["pdf"],
        author_query: true,
        chrome_query: false,
    },
    CraftApp {
        id: "cadcraft",
        title: "CADCraft",
        repo: "cadcraft",
        blurb: "Drawings",
        extensions: &["dxf"],
        author_query: false,
        chrome_query: false,
    },
    CraftApp {
        id: "figcraft",
        title: "FigCraft",
        repo: "figcraft",
        blurb: "Interface design",
        extensions: &["figcraft", "fig"],
        author_query: true,
        chrome_query: false,
    },
    CraftApp {
        id: "lightcraft",
        title: "LightCraft",
        repo: "lightcraft",
        blurb: "Photo library and raw development",
        extensions: &["jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff", "dng"],
        author_query: false,
        chrome_query: false,
    },
    CraftApp {
        id: "filmcraft",
        title: "FilmCraft",
        repo: "filmcraft",
        blurb: "Video editor",
        extensions: &["fcproj"],
        author_query: false,
        chrome_query: false,
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
            assert!(!app.extensions.is_empty(), "{}", app.id);
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
