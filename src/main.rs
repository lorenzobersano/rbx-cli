//! Command-line analysis and USB export built on rbxport's crates.
//!
//!     rbx-cli analyse FILE... [--preset normal|dynamic] [--beats]
//!     rbx-cli export SOURCE DESTINATION [--playlist NAME] [--preset ...] [--key camelot|classic] [--cache DIR] [--cues DIR]
//!     rbx-cli rebuild REKORDBOX_USB DESTINATION [--preset ...] [--key camelot|classic]
//!     rbx-cli inspect USB [--tracks 3] [--anlz 3]
//!     rbx-cli player-edits USB
//!     rbx-cli validate USB [--json]
//!     rbx-cli compare-grid USB [--tracks 40] [--skip 3] [--paths FILE...] [--json]
//!     rbx-cli compare-waveform USB [--tracks 20] [--skip 0] [--paths FILE...] [--png DIR]
//!
//! `analyse` prints one JSON object per file. `export` decodes and analyses
//! every audio file under SOURCE, reads its tags and embedded cover, and
//! writes a rekordbox USB library to DESTINATION with `rbl_export`: one
//! playlist per folder of SOURCE, or one called --playlist with every track.
//! It prints the export report as JSON. Progress goes to stderr. `inspect`
//! and `validate` read an export (`usb.rs`); the `compare-*` commands set
//! rbxport's analysis against what rekordbox wrote on a USB (`compare.rs`);
//! `rebuild` writes a rekordbox USB again with rbxport's analysis (`rebuild.rs`).
//!
//! The glue between the crates follows rbxport's own (`src-tauri/src/analysis.rs`
//! and `commands.rs`), minus its master.db.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Parser, Subcommand, ValueEnum};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::tag::{Accessor, ItemKey};
use serde_json::json;

mod cache;
mod compare;
mod cues;
mod rebuild;
mod usb;

