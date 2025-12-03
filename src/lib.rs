//! Resource merger library
//!
//! Exposes a small API to merge multiple resource packs (directories, zip bytes, or zip files)
//! into a single zip where later packs overwrite earlier ones.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
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

/// How to synthesize the supported_formats array in pack.mcmeta
#[derive(Debug, Clone, Copy)]
pub enum SupportedFormatsPolicy {
    /// [1, highest_found]
    OneToHighest,
    /// [lowest_found, highest_found]
    LowestToHighest,
    /// [1, latest_known] - not implemented: falls back to OneToHighest
    OneToLatest,
}

impl std::str::FromStr for SupportedFormatsPolicy {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "one-to-highest" | "one_to_highest" | "1-to-highest" | "1-to-high" | "one" => {
                Ok(SupportedFormatsPolicy::OneToHighest)
            }
            "lowest-to-highest" | "lowest_to_highest" | "lowest" => {
                Ok(SupportedFormatsPolicy::LowestToHighest)
            }
            "one-to-latest" | "one_to_latest" => Ok(SupportedFormatsPolicy::OneToLatest),
            other => Err(format!("unknown supported formats policy: {}", other)),
        }
    }
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
    /// If set, force this pack_format in generated pack.mcmeta
    pub pack_format_override: Option<u32>,
    /// How to synthesize supported_formats in pack.mcmeta
    pub supported_formats_policy: SupportedFormatsPolicy,
    /// Optional description to use in generated pack.mcmeta
    pub description_override: Option<String>,
    /// If true, continue when input URLs fail to download or aren't valid zips (warn and skip)
    pub tolerate_missing_inputs: bool,
}

impl Default for MergeOptions {
    fn default() -> Self {
        MergeOptions {
            overwrite: OverwritePolicy::LastWins,
            dry_run: false,
            buffer_size: 32 * 1024,
            atomic: true,
            preserve_timestamps: false,
            pack_format_override: None,
            supported_formats_policy: SupportedFormatsPolicy::OneToHighest,
            description_override: None,
            tolerate_missing_inputs: false,
        }
    }
}

/// Represents an input pack. It can be a directory on disk, a zip file on disk, or raw zip bytes.
#[derive(Debug, Clone)]
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
    // Capture content-type header before consuming the response
    let ct_header = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let bytes = resp
        .bytes()
        .map_err(|e| MergeError::InvalidInput(format!("read {} body: {}", url, e)))?;
    let b = bytes.to_vec();
    // Quick sanity check: ensure the bytes look like a ZIP file (start with PK signature).
    // Many servers may return HTML error pages or other content; detect that early.
    if b.len() >= 2 && &b[0..2] == b"PK" {
        Ok(b)
    } else {
        // Try to include content-type header for better debugging
        let ct = ct_header.as_deref().unwrap_or("<unknown>");
        Err(MergeError::InvalidInput(format!(
            "GET {} did not return a zip file (content-type: {}).",
            url, ct
        )))
    }
}

/// Merge multiple packs into a single zip archive (returned as Vec<u8>).
///
/// The order of `packs` matters: earlier packs form the base, later packs overwrite files with the
/// same path.
pub fn merge_packs_to_bytes(packs: &[PackInput]) -> Result<Vec<u8>> {
    // Backwards-compatible wrapper: use default options
    merge_packs_to_bytes_with_options(packs, &MergeOptions::default())
}

