use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use rand::RngCore;

use crate::error::Error;

/// A relative path inside one user's home. Empty is the home itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelPath {
    inner: String,
}

impl RelPath {
    pub fn root() -> Self {
        Self { inner: String::new() }
    }

    pub fn parse(input: &str) -> Result<Self, Error> {
        let input = input.trim().trim_matches('/');
        if input.is_empty() {
            return Ok(Self::root());
        }
        if input.len() > 1024 || input.contains('\0') {
            return Err(Error::BadPath);
        }
        let mut parts = Vec::new();
        for part in input.split(['/', '\\']) {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." || part.contains('\0') || part.len() > 255 || part.chars().any(|c| c.is_control()) {
                return Err(Error::BadPath);
            }
            parts.push(part);
        }
        if parts.is_empty() {
            return Ok(Self::root());
        }
        Ok(Self { inner: parts.join("/") })
    }

    pub fn is_root(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.inner
    }

    pub fn name(&self) -> Option<&str> {
        if self.inner.is_empty() {
            None
        } else {
            self.inner.rsplit('/').next()
        }
    }

    pub fn parent(&self) -> Self {
        match self.inner.rfind('/') {
            Some(index) => Self { inner: self.inner[..index].to_string() },
            None => Self::root(),
        }
    }

    pub fn join_name(&self, name: &str) -> Result<Self, Error> {
        let name = single_name(name)?;
        if self.inner.is_empty() {
            Ok(Self { inner: name })
        } else {
            Ok(Self { inner: format!("{}/{name}", self.inner) })
        }
    }

    pub fn join_rel(&self, child: &RelPath) -> Result<Self, Error> {
        if child.is_root() {
            return Ok(self.clone());
        }
        if self.is_root() {
            return Ok(child.clone());
        }
        let inner = format!("{}/{}", self.inner, child.inner);
        if inner.len() > 1024 {
            return Err(Error::BadPath);
        }
        Ok(Self { inner })
    }

    pub fn starts_with(&self, prefix: &RelPath) -> bool {
        if prefix.is_root() {
            return true;
        }
        self.inner == prefix.inner || self.inner.starts_with(&(prefix.inner.clone() + "/"))
    }

    pub fn to_path(&self) -> PathBuf {
        let mut path = PathBuf::new();
        if !self.inner.is_empty() {
            for part in self.inner.split('/') {
                path.push(part);
            }
        }
        path
    }
}

pub fn single_name(name: &str) -> Result<String, Error> {
    let parsed = RelPath::parse(name)?;
    if parsed.is_root() || parsed.as_str().contains('/') {
        return Err(Error::BadPath);
    }
    Ok(parsed.as_str().to_string())
}

pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

pub fn random_id() -> String {
    random_token(16)
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(data))
}

pub fn mint_etag(rev: i64) -> String {
    let mut bytes = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{rev}-{}", hex::encode(bytes))
}

pub fn quote_etag(etag: &str) -> String {
    format!("\"{etag}\"")
}

pub fn etag_matches(header: &str, current: &str) -> bool {
    header.split(',').any(|part| {
        let part = part.trim();
        if part == "*" {
            return true;
        }
        let part = part.strip_prefix("W/").unwrap_or(part).trim().trim_matches('"');
        part == current
    })
}

pub fn normalize_username(input: &str) -> Result<String, Error> {
    let name = input.trim().to_lowercase();
    if !(1..=64).contains(&name.len()) {
        return Err(Error::Invalid("username must be 1-64 characters".into()));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(Error::Invalid("username may contain letters, numbers, dot, underscore and hyphen".into()));
    }
    Ok(name)
}

pub fn check_password_policy(password: &str) -> Result<(), Error> {
    if password.len() < 8 || password.len() > 1024 {
        return Err(Error::Invalid("password must be 8 to 1024 characters".into()));
    }
    Ok(())
}

pub fn display_name_of(input: Option<&str>, username: &str) -> Result<String, Error> {
    let name = input.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(username);
    if name.chars().count() > 200 || name.chars().any(|c| c.is_control()) {
        return Err(Error::Invalid("display name is invalid".into()));
    }
    Ok(name.to_string())
}

pub fn modified_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn contained_parent(root: &Path, rel: &RelPath) -> Result<PathBuf, Error> {
    if rel.is_root() {
        return Err(Error::BadPath);
    }
    let root = root.canonicalize().map_err(|_| Error::NotFound)?;
    let parent = root.join(rel.parent().to_path());
    let meta = std::fs::symlink_metadata(&parent).map_err(|_| Error::NotFound)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(Error::NotFound);
    }
    let parent = parent.canonicalize().map_err(|_| Error::NotFound)?;
    if !parent.starts_with(&root) {
        return Err(Error::BadPath);
    }
    Ok(parent.join(rel.name().ok_or(Error::BadPath)?))
}

pub fn contained_existing(root: &Path, rel: &RelPath) -> Result<(PathBuf, std::fs::Metadata), Error> {
    let root = root.canonicalize().map_err(|_| Error::NotFound)?;
    if rel.is_root() {
        let meta = std::fs::symlink_metadata(&root)?;
        return Ok((root, meta));
    }
    let candidate = root.join(rel.to_path());
    let meta = match std::fs::symlink_metadata(&candidate) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Err(Error::NotFound),
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_symlink() {
        return Err(Error::BadPath);
    }
    let real = candidate.canonicalize().map_err(|_| Error::NotFound)?;
    if !real.starts_with(&root) {
        return Err(Error::BadPath);
    }
    Ok((real, meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_reject_escape() {
        assert!(RelPath::parse("..").is_err());
        assert!(RelPath::parse("a/../../b").is_err());
        assert!(RelPath::parse("a/../b").is_err());
        assert_eq!(RelPath::parse("/docs/a.docx").unwrap().as_str(), "docs/a.docx");
        assert_eq!(RelPath::parse("docs//a").unwrap().as_str(), "docs/a");
        assert!(RelPath::parse("").unwrap().is_root());
        assert_eq!(RelPath::parse("docs").unwrap().join_name("a.docx").unwrap().as_str(), "docs/a.docx");
    }

    #[test]
    fn etags_compare_quoted_and_lists() {
        assert!(etag_matches("\"3-abcd\"", "3-abcd"));
        assert!(etag_matches("W/\"3-abcd\"", "3-abcd"));
        assert!(etag_matches("\"no\", \"3-abcd\"", "3-abcd"));
        assert!(!etag_matches("\"other\"", "3-abcd"));
        assert!(etag_matches("*", "3-abcd"));
    }
}
