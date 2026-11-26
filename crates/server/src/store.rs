use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use artcraft_cloud_types::FileEntry;

use crate::db::{self, Db};
use crate::error::Error;
use crate::paths::{self, contained_existing, contained_parent, mint_etag, modified_secs, single_name, RelPath};

#[derive(Clone)]
pub struct Store {
    pub db: Db,
    pub files_root: PathBuf,
    pub trash_root: PathBuf,
    pub tmp_root: PathBuf,
}

impl Store {
    pub fn home(&self, owner: i64) -> PathBuf {
        self.files_root.join(owner.to_string())
    }

    pub fn ensure_home(&self, owner: i64) -> Result<PathBuf, Error> {
        let path = self.home(owner);
        fs::create_dir_all(&path)?;
        let _ = self.db.ensure_etag(owner, "", true)?;
        Ok(path)
    }

    pub fn list(&self, owner: i64, rel: &RelPath) -> Result<Vec<FileEntry>, Error> {
        let root = self.ensure_home(owner)?;
        let (dir, meta) = contained_existing(&root, rel)?;
        if !meta.is_dir() {
            return Err(Error::Invalid("not a folder".into()));
        }
        let mut entries = Vec::new();
        for item in fs::read_dir(&dir)? {
            let item = item?;
            let name = item.file_name().to_string_lossy().to_string();
            if name.ends_with(".artcraft-partial") {
                continue;
            }
            let meta = item.metadata()?;
            if meta.file_type().is_symlink() {
                continue;
            }
            let child = rel.join_name(&name)?;
            let is_dir = meta.is_dir();
            let size = if is_dir { 0 } else { meta.len() };
            let etag = self.db.ensure_etag(owner, child.as_str(), is_dir)?;
            entries.push(FileEntry {
                name,
                path: child.as_str().to_string(),
                is_dir,
                size,
                etag: etag.clone(),
                modified: modified_secs(&meta),
                rev: artcraft_cloud_types::rev_from_etag(&etag),
            });
        }
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(entries)
    }

    pub fn stat(&self, owner: i64, rel: &RelPath) -> Result<FileEntry, Error> {
        let root = self.ensure_home(owner)?;
        let (_path, meta) = contained_existing(&root, rel)?;
        let is_dir = meta.is_dir();
        let etag = if rel.is_root() {
            self.db.ensure_etag(owner, "", true)?
        } else {
            self.db.ensure_etag(owner, rel.as_str(), is_dir)?
        };
        Ok(FileEntry {
            name: rel.name().unwrap_or("").to_string(),
            path: rel.as_str().to_string(),
            is_dir,
            size: if is_dir { 0 } else { meta.len() },
            etag: etag.clone(),
            modified: modified_secs(&meta),
            rev: artcraft_cloud_types::rev_from_etag(&etag),
        })
    }

    pub fn open_path(&self, owner: i64, rel: &RelPath) -> Result<(PathBuf, FileEntry), Error> {
        let entry = self.stat(owner, rel)?;
        if entry.is_dir {
            return Err(Error::Invalid("not a file".into()));
        }
        let root = self.ensure_home(owner)?;
        let (path, _) = contained_existing(&root, rel)?;
        Ok((path, entry))
    }