pub fn merge_packs_to_bytes_with_options(
    packs: &[PackInput],
    opts: &MergeOptions,
) -> Result<Vec<u8>> {
    // We'll maintain a map of path -> file bytes. Later packs overwrite earlier ones.
    let mut files: HashMap<String, Vec<u8>> = HashMap::new();
    // Track pack_format and max_format numbers found in inputs
    let mut found_formats: Vec<u32> = Vec::new();
    let mut found_max_formats: Vec<u32> = Vec::new();
    // Collect overlays from all packs (later packs overwrite earlier ones)
    let mut overlays_values: Vec<serde_json::Value> = Vec::new();

    // First, inspect each input for pack.mcmeta to collect pack_format values across all inputs.
    // We do a best-effort peek so we can choose the HIGHEST pack_format observed, independent
    // of later overwrites.
    for pack in packs {
        match pack {
            PackInput::Dir(p) => {
                if let Some((pf, mf, overlays)) = peek_pack_format_from_dir(p) {
                    found_formats.push(pf);
                    if let Some(max) = mf {
                        found_max_formats.push(max);
                    }
                    if let Some(ov) = overlays {
                        overlays_values.push(ov);
                    }
                }
                read_dir_into_map(p, &mut files)?;
            }
            PackInput::ZipFile(p) => {
                if let Some((pf, mf, overlays)) = peek_pack_format_from_zipfile(p) {
                    found_formats.push(pf);
                    if let Some(max) = mf {
                        found_max_formats.push(max);
                    }
                    if let Some(ov) = overlays {
                        overlays_values.push(ov);
                    }
                }
                read_zipfile_into_map(p, &mut files)?;
            }
            PackInput::ZipBytes(b) => {
                if let Some((pf, mf, overlays)) = peek_pack_format_from_zipbytes(b) {
                    found_formats.push(pf);
                    if let Some(max) = mf {
                        found_max_formats.push(max);
                    }
                    if let Some(ov) = overlays {
                        overlays_values.push(ov);
                    }
                }
                read_zipbytes_into_map(b, &mut files)?;
            }
            PackInput::Url(u) => match fetch_url_bytes(u) {
                Ok(bytes) => {
                    if let Some((pf, mf, overlays)) = peek_pack_format_from_zipbytes(&bytes) {
                        found_formats.push(pf);
                        if let Some(max) = mf {
                            found_max_formats.push(max);
                        }
                        if let Some(ov) = overlays {
                            overlays_values.push(ov);
                        }
                    }
                    read_zipbytes_into_map(&bytes, &mut files)?;
                }
                Err(e) => {
                    if opts.tolerate_missing_inputs {
                        eprintln!("warning: skipping input {}: {}", u, e);
                    } else {
                        return Err(e);
                    }
                }
            },
        }
    }

    // Inspect any pack.mcmeta files found and collect pack_format values
    // (overlays are now collected during the peek phase above)
    for (k, v) in &files {
        if k == "pack.mcmeta" || k.ends_with("/pack.mcmeta") {
            if let Ok(s) = std::str::from_utf8(v) {
                if let Ok((pf, mf)) = extract_pack_format_from_mcmeta(s) {
                    found_formats.push(pf);
                    if let Some(max) = mf {
                        found_max_formats.push(max);
                    }
                }
            }
        }
    }

    // Write map into an in-memory zip
    let buffer: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    let mut zip = ZipWriter::new(buffer);
    let options: zip::write::FileOptions<'_, zip::write::ExtendedFileOptions> =
        zip::write::FileOptions::default().unix_permissions(0o644);

    // Ensure deterministic order by sorting keys
    // We'll skip certain auto-generated names when emitting from the map so we can synthesize them
    let mut keys: Vec<&String> = files
        .keys()
        .filter(|k| {
            let kk = k.as_str();
            kk != "pack.mcmeta" && kk != "pack.png" && kk != "README.md"
        })
        .collect();
    keys.sort();

    for key in keys {
        let data = &files[key];
        zip.start_file(key, options.clone())?;
        zip.write_all(data)?;
    }

    // Determine final pack_format: override via opts if present, otherwise highest found or 1
    let final_pack_fmt = if let Some(ov) = opts.pack_format_override {
        ov
    } else if found_formats.is_empty() {
        1u32
    } else {
        *found_formats.iter().max().unwrap_or(&1u32)
    };

    // Compute supported_formats vector based on policy.
    // For user-friendly pack.mcmeta we emit only the endpoint values (lowest/highest)
    // instead of every integer in the inclusive range. Examples:
    // - OneToHighest => [1, high]
    // - LowestToHighest => [low, high]
    // If low == high we emit a single-element array [low].
    let supported_formats: Vec<u32> = match opts.supported_formats_policy {
        SupportedFormatsPolicy::OneToHighest => {
            let high = if found_formats.is_empty() {
                final_pack_fmt
            } else {
                *found_formats.iter().max().unwrap_or(&final_pack_fmt)
            };
            if high <= 1 {
                vec![1u32]
            } else {
                vec![1u32, high]
            }
        }
        SupportedFormatsPolicy::LowestToHighest => {
            if found_formats.is_empty() {
                vec![final_pack_fmt]
            } else {
                let low = *found_formats.iter().min().unwrap_or(&final_pack_fmt);
                let high = *found_formats.iter().max().unwrap_or(&final_pack_fmt);
                if low == high {
                    vec![low]
                } else {
                    vec![low, high]
                }
            }
        }
        SupportedFormatsPolicy::OneToLatest => {
            // Not implemented: fall back to OneToHighest for now
            let high = if found_formats.is_empty() {
                final_pack_fmt
            } else {
                *found_formats.iter().max().unwrap_or(&final_pack_fmt)
            };
            if high <= 1 {
                vec![1u32]
            } else {
                vec![1u32, high]
            }
        }
    };

    // Determine actual max format from all sources
    let actual_max_format = if found_max_formats.is_empty() {
        *supported_formats.last().unwrap_or(&final_pack_fmt)
    } else {
        *found_max_formats.iter().max().unwrap_or(&final_pack_fmt)
    };

    // Merge overlays: later ones overwrite earlier, keyed by directory name
    let merged_overlays = merge_overlays(&overlays_values);

    // Ensure pack.mcmeta exists with an appropriate pack_format & supported_formats
    let mcmeta = make_pack_mcmeta(
        final_pack_fmt,
        &supported_formats,
        opts.description_override.as_deref(),
        actual_max_format,
        merged_overlays.as_ref(),
    );
    zip.start_file("pack.mcmeta", options.clone())?;
    zip.write_all(mcmeta.as_bytes())?;

    // Ensure pack.png exists (small default) if missing
    // Always write our embedded default pack.png into the merged zip as pack.png.
    // This ensures a consistent default image regardless of input packs.
    let png = default_pack_png_bytes();
    zip.start_file("pack.png", options.clone())?;
    zip.write_all(&png)?;

    // Ensure README.md exists with simple generation notes
    if !files.contains_key("README.md") {
        let readme = make_readme(packs);
        zip.start_file("README.md", options.clone())?;
        zip.write_all(readme.as_bytes())?;
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
        let _ = merge_packs_to_bytes_with_options(packs, opts)?;
        return Ok(());
    }

    // For small inputs we keep using the in-memory path. We'll add streaming dir-based merging later.
    let bytes = merge_packs_to_bytes_with_options(packs, opts)?;
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
        let _ = merge_packs_to_bytes_with_options(packs, opts)?;
        return Ok(());
    }

    // Fallback: unzip the in-memory merged zip into out_dir.
    let bytes = merge_packs_to_bytes_with_options(packs, opts)?;
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor)?;
    let out_path = out_dir.as_ref();
    std::fs::create_dir_all(out_path)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        if file.is_dir() {
            continue;
        }
        let raw_name = file.name().to_string();
        let name = match sanitize_zip_entry_name(&raw_name) {
            Some(n) => n,
            None => continue,
        };
        // Build a destination path from the sanitized components to ensure correct
        // OS-specific separators and avoid zip-slip.
        let dest = {
            let mut p = out_path.to_path_buf();
            for comp in name.split('/') {
                p.push(comp);
            }
            p
        };
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

