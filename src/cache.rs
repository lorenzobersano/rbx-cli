//! `export --cache`: each track's analysis, kept on disk and reused while the
//! file is unchanged, so exporting a library again only analyses new tracks.
//!
//! One folder per track, named after a hash of its path, size, modification
//! time and preset, holding `meta.json` (tempo, key, duration, date added) and
//! the three ANLZ files. A hit touches `meta.json`, so a cleanup can drop the
//! entries nobody used for a while by their age.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::Result;

/// Bump when what an entry holds, or how it is computed, changes.
const VERSION: u32 = 1;
const ANLZ: [&str; 3] = ["DAT", "EXT", "2EX"];

/// What an export takes from a track's analysis.
pub struct Entry {
    pub bpm_x100: u32,
    /// Classic notation (`Am`), empty when no key was found.
    pub key: String,
    pub camelot: String,
    pub duration_secs: f64,
    /// The day the track was first analysed: the stick's "date added".
    pub date_added: String,
    pub files: Vec<(String, Vec<u8>)>,
}

/// The entry's folder for this file, or None when its metadata is unreadable.
pub fn folder(root: &Path, path: &Path, preset: &str) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    let id = format!("{VERSION}\0{preset}\0{}\0{}\0{mtime}", path.display(), meta.len());
    // FNV-1a, as track_id.
    let hash = id.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    Some(root.join(format!("{hash:016x}")))
}

pub fn load(dir: &Path) -> Option<Entry> {
    let meta_path = dir.join("meta.json");
    let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(&meta_path).ok()?).ok()?;
    let text = |k: &str| meta[k].as_str().map(str::to_owned);
    let files = ANLZ
        .iter()
        .map(|ext| std::fs::read(dir.join(format!("ANLZ.{ext}"))).ok().map(|b| ((*ext).to_owned(), b)))
        .collect::<Option<Vec<_>>>()?;
    let entry = Entry {
        bpm_x100: u32::try_from(meta["bpm_x100"].as_u64()?).ok()?,
        key: text("key")?,
        camelot: text("camelot")?,
        duration_secs: meta["duration_secs"].as_f64()?,
        date_added: text("date_added")?,
        files,
    };
    // Marks the entry as in use; failing to is harmless.
    if let Ok(f) = std::fs::File::options().append(true).open(&meta_path) {
        let _ = f.set_modified(SystemTime::now());
    }
    Some(entry)
}

/// Writes the entry beside its final place and renames it there, so an
/// interrupted run never leaves half an entry.
pub fn store(dir: &Path, entry: &Entry) -> Result<()> {
    let parent = dir.parent().ok_or("cache: no parent folder")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("cache: {e}"))?;
    let staging = tempfile::tempdir_in(parent).map_err(|e| format!("cache: {e}"))?;
    let meta = json!({
        "bpm_x100": entry.bpm_x100,
        "key": entry.key,
        "camelot": entry.camelot,
        "duration_secs": entry.duration_secs,
        "date_added": entry.date_added,
    });
    std::fs::write(staging.path().join("meta.json"), meta.to_string()).map_err(|e| format!("cache: {e}"))?;
    for (ext, bytes) in &entry.files {
        std::fs::write(staging.path().join(format!("ANLZ.{ext}")), bytes).map_err(|e| format!("cache: {e}"))?;
    }
    let staged = staging.keep();
    if std::fs::rename(&staged, dir).is_err() {
        // Another run stored the same entry first.
        let _ = std::fs::remove_dir_all(&staged);
    }
    Ok(())
}