    pub fn mkdir(&self, owner: i64, rel: &RelPath) -> Result<FileEntry, Error> {
        if rel.is_root() {
            return Err(Error::BadPath);
        }
        let root = self.ensure_home(owner)?;
        let dest = contained_parent(&root, rel)?;
        if dest.exists() {
            return Err(Error::Exists);
        }
        fs::create_dir(&dest).map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                Error::NotFound
            } else {
                Error::Io(err)
            }
        })?;
        let etag = self.db.next_file_etag(owner, rel.as_str(), true)?;
        Ok(entry_of(rel, true, 0, &etag))
    }

    pub fn write_file(&self, owner: i64, rel: &RelPath, tmp: &Path, if_match: Option<&str>) -> Result<FileEntry, Error> {
        if rel.is_root() {
            return Err(Error::BadPath);
        }
        let root = self.ensure_home(owner)?;
        let dest = contained_parent(&root, rel)?;
        let size = fs::metadata(tmp)?.len();
        let path_key = rel.as_str().to_string();
        let etag = self.db.with_conn(|conn| {
            let current = db::read_meta(conn, owner, &path_key)?;
            if dest.is_dir() {
                return Err(Error::Exists);
            }
            if dest.is_file() {
                let current_etag = current.as_ref().map(|m| m.etag.clone()).unwrap_or_default();
                match if_match {
                    Some(header) if paths::etag_matches(header, &current_etag) => {}
                    _ => return Err(Error::Conflict { etag: current_etag }),
                }
            } else if dest.exists() {
                return Err(Error::Exists);
            }
            fs::rename(tmp, &dest)?;
            let rev = current.map(|m| m.rev.saturating_add(1)).unwrap_or(1);
            let etag = mint_etag(rev);
            db::write_meta(conn, owner, &path_key, false, rev, &etag)?;
            Ok(etag)
        })?;
        Ok(entry_of(rel, false, size, &etag))
    }

    pub fn rename(&self, owner: i64, from: &RelPath, new_name: &str) -> Result<FileEntry, Error> {
        let name = single_name(new_name)?;
        let dest = from.parent().join_name(&name)?;
        self.move_to(owner, from, &dest)
    }

    pub fn move_into(&self, owner: i64, from: &RelPath, to_dir: &RelPath, name: Option<&str>) -> Result<FileEntry, Error> {
        let name = match name {
            Some(name) => single_name(name)?,
            None => from.name().ok_or(Error::BadPath)?.to_string(),
        };
        let dest = to_dir.join_name(&name)?;
        self.move_to(owner, from, &dest)
    }

    fn move_to(&self, owner: i64, from: &RelPath, dest: &RelPath) -> Result<FileEntry, Error> {
        if from.is_root() || dest.is_root() {
            return Err(Error::BadPath);
        }
        if dest.starts_with(from) {
            return Err(Error::Invalid("cannot move a folder into itself".into()));
        }
        let root = self.ensure_home(owner)?;
        let (src, meta) = contained_existing(&root, from)?;
        let dst = contained_parent(&root, dest)?;
        if dst.exists() {
            return Err(Error::Exists);
        }
        fs::rename(&src, &dst)?;
        self.db.rewrite_meta_prefix(owner, from.as_str(), dest.as_str())?;
        self.db.rewrite_share_prefix(owner, from.as_str(), dest.as_str())?;
        let etag = self.db.ensure_etag(owner, dest.as_str(), meta.is_dir())?;
        let size = if meta.is_dir() { 0 } else { meta.len() };
        Ok(entry_of(dest, meta.is_dir(), size, &etag))
    }

    pub fn copy(&self, owner: i64, from: &RelPath, to_dir: &RelPath, name: Option<&str>) -> Result<FileEntry, Error> {
        if from.is_root() {
            return Err(Error::BadPath);
        }
        let name = match name {
            Some(name) => single_name(name)?,
            None => from.name().ok_or(Error::BadPath)?.to_string(),
        };
        let dest = to_dir.join_name(&name)?;
        if dest.starts_with(from) && dest.as_str() != from.as_str() {
            return Err(Error::Invalid("cannot copy a folder into itself".into()));
        }
        let root = self.ensure_home(owner)?;
        let (src, meta) = contained_existing(&root, from)?;
        let dst = contained_parent(&root, &dest)?;
        if dst.exists() {
            return Err(Error::Exists);
        }
        copy_tree(&src, &dst)?;
        index_new_tree(&self.db, owner, &dest, &dst)?;
        let etag = self.db.ensure_etag(owner, dest.as_str(), meta.is_dir())?;
        let size = if meta.is_dir() { 0 } else { fs::metadata(&dst)?.len() };
        Ok(entry_of(&dest, meta.is_dir(), size, &etag))
    }

    pub fn trash(&self, owner: i64, rel: &RelPath) -> Result<(), Error> {
        if rel.is_root() {
            return Err(Error::BadPath);
        }
        let root = self.ensure_home(owner)?;
        let (src, meta) = contained_existing(&root, rel)?;
        let id = paths::random_id();
        let bucket = self.trash_root.join(owner.to_string()).join(&id);
        fs::create_dir_all(&bucket)?;
        let name = rel.name().ok_or(Error::BadPath)?;
        fs::rename(&src, bucket.join(name))?;
        let size = if meta.is_dir() { tree_size(&bucket) } else { meta.len() };
        self.db.insert_trash(&id, owner, rel.as_str(), name, meta.is_dir(), size)?;
        self.db.delete_meta_tree(owner, rel.as_str())?;
        self.db.delete_share_tree(owner, rel.as_str())?;
        Ok(())
    }

    pub fn restore(&self, owner: i64, id: &str) -> Result<FileEntry, Error> {
        let row = self.db.trash(owner, id)?.ok_or(Error::NotFound)?;
        let rel = RelPath::parse(&row.original_path)?;
        let root = self.ensure_home(owner)?;
        if !rel.parent().is_root() {
            let parent = root.join(rel.parent().to_path());
            fs::create_dir_all(&parent)?;
        }
        let dest = contained_parent(&root, &rel)?;
        if dest.exists() {
            return Err(Error::Exists);
        }
        let src = self.trash_root.join(owner.to_string()).join(id).join(&row.name);
        fs::rename(&src, &dest)?;
        let _ = fs::remove_dir_all(self.trash_root.join(owner.to_string()).join(id));
        self.db.delete_trash(owner, id)?;
        index_new_tree(&self.db, owner, &rel, &dest)?;
        let etag = self.db.ensure_etag(owner, rel.as_str(), row.is_dir)?;
        Ok(entry_of(&rel, row.is_dir, row.size, &etag))
    }

    pub fn purge(&self, owner: i64, id: &str) -> Result<(), Error> {
        let row = self.db.trash(owner, id)?.ok_or(Error::NotFound)?;
        let _ = row;
        let dir = self.trash_root.join(owner.to_string()).join(id);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        self.db.delete_trash(owner, id)?;
        Ok(())
    }

    pub fn empty_trash(&self, owner: i64) -> Result<(), Error> {
        for id in self.db.trash_ids(owner)? {
            self.purge(owner, &id)?;
        }
        Ok(())
    }
}