/// Settings that represent the full runtime configuration for a merge run.
/// This mirrors the CLI args/config file and is the single object used to execute a merge.
#[derive(Debug, Clone)]
pub struct Settings {
    /// Ordered list of inputs (directories, zip files, or URLs). These are applied in order.
    pub inputs: Vec<PackInput>,
    /// Output path (file or directory) - resolved by the caller
    pub out: PathBuf,
    /// If true, write to a directory instead of a zip file
    pub dir: bool,
    /// Merge behavior options
    pub options: MergeOptions,
}

/// Execute a merge according to `Settings`.
/// This is the single entrypoint consumers (like the CLI) should call.
pub fn run_with_settings(settings: &Settings) -> Result<()> {
    if settings.dir {
        merge_packs_to_dir(&settings.inputs, &settings.out, &settings.options)
    } else {
        merge_packs_to_file_with_options(&settings.inputs, &settings.out, &settings.options)
    }
}

/// Read a simple config file (one URL or path per line, comments start with #) and return PackInput list
use serde::Deserialize;

/// Configuration structure for JSON config files.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// Ordered list of inputs (directories, zip files, or URLs). These are applied first.
    pub inputs: Option<Vec<String>>,
    /// Overwrite policy: last, first, error, skip
    pub overwrite: Option<String>,
    /// Dry run
    pub dry_run: Option<bool>,
    /// Buffer size
    pub buffer_size: Option<usize>,
    /// Atomic write
    pub atomic: Option<bool>,
    /// Preserve timestamps
    pub preserve_timestamps: Option<bool>,
    /// Force pack_format
    pub pack_format: Option<u32>,
    /// Supported formats policy: one-to-highest, lowest-to-highest, one-to-latest
    pub supported_formats: Option<String>,
    /// Optional output path if you want the config to specify a default output file
    pub out: Option<String>,
    /// If true, write output as a directory instead of a zip file
    pub dir: Option<bool>,
    /// Optional description to use for generated pack.mcmeta
    pub description: Option<String>,
    /// If true, continue when input URLs fail to download or aren't valid zips
    pub tolerate_missing_inputs: Option<bool>,
}

