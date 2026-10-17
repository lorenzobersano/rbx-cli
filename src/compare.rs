//! Comparisons with what rekordbox wrote on a USB: `compare-grid` (tempo,
//! phase, downbeat, key) and `compare-waveform`.
//!
//! Ported from the comparison tools of an earlier Python prototype, with
//! rbxport's analysis in place of its own. Tracks are spread evenly over
//! the library (after --skip), or named with --paths.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rbl_pdb::{PageType, Pdb, TrackRow};
use serde_json::json;

use crate::usb::{on_usb, read_pdb};
use crate::{analyse, analysis_files, Preset};

fn library_tracks(root: &Path) -> Result<(Vec<TrackRow>, HashMap<u32, String>), String> {
    let bytes = read_pdb(root)?;
    let pdb = Pdb::parse(&bytes).map_err(|e| e.to_string())?;
    let rows = pdb.table(PageType::Tracks).map(|t| pdb.track_rows(t)).unwrap_or_default();
    let keys = pdb
        .table(PageType::Keys)
        .map(|t| pdb.named_rows(t).into_iter().map(|r| (r.id, r.name)).collect())
        .unwrap_or_default();
    Ok((rows, keys))
}

/// The tracks to compare: those named by `paths` (files on the USB, or links
/// to them), or `count` spread evenly over the candidates after `skip`.
fn sample(root: &Path, candidates: Vec<TrackRow>, count: usize, skip: usize, paths: &[PathBuf]) -> Result<Vec<TrackRow>, String> {
    if paths.is_empty() {
        let step = (candidates.len() / count.max(1)).max(1);
        return Ok(candidates.into_iter().skip(skip).step_by(step).take(count).collect());
    }
    let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    let by_path: HashMap<String, TrackRow> = candidates.into_iter().map(|t| (t.file_path.clone(), t)).collect();
    paths
        .iter()
        .map(|p| {
            let real = p.canonicalize().map_err(|e| format!("{}: {e}", p.display()))?;
            let rel = real.strip_prefix(&root).map_err(|_| format!("{} is not on {}", real.display(), root.display()))?;
            let key = format!("/{}", rel.to_string_lossy());
            by_path.get(&key).cloned().ok_or_else(|| format!("{key}: not an analysed track of the library"))
        })
        .collect()
}

fn classify(bpm: f64, reference: f64) -> &'static str {
    if (bpm - reference).abs() < 0.011 {
        return "exact";
    }
    if (bpm - reference).abs() < 0.1 {
        return "close";
    }
    let ratio = bpm / reference;
    [("double", 2.0), ("half", 0.5), ("4/3", 4.0 / 3.0), ("3/4", 0.75), ("3/2", 1.5), ("2/3", 2.0 / 3.0)]
        .iter()
        .find(|(_, r)| (ratio - r).abs() < 0.01)
        .map_or("wrong", |(name, _)| name)
}

/// A key name in either notation, as a Camelot code (`Am` and `8A` → `8A`).
pub fn camelot(name: &str) -> Option<String> {
    const WHEEL: [(&str, &str); 24] = [
        ("Abm", "1A"), ("Ebm", "2A"), ("Bbm", "3A"), ("Fm", "4A"), ("Cm", "5A"), ("Gm", "6A"),
        ("Dm", "7A"), ("Am", "8A"), ("Em", "9A"), ("Bm", "10A"), ("F#m", "11A"), ("Dbm", "12A"),
        ("B", "1B"), ("F#", "2B"), ("Db", "3B"), ("Ab", "4B"), ("Eb", "5B"), ("Bb", "6B"),
        ("F", "7B"), ("C", "8B"), ("G", "9B"), ("D", "10B"), ("A", "11B"), ("E", "12B"),
    ];
    const ENHARMONIC: [(&str, &str); 10] =
        [("G#", "Ab"), ("D#", "Eb"), ("A#", "Bb"), ("Gb", "F#"), ("C#", "Db"), ("Cb", "B"), ("Fb", "E"), ("E#", "F"), ("B#", "C"), ("♯", "#")];
    let name = name.trim();
    let upper = name.to_uppercase();
    if let Some(number) = upper.strip_suffix('A').or_else(|| upper.strip_suffix('B')) {
        if number.parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) {
            return Some(upper);
        }
    }
    let (root, minor) = name.strip_suffix('m').map_or((name, false), |r| (r, true));
    let root = ENHARMONIC.iter().find(|(from, _)| *from == root).map_or(root, |(_, to)| to);
    let classic = if minor { format!("{root}m") } else { root.to_owned() };
    WHEEL.iter().find(|(c, _)| *c == classic).map(|(_, code)| (*code).to_owned())
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = q * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