/// rekordbox's analysis decodes at most half an hour, as rbxport's does.
const DECODE_CAP_SECS: f64 = 1800.0;
const AUDIO: [&str; 7] = ["mp3", "m4a", "mp4", "flac", "wav", "aif", "aiff"];
/// The small and medium images rekordbox writes for each cover.
const ARTWORK_SMALL: u32 = 80;
const ARTWORK_MEDIUM: u32 = 240;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyse files and print tempo, key and grid as JSON lines.
    Analyse {
        files: Vec<PathBuf>,
        #[arg(long, value_enum, default_value_t = Preset::Normal)]
        preset: Preset,
        /// Include every beat of the grid.
        #[arg(long)]
        beats: bool,
    },
    /// Analyse a folder of audio files and export it as a rekordbox USB.
    Export {
        source: PathBuf,
        destination: PathBuf,
        /// Put every track in one playlist with this name.
        #[arg(long)]
        playlist: Option<String>,
        #[arg(long, value_enum, default_value_t = Preset::Normal)]
        preset: Preset,
        #[arg(long, value_enum, default_value_t = KeyNotation::Camelot)]
        key: KeyNotation,
        /// Library folder for a blank volume; auto picks .PIONEER on HFS+.
        #[arg(long, value_enum, default_value_t = Root::Auto)]
        root: Root,
        /// Keep each track's analysis here and reuse it while the file is unchanged.
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Keep each track's cues here and export them to every stick; cues
        /// changed on a player are always kept, store or not.
        #[arg(long)]
        cues: Option<PathBuf>,
    },
    /// Write a rekordbox USB again: same tracks, playlist tree, metadata,
    /// covers and cues, with rbxport's beat grid, key and waveforms.
    Rebuild {
        source: PathBuf,
        destination: PathBuf,
        #[arg(long, value_enum, default_value_t = Preset::Normal)]
        preset: Preset,
        #[arg(long, value_enum, default_value_t = KeyNotation::Camelot)]
        key: KeyNotation,
        #[arg(long, value_enum, default_value_t = Root::Auto)]
        root: Root,
        /// Only the first N tracks, to try a rebuild quickly.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Summarise an export: tables and pages, tracks, playlists, ANLZ tags, validation.
    Inspect {
        usb: PathBuf,
        #[arg(long, default_value_t = 3)]
        tracks: usize,
        #[arg(long, default_value_t = 3)]
        anlz: usize,
    },
    /// List the tracks whose analysis (cues, grid) a player changed since the
    /// last export, one source path per line.
    PlayerEdits { usb: PathBuf },
    /// Check an export's pages, references and files; exits 1 on errors.
    Validate {
        usb: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Compare tempo, phase, downbeat and key with rekordbox's on a rekordbox USB.
    CompareGrid {
        usb: PathBuf,
        #[arg(long, default_value_t = 40)]
        tracks: usize,
        #[arg(long, default_value_t = 3)]
        skip: usize,
        /// Compare these files (on the USB, or links to them) instead of a sample.
        #[arg(long, num_args = 1..)]
        paths: Vec<PathBuf>,
        #[arg(long, value_enum, default_value_t = Preset::Normal)]
        preset: Preset,
        /// One JSON object per track, then a summary.
        #[arg(long)]
        json: bool,
        /// Print every track, not only the ones that differ.
        #[arg(long)]
        verbose: bool,
    },
    /// Compare generated waveforms with rekordbox's on a rekordbox USB.
    CompareWaveform {
        usb: PathBuf,
        #[arg(long, default_value_t = 20)]
        tracks: usize,
        #[arg(long, default_value_t = 0)]
        skip: usize,
        #[arg(long, num_args = 1..)]
        paths: Vec<PathBuf>,
        #[arg(long, value_enum, default_value_t = Preset::Normal)]
        preset: Preset,
        /// Save one image per track here: PWV4, then 20 s of PWV5, rekordbox above ours.
        #[arg(long)]
        png: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Preset {
    /// rekordbox's Normal analysis: one constant tempo.
    Normal,
    /// rbxport's own: follows tempo changes.
    Dynamic,
}

impl Preset {
    const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Dynamic => "dynamic",
        }
    }

    fn options(self) -> rbl_analysis::AnalysisOptions {
        match self {
            Self::Normal => rbl_analysis::AnalysisPreset::Rekordbox,
            Self::Dynamic => rbl_analysis::AnalysisPreset::Rbxport,
        }
        .options()
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum KeyNotation {
    Classic,
    Camelot,
}

#[derive(Clone, Copy, ValueEnum)]
enum Root {
    Auto,
    Standard,
    Hidden,
}

type Result<T> = std::result::Result<T, String>;

fn main() {
    let outcome = match Cli::parse().command {
        Command::Analyse { files, preset, beats } => analyse_files(&files, preset, beats),
        Command::Export { source, destination, playlist, preset, key, root, cache, cues } => {
            export(&source, &destination, playlist.as_deref(), preset, key, root, cache.as_deref(), cues.as_deref())
        }
        Command::Rebuild { source, destination, preset, key, root, limit } => {
            rebuild::rebuild(&source, &destination, preset, key, root, limit)
        }
        Command::Inspect { usb, tracks, anlz } => usb::inspect(&usb, tracks, anlz),
        Command::PlayerEdits { usb } => cues::Cues::new(&usb, None).map(|c| c.edited().iter().for_each(|s| println!("{s}"))),
        Command::Validate { usb, json } => usb::validate(&usb).map(|report| {
            if json {
                println!("{}", json!({"errors": report.errors, "warnings": report.warnings}));
            } else {
                usb::print_report(&report);
            }
            if !report.errors.is_empty() {
                std::process::exit(1);
            }
        }),
        Command::CompareGrid { usb, tracks, skip, paths, preset, json, verbose } => {
            compare::compare_grid(&usb, tracks, skip, &paths, preset, json, verbose)
        }
        Command::CompareWaveform { usb, tracks, skip, paths, preset, png } => {
            compare::compare_waveform(&usb, tracks, skip, &paths, preset, png.as_deref())
        }
    };
    if let Err(e) = outcome {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

struct Analysed {
    analysis: rbl_analysis::Analysis,
    duration_secs: f64,
    elapsed_ms: u128,
}

fn analyse(path: &Path, preset: Preset) -> Result<Analysed> {
    let started = Instant::now();
    let audio = rbl_audio::decode_mono(path, Some(DECODE_CAP_SECS)).map_err(|e| format!("{}: {e}", path.display()))?;
    if audio.samples.is_empty() {
        return Err(format!("{}: decoded no samples", path.display()));
    }
    let analysis = rbl_analysis::analyse_with(&audio.samples, audio.sample_rate, preset.options());
    Ok(Analysed { analysis, duration_secs: audio.duration_secs(), elapsed_ms: started.elapsed().as_millis() })
}

fn analyse_files(files: &[PathBuf], preset: Preset, with_beats: bool) -> Result<()> {
    for path in files {
        let a = match analyse(path, preset) {
            Ok(a) => a,
            Err(e) => {
                println!("{}", json!({"path": path, "error": e}));
                continue;
            }
        };
        let tempo = &a.analysis.tempo;
        let mut out = json!({
            "path": path,
            "bpm": tempo.bpm,
            "confidence": tempo.confidence,
            "first_beat_ms": tempo.beats.first().map(|b| b.time_ms),
            "first_beat_number": tempo.beats.first().map(|b| b.beat_number),
            "segments": tempo.segments.len(),
            "beat_count": tempo.beats.len(),
            "key": a.analysis.key.as_ref().map(|k| &k.name),
            "camelot": a.analysis.key.as_ref().map(rbl_analysis::key::MusicalKey::camelot),
            "duration_secs": a.duration_secs,
            "elapsed_ms": a.elapsed_ms,
        });
        if with_beats {
            out["beats"] = tempo.beats.iter().map(|b| json!([b.time_ms, b.beat_number, b.tempo_x100])).collect();
        }
        println!("{out}");
    }
    Ok(())
}

fn key_name(analysis: &rbl_analysis::Analysis, notation: KeyNotation) -> String {
    analysis.key.as_ref().map_or_else(String::new, |k| match notation {
        KeyNotation::Classic => k.name.clone(),
        KeyNotation::Camelot => k.camelot(),
    })
}

fn bpm_x100(analysis: &rbl_analysis::Analysis) -> u32 {
    (analysis.tempo.bpm * 100.0).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// The three ANLZ files for an analysis, as `rbl_export` takes them.
fn analysis_files(audio_path: &Path, analysis: &rbl_analysis::Analysis) -> Vec<(String, Vec<u8>)> {
    let beats: Vec<rbl_anlz::Beat> = analysis
        .tempo
        .beats
        .iter()
        .map(|b| rbl_anlz::Beat { beat_number: b.beat_number, tempo_x100: b.tempo_x100, time_ms: b.time_ms })
        .collect();
    let columns: Vec<rbl_anlz::BandColumn> = analysis
        .waveform
        .columns
        .iter()
        .map(|c| rbl_anlz::BandColumn { low: c.low, mid: c.mid, high: c.high, peak: c.peak })
        .collect();
    // The export rewrites PPTH to the file's path on the stick.
    let files = rbl_anlz::author_with_overview(
        &rbl_anlz::share_tree_ppth(&audio_path.to_string_lossy()),
        &beats,
        &columns,
        analysis.waveform.overview.as_slice().try_into().ok(),
        rbl_anlz::Existing::default(),
    );
    vec![("DAT".into(), files.dat), ("EXT".into(), files.ext), ("2EX".into(), files.two_ex)]
}

#[derive(Default)]
struct Tags {
    title: String,
    artist: String,
    album: String,
    genre: String,
    label: String,
    comment: String,
    isrc: String,
    year: u16,
    track_number: u32,
    disc_number: u16,
    bitrate: u32,
    sample_rate: u32,
    bit_depth: u16,
    cover: Option<Vec<u8>>,
}

fn read_tags(path: &Path) -> Tags {
    let mut tags = Tags::default();
    let Ok(file) = lofty::read_from_path(path) else { return tags };
    let props = file.properties();
    tags.bitrate = props.audio_bitrate().unwrap_or(0);
    tags.sample_rate = props.sample_rate().unwrap_or(0);
    tags.bit_depth = props.bit_depth().map_or(0, u16::from);
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else { return tags };
    let text = |v: Option<std::borrow::Cow<'_, str>>| v.map(|s| s.trim().to_owned()).unwrap_or_default();
    let item = |k: &ItemKey| tag.get_string(k).map(|s| s.trim().to_owned()).unwrap_or_default();
    tags.title = text(tag.title());
    tags.artist = text(tag.artist());
    tags.album = text(tag.album());
    tags.genre = text(tag.genre());
    tags.comment = text(tag.comment());
    tags.label = [ItemKey::Label, ItemKey::Publisher].iter().map(item).find(|s| !s.is_empty()).unwrap_or_default();
    tags.isrc = item(&ItemKey::Isrc);
    tags.year = [ItemKey::Year, ItemKey::RecordingDate, ItemKey::ReleaseDate]
        .iter()
        .map(item)
        .find_map(|s| s.get(..4).and_then(|y| y.parse().ok()))
        .unwrap_or(0);
    tags.track_number = tag.track().unwrap_or(0);
    tags.disc_number = tag.disk().and_then(|d| u16::try_from(d).ok()).unwrap_or(0);
    let pictures = tag.pictures();
    tags.cover = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())
        .map(|p| p.data().to_vec());
    tags
}

/// Writes a cover where `rbl_export` looks for it: the image, with its small
/// and medium versions beside it as `artwork_s.jpg` and `artwork_m.jpg`.
fn stage_cover(dir: &Path, data: &[u8]) -> Result<PathBuf> {
    let image = image::load_from_memory(data).map_err(|e| format!("cover: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for (name, size) in [("artwork_s.jpg", ARTWORK_SMALL), ("artwork_m.jpg", ARTWORK_MEDIUM)] {
        image
            .resize_to_fill(size, size, image::imageops::FilterType::Lanczos3)
            .to_rgb8()
            .save_with_format(dir.join(name), image::ImageFormat::Jpeg)
            .map_err(|e| format!("cover: {e}"))?;
    }
    let full = dir.join("cover.jpg");
    image.to_rgb8().save_with_format(&full, image::ImageFormat::Jpeg).map_err(|e| format!("cover: {e}"))?;
    Ok(full)
}

fn audio_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        // metadata() follows symlinks, so linked files and folders count.
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_dir() {
            audio_files(&path, out)?;
        } else if path.extension().is_some_and(|e| AUDIO.contains(&e.to_string_lossy().to_lowercase().as_str())) {
            out.push(path);
        }
    }
    Ok(())
}

/// A stable, non-zero id for a source file: a second export to the same
/// stick recognises a track by it.
fn track_id(path: &Path) -> u64 {
    // FNV-1a, 53 bits so it survives a round trip through JSON numbers.
    let hash = path.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
    (hash & ((1 << 53) - 1)).max(1)
}

fn root_for(destination: &Path, root: Root) -> Option<rbl_export::ExportRoot> {
    match root {
        Root::Standard => Some(rbl_export::ExportRoot::Standard),
        Root::Hidden => Some(rbl_export::ExportRoot::Hidden),
        Root::Auto => rbl_export::ExportRoot::for_file_system(&file_system(destination)?),
    }
}

/// The file system type of the volume holding `path`, as the OS names it.
#[cfg(target_os = "macos")]
fn file_system(path: &Path) -> Option<String> {
    // `diskutil info` names the type as "Type (Bundle): hfs".
    let out = std::process::Command::new("diskutil").arg("info").arg(path).output().ok()?;
    let info = String::from_utf8_lossy(&out.stdout);
    info.lines().find_map(|l| l.trim().strip_prefix("Type (Bundle):")).map(|t| t.trim().to_owned())
}

/// The file system type of the volume holding `path`, as the OS names it.
#[cfg(not(target_os = "macos"))]
fn file_system(path: &Path) -> Option<String> {
    // The longest mount point that contains the path: "hfsplus", "vfat", ...
    let path = path.canonicalize().ok()?;
    let mounts = std::fs::read_to_string("/proc/self/mounts").ok()?;
    mounts
        .lines()
        .filter_map(|l| {
            let mut fields = l.split_whitespace();
            let (_, mount_point, fs) = (fields.next()?, fields.next()?, fields.next()?);
            // /proc/mounts escapes spaces and other separators as octal.
            let mount_point = mount_point.replace("\\040", " ").replace("\\011", "\t").replace("\\134", "\\");
            path.starts_with(&mount_point).then(|| (mount_point.len(), fs.to_owned()))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, fs)| fs)
}

/// A track's analysis for an export, from the cache when it holds one.
/// The elapsed time is None for a cache hit.
fn analyse_for_export(path: &Path, preset: Preset, cache_root: Option<&Path>, today: &str) -> Result<(cache::Entry, Option<u128>)> {
    let cached = cache_root.and_then(|root| cache::folder(root, path, preset.name()));
    if let Some(entry) = cached.as_deref().and_then(cache::load) {
        return Ok((entry, None));
    }
    let a = analyse(path, preset)?;
    let entry = cache::Entry {
        bpm_x100: bpm_x100(&a.analysis),
        key: key_name(&a.analysis, KeyNotation::Classic),
        camelot: key_name(&a.analysis, KeyNotation::Camelot),
        duration_secs: a.duration_secs,
        date_added: today.to_owned(),
        files: analysis_files(path, &a.analysis),
    };
    if let Some(dir) = cached {
        if let Err(e) = cache::store(&dir, &entry) {
            eprintln!("  {}: {e}", path.display());
        }
    }
    Ok((entry, Some(a.elapsed_ms)))
}

fn export(
    source: &Path,
    destination: &Path,
    playlist: Option<&str>,
    preset: Preset,
    notation: KeyNotation,
    root: Root,
    cache_root: Option<&Path>,
    cue_store: Option<&Path>,
) -> Result<()> {
    let mut files = Vec::new();
    audio_files(source, &mut files)?;
    files.sort();
    if files.is_empty() {
        return Err(format!("no audio files under {}", source.display()));
    }
    if !destination.is_dir() {
        return Err(format!("{} is not a folder", destination.display()));
    }
    let staging = tempfile::tempdir().map_err(|e| e.to_string())?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let source_name = source.file_name().map_or_else(|| "rbx-cli".to_owned(), |n| n.to_string_lossy().into_owned());
    let mut cues = cues::Cues::new(destination, cue_store)?;

    let mut tracks = Vec::new();
    let mut playlists: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut failed = Vec::new();
    for (n, path) in files.iter().enumerate() {
        let (a, elapsed_ms) = match analyse_for_export(path, preset, cache_root, &today) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("[{}/{}] skipped: {e}", n + 1, files.len());
                failed.push(json!({"path": path, "error": e}));
                continue;
            }
        };
        let tags = read_tags(path);
        let artwork = match tags.cover.as_deref().map(|c| stage_cover(&staging.path().join(n.to_string()), c)) {
            Some(Ok(p)) => Some(p),
            Some(Err(e)) => {
                eprintln!("  {}: {e}", path.display());
                None
            }
            None => None,
        };
        let key = match notation {
            KeyNotation::Classic => a.key,
            KeyNotation::Camelot => a.camelot,
        };
        let title = if tags.title.is_empty() {
            path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            tags.title.clone()
        };
        eprintln!(
            "[{}/{}] {} - {}: {:.2} BPM, {}, {}",
            n + 1,
            files.len(),
            tags.artist,
            title,
            f64::from(a.bpm_x100) / 100.0,
            if key.is_empty() { "no key" } else { &key },
            elapsed_ms.map_or_else(|| "cached".to_owned(), |ms| format!("{:.1}s", ms as f64 / 1000.0))
        );
        let folder = path.parent().and_then(|p| p.strip_prefix(source).ok()).map(|p| p.to_string_lossy().into_owned());
        let name = match (playlist, folder) {
            (Some(name), _) => name.to_owned(),
            (None, Some(f)) if !f.is_empty() => f,
            _ => source_name.clone(),
        };
        playlists.entry(name).or_default().push(tracks.len());
        let id = track_id(path);
        tracks.push(rbl_export::SourceTrack {
            id,
            cues: cues.for_track(path, id)?,
            source_path: path.clone(),
            title,
            artist: tags.artist,
            album: tags.album,
            genre: tags.genre,
            label: tags.label,
            key,
            comment: tags.comment,
            date_added: a.date_added,
            bpm_x100: a.bpm_x100,
            duration_sec: u16::try_from(a.duration_secs.round() as u64).unwrap_or(u16::MAX),
            year: tags.year,
            bitrate: tags.bitrate,
            sample_rate: tags.sample_rate,
            file_size: std::fs::metadata(path).map_or(0, |m| m.len()),
            analysis: a.files,
            artwork,
            metadata: rbl_core::ExportMetadata {
                track_number: tags.track_number,
                disc_number: tags.disc_number,
                bit_depth: tags.bit_depth,
                // What rbxport records for a track it analysed in full.
                analysed: 105,
                hot_cue_auto_load: true,
                isrc: tags.isrc,
                ..Default::default()
            },
            ..Default::default()
        });
    }

    let playlists: Vec<rbl_export::SourcePlaylist> = playlists
        .into_iter()
        .enumerate()
        .map(|(i, (name, track_indices))| rbl_export::SourcePlaylist {
            id: i as u64 + 1,
            name,
            track_indices,
            ..Default::default()
        })
        .collect();
    for path in &cues.from_player {
        eprintln!("cues changed on a player, kept: {path}");
    }
    write_export(destination, &tracks, &playlists, root, failed)
}

/// Writes the library with `rbl_export`, verifies it, and prints the report.
fn write_export(
    destination: &Path,
    tracks: &[rbl_export::SourceTrack],
    playlists: &[rbl_export::SourcePlaylist],
    root: Root,
    not_analysed: Vec<serde_json::Value>,
) -> Result<()> {
    let options = rbl_export::ExportOptions { root: root_for(destination, root), ..Default::default() };
    let mut last_stage = "";
    let report = rbl_export::export_with_options(destination, tracks, playlists, &[], &options, &mut |p| {
        if p.stage != last_stage {
            eprintln!("export: {}", p.stage);
            last_stage = p.stage;
        }
    })
    .map_err(|e| e.to_string())?;
    let check = rbl_export::verify(destination).map_err(|e| e.to_string())?;
    println!(
        "{}",
        json!({
            "destination": destination,
            "root": rbl_export::export_root_name(destination).unwrap_or("?"),
            "tracks": report.tracks,
            "playlists": report.playlists,
            "analysis_files": report.analysis_files,
            "artwork_files": report.artwork_files,
            "bytes_copied": report.bytes_copied,
            "pdb_bytes": report.pdb_bytes,
            "one_library": report.one_library,
            "verified": report.verified,
            "verify_ok": check.is_ok(),
            "hot_cues": check.hot_cues,
            "memory_cues": check.memory_cues,
            "skipped": report.skipped,
            "failed": report.failed,
            "warnings": report.warnings.iter().map(|w| json!({"title": w.title, "reason": w.reason, "detail": w.detail})).collect::<Vec<_>>(),
            "not_analysed": not_analysed,
            "elapsed_ms": report.elapsed_ms,
        })
    );
    Ok(())
}