/// Read a JSON config file and return a Config structure.
pub fn read_config_file(path: &Path) -> Result<Config> {
    let s = std::fs::read_to_string(path)?;
    let cfg: Config = serde_json::from_str(&s).map_err(|e| {
        MergeError::InvalidInput(format!(
            "failed to parse JSON config {}: {}",
            path.display(),
            e
        ))
    })?;
    Ok(cfg)
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
        // Sanitize zip entry name to a normalized forward-slash form and skip unsafe entries
        let name = match sanitize_zip_entry_name(&name) {
            Some(n) => n,
            None => continue,
        };
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
        let name = match sanitize_zip_entry_name(&name) {
            Some(n) => n,
            None => continue,
        };
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        map.insert(name, buf);
    }
    Ok(())
}

/// Normalize a zip entry name into a safe forward-slash form suitable for
/// using as a zip path and for converting into OS paths when extracting.
/// Returns None for absolute paths or entries that attempt to traverse up
/// the filesystem ("..").
fn sanitize_zip_entry_name(name: &str) -> Option<String> {
    // Convert any backslashes to forward slashes (some zip writers use them)
    let n = name.replace('\\', "/");
    // Reject absolute paths
    if n.starts_with('/') || n.starts_with("\\") {
        return None;
    }
    // Split and remove any empty components (caused by leading/trailing slashes)
    let comps: Vec<&str> = n.split('/').filter(|s| !s.is_empty()).collect();
    // Reject parent-traversal components for safety (zip-slip)
    if comps.contains(&"..") {
        return None;
    }
    if comps.is_empty() {
        return None;
    }
    Some(comps.join("/"))
}

// Peek functions: try to locate pack.mcmeta and extract pack_format without reading all files.
// Returns (pack_format, max_format_option, overlays_option)
fn peek_pack_format_from_zipbytes(
    bytes: &[u8],
) -> Option<(u32, Option<u32>, Option<serde_json::Value>)> {
    let cursor = Cursor::new(bytes);
    if let Ok(mut archive) = ZipArchive::new(cursor) {
        if let Ok(mut file) = archive.by_name("pack.mcmeta") {
            let mut buf = String::new();
            if file.read_to_string(&mut buf).is_ok() {
                if let Ok(formats) = extract_pack_format_from_mcmeta(&buf) {
                    let overlays = extract_overlays_from_mcmeta(&buf);
                    return Some((formats.0, formats.1, overlays));
                }
            }
        }
    }
    None
}

