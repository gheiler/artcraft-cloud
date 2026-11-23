//! Telling files apart by their content when the extension is not enough.
//!
//! LightCraft's web build saves its library backup as a plain `.zip`
//! (`library/catalog.snap`, `library/presets.json`, `originals/<hash>/<name>`, `README.txt`).
//! A zip says nothing by its name, so the drive asks this module which app a zip belongs to.
//! Only the zip directory is read (at most a few tens of KiB), never the photos inside.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use artcraft_cloud_types::FileKind;

/// Bytes read from the end of the file to find the end-of-central-directory record
/// (22 bytes plus a comment of at most 65 535 bytes).
const TAIL: u64 = 22 + 65_535;
/// Bytes of the central directory scanned for the names LightCraft writes first.
const DIRECTORY_SCAN: u64 = 64 * 1024;

pub fn kind_of(path: &Path) -> std::io::Result<FileKind> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    if !name.ends_with(".zip") {
        return Ok(FileKind::default());
    }
    let mut file = File::open(path)?;
    let names = zip_names(&mut file)?;
    Ok(classify_zip(&names))
}

/// Which app a zip with these entry names belongs to.
pub fn classify_zip(names: &[String]) -> FileKind {
    let lightcraft = names.iter().any(|n| n == "library/catalog.snap" || n == "library/catalog.log")
        || (names.iter().any(|n| n.starts_with("library/")) && names.iter().any(|n| n.starts_with("originals/")));
    if lightcraft {
        FileKind { app: Some("lightcraft".into()), label: Some("LightCraft library backup".into()) }
    } else {
        FileKind::default()
    }
}

/// Entry names from the start of the central directory. Empty when the file is not a zip.
fn zip_names(file: &mut File) -> std::io::Result<Vec<String>> {
    let len = file.metadata()?.len();
    if len < 22 {
        return Ok(Vec::new());
    }
    let tail_len = TAIL.min(len);
    file.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail)?;
    let Some(eocd) = (0..=tail.len() - 22).rev().find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06]) else {
        return Ok(Vec::new());
    };
    let dir_size = u32::from_le_bytes(tail[eocd + 12..eocd + 16].try_into().unwrap_or_default()) as u64;
    let dir_offset = u32::from_le_bytes(tail[eocd + 16..eocd + 20].try_into().unwrap_or_default()) as u64;
    if dir_offset >= len {
        return Ok(Vec::new());
    }
    let scan = dir_size.min(DIRECTORY_SCAN).min(len - dir_offset);
    file.seek(SeekFrom::Start(dir_offset))?;
    let mut dir = vec![0u8; scan as usize];
    file.read_exact(&mut dir)?;
    let mut names = Vec::new();
    let mut at = 0usize;
    while at + 46 <= dir.len() && dir[at..at + 4] == [0x50, 0x4b, 0x01, 0x02] {
        let name_len = u16::from_le_bytes([dir[at + 28], dir[at + 29]]) as usize;
        let extra = u16::from_le_bytes([dir[at + 30], dir[at + 31]]) as usize;
        let comment = u16::from_le_bytes([dir[at + 32], dir[at + 33]]) as usize;
        let start = at + 46;
        if start + name_len > dir.len() {
            break;
        }
        names.push(String::from_utf8_lossy(&dir[start..start + name_len]).into_owned());
        at = start + name_len + extra + comment;
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A stored (uncompressed) zip with these entries, the way LightCraft writes its backups.
    fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let offset = out.len() as u32;
            let mut local = vec![0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            local.extend_from_slice(&0u32.to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&local);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            let mut c = vec![0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            c.extend_from_slice(&0u32.to_le_bytes());
            c.extend_from_slice(&(data.len() as u32).to_le_bytes());
            c.extend_from_slice(&(data.len() as u32).to_le_bytes());
            c.extend_from_slice(&(name.len() as u16).to_le_bytes());
            c.extend_from_slice(&[0; 12]);
            c.extend_from_slice(&offset.to_le_bytes());
            c.extend_from_slice(name.as_bytes());
            central.extend_from_slice(&c);
        }
        let dir_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&dir_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn kind_of_bytes(name: &str, bytes: &[u8]) -> FileKind {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        File::create(&path).unwrap().write_all(bytes).unwrap();
        kind_of(&path).unwrap()
    }

    #[test]
    fn a_lightcraft_backup_is_recognised_by_its_entries() {
        let zip = stored_zip(&[("library/catalog.snap", b"snap"), ("library/presets.json", b"{}"), ("originals/ab12/IMG_1.jpg", b"jpg"), ("README.txt", b"hi")]);
        let kind = kind_of_bytes("lightcraft-library-backup-2026-11-23.zip", &zip);
        assert_eq!(kind.app.as_deref(), Some("lightcraft"));
    }

    #[test]
    fn other_zips_and_other_files_have_no_app() {
        let zip = stored_zip(&[("docs/a.txt", b"a"), ("b.png", b"b")]);
        assert_eq!(kind_of_bytes("archive.zip", &zip), FileKind::default());
        assert_eq!(kind_of_bytes("not-a-zip.zip", b"hello world, this is not a zip at all"), FileKind::default());
        assert_eq!(kind_of_bytes("notes.txt", &stored_zip(&[("library/catalog.snap", b"x")])), FileKind::default());
    }
}
