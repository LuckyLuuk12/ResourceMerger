//! Resource merger library
//!
//! Exposes a small API to merge multiple resource packs (directories, zip bytes, or zip files)
//! into a single zip where later packs overwrite earlier ones.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
// ...existing code...
use thiserror::Error;
use walkdir::WalkDir;
use zip::{ZipArchive, ZipWriter};

#[derive(Error, Debug)]
pub enum MergeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

pub type Result<T> = std::result::Result<T, MergeError>;

/// How to handle multiple inputs that contain the same internal path.
#[derive(Debug, Clone, Copy)]
pub enum OverwritePolicy {
    LastWins,
    FirstWins,
    ErrorIfConflict,
    SkipIfExists,
}

impl std::str::FromStr for OverwritePolicy {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "last" | "lastwins" | "last_wins" => Ok(OverwritePolicy::LastWins),
            "first" | "firstwins" | "first_wins" => Ok(OverwritePolicy::FirstWins),
            "error" | "errorifconflict" | "error_if_conflict" => {
                Ok(OverwritePolicy::ErrorIfConflict)
            }
            "skip" | "skipifexists" | "skip_if_exists" => Ok(OverwritePolicy::SkipIfExists),
            other => Err(format!("unknown overwrite policy: {}", other)),
        }
    }
}

/// Options that control merge behavior. New fields can be added as the library expands.
#[derive(Debug, Clone)]
pub struct MergeOptions {
    pub overwrite: OverwritePolicy,
    pub dry_run: bool,
    pub buffer_size: usize,
    pub atomic: bool,
    pub preserve_timestamps: bool,
}

impl Default for MergeOptions {
    fn default() -> Self {
        MergeOptions {
            overwrite: OverwritePolicy::LastWins,
            dry_run: false,
            buffer_size: 32 * 1024,
            atomic: true,
            preserve_timestamps: false,
        }
    }
}

/// Represents an input pack. It can be a directory on disk, a zip file on disk, or raw zip bytes.
pub enum PackInput {
    Dir(PathBuf),
    ZipFile(PathBuf),
    ZipBytes(Vec<u8>),
    Url(String),
}

impl From<PathBuf> for PackInput {
    fn from(p: PathBuf) -> Self {
        if p.is_dir() {
            PackInput::Dir(p)
        } else {
            PackInput::ZipFile(p)
        }
    }
}

impl From<Vec<u8>> for PackInput {
    fn from(b: Vec<u8>) -> Self {
        PackInput::ZipBytes(b)
    }
}

impl From<String> for PackInput {
    fn from(s: String) -> Self {
        // treat http/https as urls, otherwise as path
        if s.starts_with("http://") || s.starts_with("https://") {
            PackInput::Url(s)
        } else {
            PackInput::ZipFile(PathBuf::from(s))
        }
    }
}

/// Download a URL and return bytes (blocking reqwest). Caller should handle large bodies.
fn fetch_url_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = reqwest::blocking::get(url)
        .map_err(|e| MergeError::InvalidInput(format!("failed to GET {}: {}", url, e)))?;
    if !resp.status().is_success() {
        return Err(MergeError::InvalidInput(format!(
            "GET {} returned {}",
            url,
            resp.status()
        )));
    }
    let bytes = resp
        .bytes()
        .map_err(|e| MergeError::InvalidInput(format!("read {} body: {}", url, e)))?;
    Ok(bytes.to_vec())
}

/// Merge multiple packs into a single zip archive (returned as Vec<u8>).
///
/// The order of `packs` matters: earlier packs form the base, later packs overwrite files with the
/// same path.
pub fn merge_packs_to_bytes(packs: &[PackInput]) -> Result<Vec<u8>> {
    // We'll maintain a map of path -> file bytes. Later packs overwrite earlier ones.
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();

    for pack in packs {
        match pack {
            PackInput::Dir(p) => read_dir_into_map(p, &mut files)?,
            PackInput::ZipFile(p) => read_zipfile_into_map(p, &mut files)?,
            PackInput::ZipBytes(b) => read_zipbytes_into_map(b, &mut files)?,
            PackInput::Url(u) => {
                let bytes = fetch_url_bytes(u)?;
                read_zipbytes_into_map(&bytes, &mut files)?;
            }
        }
    }

    // Write map into an in-memory zip
    let buffer: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buffer);
    let options: zip::write::FileOptions<'_, zip::write::ExtendedFileOptions> =
        zip::write::FileOptions::default().unix_permissions(0o644);

    // Ensure deterministic order by sorting keys
    let mut keys: Vec<&String> = files.keys().collect();
    keys.sort();

    for key in keys {
        let data = &files[key];
        zip.start_file(key, options.clone())?;
        zip.write_all(data)?;
    }

    let writer = zip.finish()?;
    // writer is Cursor<Vec<u8>>
    let mut inner = writer.into_inner();
    // ensure start at 0
    let _ = Cursor::new(&mut inner).seek(SeekFrom::Start(0));
    Ok(inner)
}

/// Merge packs and write resulting zip to a file path.
pub fn merge_packs_to_file<P: AsRef<Path>>(packs: &[PackInput], out: P) -> Result<()> {
    let bytes = merge_packs_to_bytes(packs)?;
    std::fs::write(out, bytes)?;
    Ok(())
}