fn peek_pack_format_from_zipfile(
    path: &Path,
) -> Option<(u32, Option<u32>, Option<serde_json::Value>)> {
    if let Ok(f) = File::open(path) {
        if let Ok(mut archive) = ZipArchive::new(f) {
            if let Ok(mut file) = archive.by_name("pack.mcmeta") {
                let mut buf = String::new();
                if file.read_to_string(&mut buf).is_ok() {
                    if let Ok(formats) = extract_pack_format_from_mcmeta(&buf) {
                        let overlays = extract_overlays_from_mcmeta(&buf);
                        return Some((formats.0, formats.1, overlays));
                    }
                }
            }
        }
    }
    None
}

fn peek_pack_format_from_dir(dir: &Path) -> Option<(u32, Option<u32>, Option<serde_json::Value>)> {
    let p = dir.join("pack.mcmeta");
    if p.is_file() {
        if let Ok(s) = std::fs::read_to_string(p) {
            if let Ok(formats) = extract_pack_format_from_mcmeta(&s) {
                let overlays = extract_overlays_from_mcmeta(&s);
                return Some((formats.0, formats.1, overlays));
            }
        }
    }
    None
}

/// Extract overlays section from a pack.mcmeta JSON string.
fn extract_overlays_from_mcmeta(s: &str) -> Option<serde_json::Value> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(s) {
        if let Some(overlays) = json.get("overlays") {
            return Some(overlays.clone());
        }
    }
    None
}

/// Merge overlays from multiple pack.mcmeta files.
/// Later overlays overwrite earlier ones based on directory name.
fn merge_overlays(overlays_list: &[serde_json::Value]) -> Option<serde_json::Value> {
    if overlays_list.is_empty() {
        return None;
    }

    // Collect all overlay entries, keyed by directory name (later overwrites earlier)
    let mut merged_entries: HashMap<String, serde_json::Value> = HashMap::new();

    for overlay_val in overlays_list {
        if let Some(entries_arr) = overlay_val.get("entries").and_then(|v| v.as_array()) {
            for entry in entries_arr {
                if let Some(dir) = entry.get("directory").and_then(|v| v.as_str()) {
                    merged_entries.insert(dir.to_string(), entry.clone());
                }
            }
        }
    }

    if merged_entries.is_empty() {
        return None;
    }

    // Convert back to array, sorted by directory name for determinism
    let mut sorted_entries: Vec<_> = merged_entries.into_iter().collect();
    sorted_entries.sort_by(|a, b| a.0.cmp(&b.0));
    let entries_array: Vec<serde_json::Value> =
        sorted_entries.into_iter().map(|(_, v)| v).collect();

    Some(serde_json::json!({
        "entries": entries_array
    }))
}

/// Try to extract pack_format and max_format from a pack.mcmeta JSON string.
/// Returns (pack_format, max_format) where max_format might be higher than pack_format.
fn extract_pack_format_from_mcmeta(s: &str) -> std::result::Result<(u32, Option<u32>), ()> {
    // Quick and tolerant parser: look for "pack_format", "max_format", and "supported_formats".
    // Accept both the common shape { "pack": { ... } } and rare top-level fields.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
        // helper to extract numeric from a Value
        let try_from_value = |val: &serde_json::Value| -> Option<u32> {
            if let Some(n) = val.as_u64() {
                return Some(n as u32);
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<u32>() {
                    return Some(n);
                }
            }
            None
        };

        let mut pack_format = None;
        let mut max_format = None;

        // Check common shape: { "pack": { ... } }
        if let Some(pack) = v.get("pack") {
            if let Some(fmt) = pack.get("pack_format") {
                pack_format = try_from_value(fmt);
            }

            // Look for explicit max_format
            if let Some(mf) = pack.get("max_format") {
                max_format = try_from_value(mf);
            }

            // Also check supported_formats for max_inclusive
            if let Some(sf) = pack.get("supported_formats") {
                if let Some(obj) = sf.as_object() {
                    if let Some(max_inc) = obj.get("max_inclusive") {
                        if let Some(n) = try_from_value(max_inc) {
                            max_format = Some(n);
                        }
                    }
                }
            }
        }

        // Fallback: check top-level pack_format and max_format
        if pack_format.is_none() {
            if let Some(fmt) = v.get("pack_format") {
                pack_format = try_from_value(fmt);
            }
        }
        if max_format.is_none() {
            if let Some(mf) = v.get("max_format") {
                max_format = try_from_value(mf);
            }
        }

        if let Some(pf) = pack_format {
            return Ok((pf, max_format));
        }
    }
    Err(())
}