pub fn compare_grid(root: &Path, count: usize, skip: usize, paths: &[PathBuf], preset: Preset, json_out: bool, verbose: bool) -> Result<(), String> {
    let (rows, keys) = library_tracks(root)?;
    let candidates = rows.into_iter().filter(|t| on_usb(root, &t.analyze_path).is_file()).collect();
    let tracks = sample(root, candidates, count, skip, paths)?;
    let started = Instant::now();
    let (mut kinds, mut bars): (BTreeMap<&str, usize>, BTreeMap<i64, usize>) = (BTreeMap::new(), BTreeMap::new());
    let (mut phase_errors, mut grid_ok, mut double_ok, mut key_ok, mut key_known, mut analysed) = (Vec::new(), 0, 0, 0, 0, 0);
    for track in &tracks {
        let Some(grid) = rbl_anlz::Anlz::read(&on_usb(root, &track.analyze_path)).ok().and_then(|a| a.beat_grid()) else { continue };
        let (Some(first), Some(theirs_down)) = (grid.first(), grid.iter().find(|b| b.beat_number == 1)) else { continue };
        let a = match analyse(&on_usb(root, &track.file_path), preset) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("{e}");
                continue;
            }
        };
        analysed += 1;
        let reference = f64::from(first.tempo_x100) / 100.0;
        let bpm = a.analysis.tempo.bpm;
        let kind = classify(bpm, reference);
        *kinds.entry(kind).or_default() += 1;
        let ours = a.analysis.tempo.beats.first();
        let (mut phase, mut bar) = (None, None);
        if let (Some(ours), true) = (ours, matches!(kind, "exact" | "close" | "double")) {
            // A double-time grid has a beat on each of rekordbox's and one between.
            let period = 60_000.0 / if kind == "double" { bpm } else { reference };
            let error = (f64::from(ours.time_ms) - f64::from(first.time_ms) + period / 2.0).rem_euclid(period) - period / 2.0;
            phase = Some(error);
            phase_errors.push(error.abs());
            // Arguably right for drum and bass: every rekordbox beat is on a beat of ours.
            if kind == "double" && error.abs() <= 25.0 {
                double_ok += 1;
            }
            if kind != "double" {
                let ours_down = f64::from(ours.time_ms) + f64::from((1 + 4 - ours.beat_number % 4) % 4) * period;
                let off = ((ours_down - f64::from(theirs_down.time_ms)) / period).round() as i64;
                let off = off.rem_euclid(4);
                *bars.entry(off).or_default() += 1;
                bar = Some(off);
                if kind == "exact" && error.abs() <= 25.0 && off == 0 {
                    grid_ok += 1;
                }
            }
        }
        let theirs_key = keys.get(&track.key_id).cloned();
        let ours_key = a.analysis.key.as_ref().map(rbl_analysis::key::MusicalKey::camelot);
        let same_key = theirs_key.as_deref().and_then(camelot).zip(ours_key.clone()).map(|(t, o)| t == o);
        if let Some(same) = same_key {
            key_known += 1;
            key_ok += usize::from(same);
        }
        if json_out {
            println!(
                "{}",
                json!({"title": track.title, "file": track.file_path, "rekordbox_bpm": reference, "bpm": bpm, "kind": kind,
                       "phase_ms": phase, "bar_offset": bar, "rekordbox_key": theirs_key, "key": ours_key, "same_key": same_key})
            );
        } else if verbose || kind != "exact" || bar != Some(0) || phase.is_some_and(|e| e.abs() > 25.0) || same_key == Some(false) {
            let mut line = format!("{:36} rekordbox {reference:7.2}  ours {bpm:7.2}  {kind}", track.title.chars().take(36).collect::<String>());
            if let Some(e) = phase {
                line += &format!("  phase {e:+6.1} ms");
            }
            if let Some(b) = bar {
                line += &if b == 0 { "  bar ok".to_owned() } else { format!("  bar off by {b}") };
            }
            line += &format!(
                "  key {} vs {}",
                ours_key.as_deref().unwrap_or("-"),
                theirs_key.as_deref().unwrap_or("-")
            );
            println!("{line}");
        }
    }
    phase_errors.sort_by(f64::total_cmp);
    let within = |ms: f64| phase_errors.iter().filter(|e| **e <= ms).count() as f64 / phase_errors.len().max(1) as f64 * 100.0;
    let summary = json!({
        "tracks": analysed,
        "seconds_each": started.elapsed().as_secs_f64() / analysed.max(1) as f64,
        "bpm": kinds,
        "grid_ok": grid_ok,
        "double_time_ok": double_ok,
        "phase_p50_ms": percentile(&phase_errors, 0.5),
        "phase_p90_ms": percentile(&phase_errors, 0.9),
        "phase_within_10ms_pct": within(10.0),
        "phase_within_25ms_pct": within(25.0),
        "bar_offsets": bars.iter().map(|(k, v)| (k.to_string(), *v)).collect::<BTreeMap<_, _>>(),
        "same_key": key_ok,
        "keys_compared": key_known,
    });
    if json_out {
        println!("{}", json!({"summary": summary}));
    } else {
        println!("\n{analysed} tracks, {:.1} s each", summary["seconds_each"].as_f64().unwrap_or(0.0));
        println!("BPM: {}", kinds.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "));
        println!(
            "phase (BPM right or double): |error| p50 {:.1} ms, p90 {:.1} ms, within 10 ms {:.0}%, within 25 ms {:.0}%",
            percentile(&phase_errors, 0.5),
            percentile(&phase_errors, 0.9),
            within(10.0),
            within(25.0)
        );
        println!(
            "downbeat: {}",
            bars.iter().map(|(k, v)| if *k == 0 { format!("ok {v}") } else { format!("off by {k} {v}") }).collect::<Vec<_>>().join(", ")
        );
        println!("grid as rekordbox (exact BPM, phase within 25 ms, bar ok): {grid_ok}/{analysed}");
        println!("double time, phase within 25 ms (right for drum and bass): {double_ok}");
        println!("key as rekordbox: {key_ok}/{key_known}");
    }
    Ok(())
}