/// Merge with options and write to file. Currently uses the in-memory path when appropriate.
pub fn merge_packs_to_file_with_options<P: AsRef<Path>>(
    packs: &[PackInput],
    out: P,
    opts: &MergeOptions,
) -> Result<()> {
    // For now, if dry_run just compute plan via merge_packs_to_bytes read-only scan
    if opts.dry_run {
        // perform a simple scan to validate inputs and return early (no writes)
        let _ = merge_packs_to_bytes(packs)?;
        return Ok(());
    }

    // For small inputs we keep using the in-memory path. We'll add streaming dir-based merging later.
    let bytes = merge_packs_to_bytes(packs)?;
    std::fs::write(out, bytes)?;
    Ok(())
}

/// Streaming merge into a directory. This is a placeholder that currently falls back to in-memory behavior
/// for backwards compatibility. Later this should stream per-file into `out_dir` following `opts`.
pub fn merge_packs_to_dir<P: AsRef<Path>>(
    packs: &[PackInput],
    out_dir: P,
    opts: &MergeOptions,
) -> Result<()> {
    // TODO: implement streaming plan+execute.
    if opts.dry_run {
        // validate by scanning using existing in-memory method
        let _ = merge_packs_to_bytes(packs)?;
        return Ok(());
    }

    // Fallback: unzip the in-memory merged zip into out_dir.
    let bytes = merge_packs_to_bytes(packs)?;
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor)?;
    let out_path = out_dir.as_ref();
    std::fs::create_dir_all(out_path)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        let dest = out_path.join(name);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut outfile = std::fs::File::create(dest)?;
        std::io::copy(&mut file, &mut outfile)?;
    }
    Ok(())
}

/// Given a directory which contains multiple resourcepack folders or zip files, merge them all in
/// lexical order. Useful when users supply a single "resourcepacks" folder.
pub fn merge_all_packs_in_folder(folder: &Path) -> Result<Vec<u8>> {
    if !folder.is_dir() {
        return Err(MergeError::InvalidInput(format!(
            "{} is not a dir",
            folder.display()
        )));
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(folder)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    let packs: Vec<PackInput> = entries.into_iter().map(|p| p.into()).collect();
    merge_packs_to_bytes(&packs)
}

/// Read a simple config file (one URL or path per line, comments start with #) and return PackInput list
pub fn read_config_file(path: &Path) -> Result<Vec<PackInput>> {
    let s = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in s.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        out.push(PackInput::from(t.to_string()));
    }
    Ok(out)
}

fn read_dir_into_map(dir: &Path, map: &mut HashMap<String, Vec<u8>>) -> Result<()> {
    if !dir.is_dir() {
        return Err(MergeError::InvalidInput(format!(
            "{} is not a directory",
            dir.display()
        )));
    }

    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file() {
            let rel = path.strip_prefix(dir).unwrap();
            // Use forward slashes as zip paths
            let key = rel
                .iter()
                .map(|p| p.to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let mut f = File::open(path)?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            map.insert(key, buf);
        }
    }
    Ok(())
}

fn read_zipfile_into_map(path: &Path, map: &mut HashMap<String, Vec<u8>>) -> Result<()> {
    let f = File::open(path)?;
    let mut archive = ZipArchive::new(f)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        map.insert(name, buf);
    }
    Ok(())
}

fn read_zipbytes_into_map(bytes: &[u8], map: &mut HashMap<String, Vec<u8>>) -> Result<()> {
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_string();
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        map.insert(name, buf);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, write};
    use tempfile::tempdir;

    #[test]
    fn merge_dirs_and_zipbytes() -> anyhow::Result<()> {
        let d1 = tempdir()?;
        let base = d1.path().join("base");
        create_dir_all(base.join("assets/test"))?;
        write(base.join("assets/test/a.txt"), b"hello")?;
        write(base.join("assets/test/only_in_base.txt"), b"base")?;

        let d2 = tempdir()?;
        let override_dir = d2.path().join("over");
        create_dir_all(override_dir.join("assets/test"))?;
        write(override_dir.join("assets/test/a.txt"), b"world")?;

        // create an in-memory zip with another file
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zw = ZipWriter::new(&mut cursor);
            zw.start_file(
                "assets/test/b.txt",
                zip::write::FileOptions::<zip::write::ExtendedFileOptions>::default(),
            )?;
            zw.write_all(b"fromzip")?;
            zw.finish()?;
        }

        let bytes = cursor.into_inner();

        let packs = vec![
            PackInput::Dir(base),
            PackInput::Dir(override_dir),
            PackInput::ZipBytes(bytes),
        ];

        let out = merge_packs_to_bytes(&packs)?;
        // open output and assert contents
        let mut archive = ZipArchive::new(Cursor::new(out))?;
        {
            let mut a = archive.by_name("assets/test/a.txt")?;
            let mut s = String::new();
            a.read_to_string(&mut s)?;
            assert_eq!(s, "world"); // overridden by second pack
        }

        {
            let mut b = archive.by_name("assets/test/b.txt")?;
            let mut s2 = String::new();
            b.read_to_string(&mut s2)?;
            assert_eq!(s2, "fromzip");
        }

        Ok(())
    }
}