fn make_pack_mcmeta(
    pack_format: u32,
    supported_formats: &[u32],
    description: Option<&str>,
    max_format: u32,
    overlays: Option<&serde_json::Value>,
) -> String {
    let desc = description.map(|s| s.to_string()).unwrap_or_else(|| {
        format!(
            "Made with Rust API: resource_merger:{}",
            env!("CARGO_PKG_VERSION")
        )
    });

    // Threshold for backwards compatibility: resource pack format < 65 requires old format
    const OLD_FORMAT_THRESHOLD: u32 = 65;

    // Determine min from supported_formats array
    let min_format = supported_formats.first().copied().unwrap_or(pack_format);

    // Check if we need backwards compatibility fields (if min_format < 65)
    let needs_old_format = min_format < OLD_FORMAT_THRESHOLD;

    let mut meta = if needs_old_format {
        // Old format: include pack_format and supported_formats for backwards compatibility
        serde_json::json!({
            "pack": {
                "pack_format": pack_format,
                "min_format": min_format,
                "max_format": max_format,
                "description": desc,
                "supported_formats": supported_formats
            }
        })
    } else {
        // New format (1.21.9+): use min_format and max_format only
        serde_json::json!({
            "pack": {
                "min_format": min_format,
                "max_format": max_format,
                "description": desc
            }
        })
    };

    // Add overlays if present
    if let Some(overlays_val) = overlays {
        if let Some(obj) = meta.as_object_mut() {
            obj.insert("overlays".to_string(), overlays_val.clone());
        }
    }

    // Use compact JSON (single-line) for smaller file size - Minecraft supports this
    serde_json::to_string(&meta).unwrap_or_else(|_| {
        "{\"pack\":{\"min_format\":1,\"max_format\":1,\"description\":\"resource_merger\"}}"
            .to_string()
    })
}

fn default_pack_png_bytes() -> Vec<u8> {
    // Include the default 64x64 pack image binary at compile time. This uses the
    // provided PNG file `assets/default-pack-64.png` and embeds its bytes into
    // the binary so we can always write `pack.png` when inputs don't provide one.
    const BYTES: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/default-pack-64.png"
    ));
    BYTES.to_vec()
}

fn make_readme(packs: &[PackInput]) -> String {
    let mut out = String::new();
    out.push_str("This resource pack was generated by resource_merger.\n\n");
    out.push_str("Inputs used (in order, first -> last):\n");
    for p in packs {
        match p {
            PackInput::Dir(pb) => {
                out.push_str(&format!("- Dir: {}\n", pb.display()));
            }
            PackInput::ZipFile(pb) => {
                out.push_str(&format!("- ZipFile: {}\n", pb.display()));
            }
            PackInput::ZipBytes(_) => {
                out.push_str("- ZipBytes: <in-memory>\n");
            }
            PackInput::Url(u) => {
                out.push_str(&format!("- Url: {}\n", u));
            }
        }
    }
    out.push_str(&format!(
        "\nGenerated with resource_merger {}",
        env!("CARGO_PKG_VERSION")
    ));
    out
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