/// Per-format channels, most significant first: (name, shift, bits) for the
/// packed formats; PWV4 and the rest have one byte per channel.
fn channels(tag: &str, stride: usize) -> Vec<(String, u32, u32)> {
    let packed: &[(&str, u32, u32)] = match tag {
        "PWAV" | "PWV3" => &[("whiteness", 5, 3), ("height", 0, 5)],
        "PWV2" => &[("height", 0, 4)],
        "PWV5" => &[("red", 13, 3), ("green", 10, 3), ("blue", 7, 3), ("height", 2, 5)],
        _ => &[],
    };
    if packed.is_empty() {
        (0..stride).map(|i| (format!("d{i}"), 0, 8)).collect()
    } else {
        packed.iter().map(|(n, s, b)| ((*n).to_owned(), *s, *b)).collect()
    }
}

/// One row per column, one value per channel.
fn decode(section: &rbl_anlz::Section) -> Option<(Vec<String>, Vec<Vec<u8>>)> {
    let tag = section.tag.as_str();
    let stride = section.waveform_stride()? as usize;
    let layout = channels(tag, stride);
    let names = layout.iter().map(|(n, _, _)| n.clone()).collect();
    let columns = section
        .payload
        .chunks_exact(stride)
        .map(|entry| match tag {
            "PWAV" | "PWV2" | "PWV3" | "PWV5" => {
                let raw = entry.iter().fold(0_u32, |v, b| v << 8 | u32::from(*b));
                layout.iter().map(|(_, shift, bits)| ((raw >> shift) & ((1 << bits) - 1)) as u8).collect()
            }
            _ => entry.to_vec(),
        })
        .collect();
    Some((names, columns))
}

const WAVEFORMS: [&str; 7] = ["PWV5", "PWV4", "PWV3", "PWAV", "PWV2", "PWV6", "PWV7"];

fn sections(files: &[(String, Vec<u8>)]) -> HashMap<String, rbl_anlz::Section> {
    files
        .iter()
        .filter_map(|(_, bytes)| rbl_anlz::parse(bytes).ok())
        .flat_map(|a| a.sections)
        .map(|s| (s.tag.as_str().to_owned(), s))
        .collect()
}

fn draw_detail(columns: &[Vec<u8>], start: usize, width: usize) -> image::RgbImage {
    let height = 64_u32;
    let mut img = image::RgbImage::from_pixel(width as u32 * 2, height, image::Rgb([20, 20, 20]));
    for (x, c) in columns.iter().skip(start).take(width).enumerate() {
        let k = u32::from(c[3]) * (height / 2) / 31;
        for y in (height / 2).saturating_sub(k)..=(height / 2 + k).min(height - 1) {
            for dx in 0..2 {
                img.put_pixel(x as u32 * 2 + dx, y, image::Rgb([c[0] * 36, c[1] * 36, c[2] * 36]));
            }
        }
    }
    img
}

fn draw_preview(columns: &[Vec<u8>], width: usize) -> image::RgbImage {
    let height = 64_u32;
    let mut img = image::RgbImage::from_pixel(width as u32, height, image::Rgb([20, 20, 20]));
    for (x, c) in columns.iter().take(width).enumerate() {
        let k = u32::from(c[0]) * (height - 1) / 127;
        let m = u32::from(c[3].max(c[4]).max(c[5]).max(1));
        let colour = |v: u8| (u32::from(v) * 255 / m) as u8;
        for y in (height - 1 - k)..height {
            img.put_pixel(x as u32, y, image::Rgb([colour(c[3]), colour(c[4]), colour(c[5])]));
        }
    }
    img
}

