//! `rebuild`: a rekordbox USB written again by rbx-cli.
//!
//! Takes from the source stick what a DJ curated (every track, the playlist
//! tree with its folders and order, titles and the rest of the metadata,
//! ratings, colours, covers, hot and memory cues) and lets rbxport redo what
//! an analysis produces (beat grid, key, waveforms).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rbl_pdb::{PageType, Pdb};
use serde_json::json;

use crate::usb::{on_usb, read_pdb};
use crate::{analyse, analysis_files, bpm_x100, key_name, read_tags, stage_cover, write_export, KeyNotation, Preset, Result, Root};

/// rekordbox's cover for a track, staged where `rbl_export` looks for its
/// small and medium versions: `a<id>.jpg` is 80×80 and `a<id>_m.jpg` 240×240.
fn stage_artwork(source: &Path, path: &str, dir: &Path) -> Option<PathBuf> {
    let small = on_usb(source, path);
    let medium = small.with_file_name(format!("{}_m.jpg", small.file_stem()?.to_string_lossy()));
    std::fs::create_dir_all(dir).ok()?;
    let image = dir.join("cover.jpg");
    std::fs::copy(if medium.is_file() { &medium } else { &small }, &image).ok()?;
    std::fs::copy(&small, dir.join("artwork_s.jpg")).ok()?;
    if medium.is_file() {
        std::fs::copy(&medium, dir.join("artwork_m.jpg")).ok()?;
    }
    Some(image)
}

pub fn rebuild(source: &Path, destination: &Path, preset: Preset, notation: KeyNotation, root: Root, limit: Option<usize>) -> Result<()> {
    if source.canonicalize().ok() == destination.canonicalize().ok() {
        return Err("the destination must be another volume or folder".into());
    }
    if !destination.is_dir() {
        return Err(format!("{} is not a folder", destination.display()));
    }
    let bytes = read_pdb(source)?;
    let pdb = Pdb::parse(&bytes).map_err(|e| e.to_string())?;
    let names = |kind| -> HashMap<u32, String> {
        pdb.table(kind).map(|t| pdb.named_rows(t).into_iter().map(|r| (r.id, r.name)).collect()).unwrap_or_default()
    };
    let (artists, albums, genres, labels, artwork) = (
        names(PageType::Artists),
        names(PageType::Albums),
        names(PageType::Genres),
        names(PageType::Labels),
        names(PageType::Artwork),
    );
    let name = |map: &HashMap<u32, String>, id: u32| map.get(&id).cloned().unwrap_or_default();
    let mut rows = pdb.table(PageType::Tracks).map(|t| pdb.track_rows(t)).unwrap_or_default();
    rows.truncate(limit.unwrap_or(usize::MAX));
    let staging = tempfile::tempdir().map_err(|e| e.to_string())?;

    let mut tracks = Vec::new();
    let mut index_of: HashMap<u32, usize> = HashMap::new();
    let mut failed = Vec::new();
    for (n, row) in rows.iter().enumerate() {
        let audio = on_usb(source, &row.file_path);
        let a = match analyse(&audio, preset) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("[{}/{}] skipped: {e}", n + 1, rows.len());
                failed.push(json!({"path": row.file_path, "error": e}));
                continue;
            }
        };
        let stage = staging.path().join(n.to_string());
        let artwork = artwork
            .get(&row.artwork_id)
            .and_then(|path| stage_artwork(source, path, &stage))
            .or_else(|| read_tags(&audio).cover.and_then(|c| stage_cover(&stage, &c).ok()));
        let cues = crate::cues::read(&on_usb(source, &row.analyze_path));
        let key = key_name(&a.analysis, notation);
        eprintln!(
            "[{}/{}] {} - {}: {:.2} BPM (rekordbox {:.2}), {}, {} cues, {:.1}s",
            n + 1,
            rows.len(),
            name(&artists, row.artist_id),
            row.title,
            a.analysis.tempo.bpm,
            f64::from(row.tempo_x100) / 100.0,
            if key.is_empty() { "no key" } else { &key },
            cues.len(),
            a.elapsed_ms as f64 / 1000.0
        );
        index_of.insert(row.id, tracks.len());
        tracks.push(rbl_export::SourceTrack {
            id: u64::from(row.id),
            source_path: audio.clone(),
            title: row.title.clone(),
            artist: name(&artists, row.artist_id),
            album: name(&albums, row.album_id),
            genre: name(&genres, row.genre_id),
            label: name(&labels, row.label_id),
            key,
            comment: row.comment.clone(),
            date_added: row.date_added.clone(),
            release_date: row.release_date.clone(),
            bpm_x100: bpm_x100(&a.analysis),
            duration_sec: u16::try_from(a.duration_secs.round() as u64).unwrap_or(row.duration_sec),
            rating: row.rating,
            color_id: row.color_id,
            year: row.year,
            bitrate: row.bitrate,
            sample_rate: row.sample_rate,
            file_size: std::fs::metadata(&audio).map_or(u64::from(row.file_size), |m| m.len()),
            analysis: analysis_files(&audio, &a.analysis),
            artwork,
            cues: Some(cues),
            metadata: rbl_core::ExportMetadata {
                track_number: row.track_number,
                disc_number: row.disc_number,
                bit_depth: row.sample_depth,
                play_count: u32::from(row.play_count),
                // What rbxport records for a track it analysed in full.
                analysed: 105,
                hot_cue_auto_load: row.hot_cue_auto_load,
                ..Default::default()
            },
            ..Default::default()
        });
    }

    // Folders and playlists in tree order, so that each one's position among
    // its siblings (rbl_export numbers them in the order given) is rekordbox's.
    let tree = pdb.table(PageType::PlaylistTree).map(|t| pdb.playlist_nodes(t)).unwrap_or_default();
    let mut entries = pdb.table(PageType::PlaylistEntries).map(|t| pdb.playlist_entries(t)).unwrap_or_default();
    entries.sort_by_key(|e| (e.playlist_id, e.entry_index));
    let mut ordered = Vec::new();
    fn walk<'t>(tree: &'t [rbl_pdb::PlaylistNode], parent: u32, out: &mut Vec<&'t rbl_pdb::PlaylistNode>) {
        let mut children: Vec<_> = tree.iter().filter(|n| n.parent_id == parent).collect();
        children.sort_by_key(|n| n.sort_order);
        for node in children {
            out.push(node);
            walk(tree, node.id, out);
        }
    }
    walk(&tree, 0, &mut ordered);
    let playlists: Vec<rbl_export::SourcePlaylist> = ordered
        .into_iter()
        .map(|node| rbl_export::SourcePlaylist {
            id: u64::from(node.id),
            name: node.name.clone(),
            parent_id: u64::from(node.parent_id),
            folder: node.is_folder,
            track_indices: entries
                .iter()
                .filter(|e| e.playlist_id == node.id)
                .filter_map(|e| index_of.get(&e.track_id).copied())
                .collect(),
            ..Default::default()
        })
        .collect();
    eprintln!(
        "{} tracks, {} folders and playlists, {} hot/memory cues from rekordbox",
        tracks.len(),
        playlists.len(),
        tracks.iter().map(|t| t.cues.as_ref().map_or(0, Vec::len)).sum::<usize>()
    );
    write_export(destination, &tracks, &playlists, root, failed)
}
