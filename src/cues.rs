//! Hot cues, memory cues and loops across exports.
//!
//! A player keeps the cues set on it in the track's ANLZ files on the stick,
//! and an export writes those files again from the computer's side, so
//! without help a sync drops them. rbxport's manifest on the stick records a
//! hash of every ANLZ file it wrote: a file whose bytes no longer match was
//! changed on a player since, and `export` reads its cues back and exports
//! them with the track.
//!
//! With `--cues DIR` the cues are also kept in DIR, one JSON file per track,
//! so a cue set on one stick reaches every stick the library is exported to.
//! For each track the stick's cues win when a player changed them since the
//! last sync; otherwise the store's do. A track changed on two sticks between
//! syncs keeps the cues of the stick exported last.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rbl_anlz::cues::ExportCue;
use serde_json::json;

use crate::Result;

/// The cues in a track's `PCO2` (the `.EXT`, or the `.DAT` when there is
/// none), as `rbl_export` takes them.
pub fn read(dat: &Path) -> Vec<ExportCue> {
    let ext = rbl_anlz::sibling(dat, "EXT");
    let Ok(anlz) = rbl_anlz::Anlz::read(if ext.is_file() { &ext } else { dat }) else { return Vec::new() };
    anlz.cue_entries()
        .into_iter()
        .map(|c| ExportCue {
            // Hot cue slot 1 = A; rbxport's kinds skip 4: A–C are 1–3, D–P 5–17.
            kind: match c.hot_cue {
                0..=3 => c.hot_cue as u8,
                slot => u8::try_from(slot + 1).unwrap_or(u8::MAX),
            },
            time_ms: c.time_ms,
            loop_time_ms: (c.kind == 2).then_some(c.loop_time_ms),
            color_id: c.color_id,
            color_code: c.color_code.unwrap_or(0),
            comment: c.comment.unwrap_or_default(),
            ..Default::default()
        })
        .collect()
}

/// What an earlier export left on the stick for one track.
struct OnStick {
    dat: PathBuf,
    /// A player changed the analysis since that export.
    edited: bool,
}

/// Where each track's cues come from in one export.
pub struct Cues {
    stick: HashMap<String, OnStick>,
    store: Option<PathBuf>,
    /// Tracks whose cues were taken from the stick, for the report.
    pub from_player: Vec<String>,
}

impl Cues {
    pub fn new(destination: &Path, store: Option<&Path>) -> Result<Self> {
        if let Some(dir) = store {
            std::fs::create_dir_all(dir).map_err(|e| format!("cues: {}: {e}", dir.display()))?;
        }
        let stick = rbl_export::manifest::Manifest::load(destination)
            .map(|m| {
                m.tracks
                    .into_iter()
                    .filter(|t| !t.anlz_dir.is_empty())
                    .map(|t| {
                        let dir = destination.join(t.anlz_dir.trim_start_matches('/'));
                        let edited = t.analysis_hashes.iter().any(|(ext, recorded)| {
                            std::fs::read(dir.join(format!("ANLZ0000.{ext}")))
                                .is_ok_and(|bytes| rbl_export::manifest::hash(&bytes) != *recorded)
                        });
                        (t.source, OnStick { dat: dir.join("ANLZ0000.DAT"), edited })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self { stick, store: store.map(Path::to_path_buf), from_player: Vec::new() })
    }

    /// The source paths of the tracks whose analysis a player changed since
    /// the last export.
    pub fn edited(&self) -> Vec<&str> {
        let mut edited: Vec<&str> = self.stick.iter().filter(|(_, t)| t.edited).map(|(s, _)| s.as_str()).collect();
        edited.sort_unstable();
        edited
    }

    /// The cues to export with this track: None keeps what its analysis holds.
    pub fn for_track(&mut self, path: &Path, id: u64) -> Result<Option<Vec<ExportCue>>> {
        let source = path.to_string_lossy();
        let on_stick = self.stick.get(source.as_ref());
        if let Some(t) = on_stick.filter(|t| t.edited) {
            let cues = read(&t.dat);
            self.save(path, id, &cues)?;
            self.from_player.push(source.into_owned());
            return Ok(Some(cues));
        }
        if let Some(cues) = self.load(id) {
            return Ok(Some(cues));
        }
        // Cues the stick held before there was a store, or with no store.
        let cues = on_stick.map(|t| read(&t.dat)).unwrap_or_default();
        if cues.is_empty() {
            return Ok(None);
        }
        self.save(path, id, &cues)?;
        Ok(Some(cues))
    }

    fn file(&self, id: u64) -> Option<PathBuf> {
        self.store.as_ref().map(|dir| dir.join(format!("{id:016x}.json")))
    }

    fn load(&self, id: u64) -> Option<Vec<ExportCue>> {
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(self.file(id)?).ok()?).ok()?;
        serde_json::from_value(value["cues"].clone()).ok()
    }

    /// Written aside and renamed, so an interrupted run never leaves half a file.
    fn save(&self, path: &Path, id: u64, cues: &[ExportCue]) -> Result<()> {
        let Some(file) = self.file(id) else { return Ok(()) };
        let staged = file.with_extension("json.tmp");
        let body = json!({"source": path, "cues": cues});
        std::fs::write(&staged, body.to_string())
            .and_then(|()| std::fs::rename(&staged, &file))
            .map_err(|e| format!("cues: {}: {e}", file.display()))
    }
}