/// rekordbox's PWV4 above ours, then 20 s of PWV5 (rekordbox above, ours below).
fn comparison_image(ours: &HashMap<String, (Vec<String>, Vec<Vec<u8>>)>, theirs: &HashMap<String, (Vec<String>, Vec<Vec<u8>>)>) -> Option<image::RgbImage> {
    const WIDTH: u32 = 1200;
    let (theirs4, ours4) = (&theirs.get("PWV4")?.1, &ours.get("PWV4")?.1);
    let (theirs5, ours5) = (&theirs.get("PWV5")?.1, &ours.get("PWV5")?.1);
    let start = theirs5.len() / 3;
    let parts = [
        (draw_preview(theirs4, WIDTH as usize), 6),
        (draw_preview(ours4, WIDTH as usize), 18),
        (draw_detail(theirs5, start, WIDTH as usize / 2), 6),
        (draw_detail(ours5, start, WIDTH as usize / 2), 0),
    ];
    let height = parts.iter().map(|(p, gap)| p.height() + gap).sum();
    let mut img = image::RgbImage::from_pixel(WIDTH, height, image::Rgb([60, 60, 60]));
    let mut y = 0;
    for (part, gap) in &parts {
        image::imageops::replace(&mut img, part, 0, i64::from(y));
        y += part.height() + gap;
    }
    Some(img)
}

pub fn compare_waveform(root: &Path, count: usize, skip: usize, paths: &[PathBuf], preset: Preset, png: Option<&Path>) -> Result<(), String> {
    let (rows, _) = library_tracks(root)?;
    let candidates = rows
        .into_iter()
        .filter(|t| rbl_anlz::sibling(&on_usb(root, &t.analyze_path), "EXT").is_file())
        .collect();
    let tracks = sample(root, candidates, count, skip, paths)?;
    if let Some(dir) = png {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    // (format, channel) in print order → (format, channel name, per-track errors)
    let mut totals: BTreeMap<(usize, usize), (String, String, Vec<f64>)> = BTreeMap::new();
    for track in &tracks {
        let dat = on_usb(root, &track.analyze_path);
        let theirs_files: Vec<(String, Vec<u8>)> = ["DAT", "EXT", "2EX"]
            .iter()
            .filter_map(|ext| std::fs::read(rbl_anlz::sibling(&dat, ext)).ok().map(|b| ((*ext).to_owned(), b)))
            .collect();
        let audio = on_usb(root, &track.file_path);
        let a = match analyse(&audio, preset) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("{e}");
                continue;
            }
        };
        let decoded = |files: &[(String, Vec<u8>)]| -> HashMap<String, (Vec<String>, Vec<Vec<u8>>)> {
            sections(files).iter().filter_map(|(tag, s)| Some((tag.clone(), decode(s)?))).collect()
        };
        let (theirs, ours) = (decoded(&theirs_files), decoded(&analysis_files(&audio, &a.analysis)));
        let columns = |w: &HashMap<String, (Vec<String>, Vec<Vec<u8>>)>| w.get("PWV5").map_or(0, |(_, c)| c.len());
        let mut summary = Vec::new();
        for (order, tag) in WAVEFORMS.iter().enumerate() {
            let (Some((names, o)), Some((_, t))) = (ours.get(*tag), theirs.get(*tag)) else { continue };
            let n = o.len().min(t.len());
            let errors: Vec<f64> = (0..names.len())
                .map(|ch| (0..n).map(|i| f64::from(o[i][ch].abs_diff(t[i][ch]))).sum::<f64>() / n.max(1) as f64)
                .collect();
            for (ch, (name, e)) in names.iter().zip(&errors).enumerate() {
                totals.entry((order, ch)).or_insert_with(|| ((*tag).to_owned(), name.clone(), Vec::new())).2.push(*e);
            }
            summary.push(format!("{tag} {}", errors.iter().map(|e| format!("{e:.1}")).collect::<Vec<_>>().join("/")));
        }
        let title: String = track.title.chars().take(40).collect();
        println!("{title:40} {:>6} vs {:<6} columns  {}", columns(&ours), columns(&theirs), summary.join("  "));
        if let (Some(dir), Some(img)) = (png, comparison_image(&ours, &theirs)) {
            img.save(dir.join(format!("{}.png", track.id))).map_err(|e| e.to_string())?;
        }
    }
    println!("\nmean absolute error per channel:");
    let mut by_tag: BTreeMap<(usize, String), Vec<String>> = BTreeMap::new();
    for ((order, _), (tag, name, values)) in &totals {
        let mean = values.iter().sum::<f64>() / values.len().max(1) as f64;
        by_tag.entry((*order, tag.clone())).or_default().push(format!("{name} {mean:.2}"));
    }
    for ((_, tag), values) in by_tag {
        println!("  {tag}: {}", values.join(", "));
    }
    Ok(())
}