fn entry_of(rel: &RelPath, is_dir: bool, size: u64, etag: &str) -> FileEntry {
    FileEntry {
        name: rel.name().unwrap_or("").to_string(),
        path: rel.as_str().to_string(),
        is_dir,
        size,
        etag: etag.to_string(),
        modified: paths::now_secs(),
        rev: artcraft_cloud_types::rev_from_etag(etag),
    }
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), Error> {
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        return Err(Error::BadPath);
    }
    if meta.is_dir() {
        fs::create_dir(dst)?;
        for item in fs::read_dir(src)? {
            let item = item?;
            copy_tree(&item.path(), &dst.join(item.file_name()))?;
        }
    } else {
        fs::copy(src, dst)?;
    }
    Ok(())
}

fn index_new_tree(db: &Db, owner: i64, rel: &RelPath, disk: &Path) -> Result<(), Error> {
    let meta = fs::symlink_metadata(disk)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let _ = db.next_file_etag(owner, rel.as_str(), meta.is_dir())?;
    if meta.is_dir() {
        for item in fs::read_dir(disk)? {
            let item = item?;
            let name = item.file_name().to_string_lossy().to_string();
            let child = rel.join_name(&name)?;
            index_new_tree(db, owner, &child, &item.path())?;
        }
    }
    Ok(())
}

fn tree_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&current) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if let Ok(rd) = fs::read_dir(&current) {
                for item in rd.flatten() {
                    stack.push(item.path());
                }
            }
        } else {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

