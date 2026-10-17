//! Reading and checking a rekordbox USB export: `inspect` and `validate`.
//!
//! Ported from the tools of an earlier Python prototype. Rows are
//! decoded by `rbl_pdb`; the page structure (chains, flags, row counters),
//! which `rbl_pdb` does not expose, is read here.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use rbl_pdb::{PageType, Pdb, TableRef};

pub const LIBRARY_DIRS: [&str; 2] = ["PIONEER", ".PIONEER"];

/// The `PIONEER` or `.PIONEER` folder holding `rekordbox/export.pdb`.
pub fn library_dir(root: &Path) -> Result<PathBuf, String> {
    LIBRARY_DIRS
        .iter()
        .map(|name| root.join(name))
        .find(|dir| dir.join("rekordbox/export.pdb").is_file())
        .ok_or_else(|| format!("no PIONEER/rekordbox/export.pdb under {}", root.display()))
}

/// A file path from the database, under the USB's root.
pub fn on_usb(root: &Path, path: &str) -> PathBuf {
    root.join(path.trim_start_matches('/'))
}

/// `P<3 hex>/<8 hex>`, where the player looks for a file's analysis under
/// `USBANLZ`: a hash of the path's UTF-16 units modulo 200003, and P is bits
/// 0, 2, 6, 7, 9, 13 and 16 of it (docs/format.md, "ANLZ folder"). rbxport has the
/// same function (`rbl_export::analysis_directory`).
pub fn anlz_directory(file_path: &str) -> String {
    let hash = file_path
        .encode_utf16()
        .fold(0_u32, |h, unit| h.wrapping_mul(23_497).wrapping_add(u32::from(unit)).wrapping_mul(37_813).wrapping_add(u32::from(unit)))
        % 200_003;
    let shard = [0, 2, 6, 7, 9, 13, 16].iter().enumerate().fold(0_u32, |p, (i, bit)| p | ((hash >> bit) & 1) << i);
    format!("P{shard:03X}/{hash:08X}")
}

fn u2(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn u4(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub struct Header {
    pub page_size: usize,
    pub next_unused_page: u32,
    pub sequence: u32,
    pub unknown_0: u32,
    pub unknown_10: u32,
    pub tables: Vec<TableHeader>,
}

pub struct TableHeader {
    pub kind: u32,
    pub empty_candidate: u32,
    pub first_page: u32,
    pub last_page: u32,
}

pub struct Page {
    pub page_index: u32,
    pub kind: u32,
    pub next_page: u32,
    pub flags: u8,
    pub num_row_offsets: usize,
    pub num_rows: usize,
    pub present: usize,
}

impl Page {
    pub fn is_data(&self) -> bool {
        self.flags & 0x40 == 0
    }
}

/// The file header and page structure of an `export.pdb`.
pub struct Layout<'a> {
    bytes: &'a [u8],
    pub header: Header,
}

impl<'a> Layout<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        let page_size = u4(bytes, 4) as usize;
        if !(512..=65_536).contains(&page_size) || !page_size.is_power_of_two() {
            return Err(format!("page size {page_size}: not a DeviceSQL file"));
        }
        if u4(bytes, 24) != 0 {
            return Err("unexpected non-zero header gap".into());
        }
        let tables = (0..u4(bytes, 8) as usize)
            .map(|i| 0x1c + 16 * i)
            .map(|at| TableHeader {
                kind: u4(bytes, at),
                empty_candidate: u4(bytes, at + 4),
                first_page: u4(bytes, at + 8),
                last_page: u4(bytes, at + 12),
            })
            .collect();
        let header = Header {
            page_size,
            unknown_0: u4(bytes, 0),
            next_unused_page: u4(bytes, 12),
            unknown_10: u4(bytes, 16),
            sequence: u4(bytes, 20),
            tables,
        };
        Ok(Self { bytes, header })
    }

    pub fn num_pages(&self) -> usize {
        self.bytes.len() / self.header.page_size
    }

    pub fn page(&self, index: u32) -> Page {
        let size = self.header.page_size;
        let p = self.bytes.get(index as usize * size..(index as usize + 1) * size).unwrap_or(&[]);
        let packed = u4(p, 0x18) & 0x00ff_ffff;
        let mut page = Page {
            page_index: u4(p, 4),
            kind: u4(p, 8),
            next_page: u4(p, 0x0c),
            flags: p.get(0x1b).copied().unwrap_or(0),
            num_row_offsets: (packed & 0x1fff) as usize,
            num_rows: (packed >> 13) as usize,
            present: 0,
        };
        if page.is_data() && !p.is_empty() {
            // Row groups of 16 build backwards from the end of the page,
            // each ending in its presence bits.
            page.present = (0..page.num_row_offsets)
                .filter(|i| u2(p, size - (i / 16) * 0x24 - 4) >> (i % 16) & 1 == 1)
                .count();
        }
        page
    }

    /// The table's page chain, from its first page to its last.
    pub fn table_pages(&self, table: &TableHeader) -> Vec<u32> {
        let (mut pages, mut seen, mut index) = (Vec::new(), BTreeSet::new(), table.first_page);
        while (index as usize) < self.num_pages() && seen.insert(index) {
            pages.push(index);
            if index == table.last_page {
                break;
            }
            index = self.page(index).next_page;
        }
        pages
    }
}

/// Table names as crate-digger gives them. `rbl_pdb` calls 17
/// and 18 the history tables and 19 `property`; on a fresh export 17 and 18
/// hold 22 and 17 fixed rows, so they are not the history.
pub fn table_name(kind: u32) -> String {
    const NAMES: [&str; 20] = [
        "tracks", "genres", "artists", "albums", "labels", "keys", "colors", "playlist_tree", "playlist_entries",
        "unknown_9", "unknown_10", "history_playlists", "history_entries", "artwork", "unknown_14", "unknown_15",
        "columns", "unknown_17", "unknown_18", "history",
    ];
    NAMES.get(kind as usize).map_or_else(|| format!("type_{kind}"), |n| (*n).to_owned())
}

#[derive(Default)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Page chains, page indices and row counters.
fn check_structure(layout: &Layout<'_>, report: &mut Report) {
    let mut claimed: HashMap<u32, String> = HashMap::new();
    for table in &layout.header.tables {
        let name = table_name(table.kind);
        let pages = layout.table_pages(table);
        if pages.last() != Some(&table.last_page) {
            report.errors.push(format!("{name}: page chain does not reach last_page {}", table.last_page));
        }
        for index in pages {
            if let Some(other) = claimed.insert(index, name.clone()) {
                report.errors.push(format!("page {index} used by both {other} and {name}"));
            }
            let page = layout.page(index);
            if page.page_index != index {
                report.errors.push(format!("{name}: page {index} says it is page {}", page.page_index));
            }
            if page.kind != table.kind {
                report.errors.push(format!("{name}: page {index} has type {}", table_name(page.kind)));
            }
            if page.is_data() && page.present != page.num_rows {
                report.errors.push(format!("{name}: page {index} num_rows={} but {} present", page.num_rows, page.present));
            }
        }
    }
    if (layout.header.next_unused_page as usize) < layout.num_pages() {
        report
            .errors
            .push(format!("next_unused_page {} < file pages {}", layout.header.next_unused_page, layout.num_pages()));
    }
}

fn ids(pdb: &Pdb<'_>, kind: PageType) -> BTreeSet<u32> {
    pdb.table(kind).map(|t| pdb.named_rows(t).into_iter().map(|r| r.id).collect()).unwrap_or_default()
}

fn tracks<'p>(pdb: &'p Pdb<'_>) -> (Option<&'p TableRef>, Vec<rbl_pdb::TrackRow>) {
    let table = pdb.table(PageType::Tracks);
    (table, table.map(|t| pdb.track_rows(t)).unwrap_or_default())
}

/// Every id a track or playlist points at exists (0 means "none").
fn check_references(pdb: &Pdb<'_>, report: &mut Report) {
    let (table, rows) = tracks(pdb);
    let artists = ids(pdb, PageType::Artists);
    let refs = table.map(|t| pdb.rows(t)).unwrap_or_default();
    let mut dangling: BTreeMap<&str, usize> = BTreeMap::new();
    let targets: [(&str, BTreeSet<u32>); 7] = [
        ("artist_id", artists.clone()),
        ("album_id", ids(pdb, PageType::Albums)),
        ("genre_id", ids(pdb, PageType::Genres)),
        ("key_id", ids(pdb, PageType::Keys)),
        ("label_id", ids(pdb, PageType::Labels)),
        ("artwork_id", ids(pdb, PageType::Artwork)),
        ("color_id", ids(pdb, PageType::Colors)),
    ];
    for (track, row) in rows.iter().zip(&refs) {
        let values = [
            track.artist_id,
            track.album_id,
            track.genre_id,
            track.key_id,
            track.label_id,
            track.artwork_id,
            u32::from(track.color_id),
        ];
        for ((name, ids), value) in targets.iter().zip(values) {
            if value != 0 && !ids.contains(&value) {
                *dangling.entry(name).or_default() += 1;
            }
        }
        // Not decoded by rbl_pdb: composer, original artist, remixer.
        for (name, at) in [("composer_id", 0x0c), ("original_artist_id", 0x24), ("remixer_id", 0x2c)] {
            let value = pdb.u4_at(*row, at);
            if value != 0 && !artists.contains(&value) {
                *dangling.entry(name).or_default() += 1;
            }
        }
    }
    for (name, count) in dangling {
        report.errors.push(format!("{count} tracks have a dangling {name}"));
    }

    let track_ids: BTreeSet<u32> = rows.iter().map(|t| t.id).collect();
    if track_ids.len() != rows.len() {
        report.errors.push("duplicate track ids".into());
    }
    let tree = pdb.table(PageType::PlaylistTree).map(|t| pdb.playlist_nodes(t)).unwrap_or_default();
    let tree_ids: BTreeSet<u32> = tree.iter().map(|n| n.id).collect();
    for node in &tree {
        if node.parent_id != 0 && !tree_ids.contains(&node.parent_id) {
            report.errors.push(format!("playlist {:?} has missing parent {}", node.name, node.parent_id));
        }
    }
    let playlists: BTreeSet<u32> = tree.iter().filter(|n| !n.is_folder).map(|n| n.id).collect();
    let entries = pdb.table(PageType::PlaylistEntries).map(|t| pdb.playlist_entries(t)).unwrap_or_default();
    let bad = entries.iter().filter(|e| !track_ids.contains(&e.track_id) || !playlists.contains(&e.playlist_id)).count();
    if bad > 0 {
        report.errors.push(format!("{bad} playlist entries point at missing tracks/playlists"));
    }
}

/// The analysis files parse, their PPTH names the track's audio file, and
/// they have what the CDJ-2000NXS2 needs (docs/format.md, "What the
/// CDJ-2000NXS2 needs").
fn check_anlz(dat: &Path, file_path: &str, report: &mut Report) {
    for path in [dat.to_path_buf(), rbl_anlz::sibling(dat, "EXT")] {
        if !path.is_file() {
            continue;
        }
        let anlz = match rbl_anlz::Anlz::read(&path) {
            Ok(a) => a,
            Err(e) => {
                report.errors.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let ppth = anlz.path().unwrap_or_default();
        if ppth != file_path {
            report.errors.push(format!("{}: PPTH is {ppth:?}, track file is {file_path:?}", path.display()));
        }
        let has = |tag: &[u8; 4]| anlz.section(tag).is_some();
        let is_ext = path.extension().is_some_and(|e| e == "EXT");
        if !is_ext && !(has(b"PVBR") || has(b"PWAV") || has(b"PWV2")) {
            report.errors.push(format!("{}: no PVBR, PWAV or PWV2, the NXS2 ignores such a .DAT", path.display()));
        }
        if is_ext && !(has(b"PWV4") && has(b"PWV5")) {
            report.warnings.push(format!("{}: no PWV4/PWV5, no colour waveform on the NXS2", path.display()));
        }
    }
}

/// Audio, analysis and artwork files named by the database exist, and each
/// analysis is where the player looks for it.
fn check_files(pdb: &Pdb<'_>, root: &Path, report: &mut Report) {
    let mut missing: BTreeMap<&str, usize> = BTreeMap::new();
    for track in tracks(pdb).1 {
        if !on_usb(root, &track.file_path).is_file() {
            *missing.entry("audio").or_default() += 1;
        }
        let expected = anlz_directory(&track.file_path);
        let parts: Vec<&str> = track.analyze_path.rsplitn(4, '/').collect();
        if parts.len() < 4 || format!("{}/{}", parts[2], parts[1]) != expected {
            report.errors.push(format!(
                "{}: the player expects the analysis of {:?} in USBANLZ/{expected}",
                track.analyze_path, track.file_path
            ));
        }
        let dat = on_usb(root, &track.analyze_path);
        if dat.is_file() {
            check_anlz(&dat, &track.file_path, report);
        } else {
            *missing.entry("ANLZ .DAT").or_default() += 1;
        }
        if !rbl_anlz::sibling(&dat, "EXT").is_file() {
            *missing.entry("ANLZ .EXT").or_default() += 1;
        }
    }
    if let Some(table) = pdb.table(PageType::Artwork) {
        for art in pdb.named_rows(table) {
            if !on_usb(root, &art.name).is_file() {
                *missing.entry("artwork").or_default() += 1;
            }
        }
    }
    for (kind, count) in missing {
        // rekordbox's own exports occasionally lack the .EXT, so it is not fatal.
        let log = if kind == "ANLZ .EXT" { &mut report.warnings } else { &mut report.errors };
        log.push(format!("{count} referenced {kind} files are missing"));
    }
}

pub fn read_pdb(root: &Path) -> Result<Vec<u8>, String> {
    let path = library_dir(root)?.join("rekordbox/export.pdb");
    std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn validate(root: &Path) -> Result<Report, String> {
    let bytes = read_pdb(root)?;
    let layout = Layout::parse(&bytes)?;
    let pdb = Pdb::parse(&bytes).map_err(|e| e.to_string())?;
    let mut report = Report::default();
    check_structure(&layout, &mut report);
    check_references(&pdb, &mut report);
    check_files(&pdb, root, &mut report);
    Ok(report)
}

pub fn print_report(report: &Report) {
    println!("validation: {} errors, {} warnings", report.errors.len(), report.warnings.len());
    for msg in &report.errors {
        println!("  ERROR {msg}");
    }
    for msg in &report.warnings {
        println!("  warn  {msg}");
    }
}

pub fn inspect(root: &Path, limit_tracks: usize, limit_anlz: usize) -> Result<(), String> {
    let dir = library_dir(root)?;
    println!("PIONEER dir: {}", dir.display());
    let bytes = read_pdb(root)?;
    let layout = Layout::parse(&bytes)?;
    let pdb = Pdb::parse(&bytes).map_err(|e| e.to_string())?;
    let h = &layout.header;
    println!("export.pdb: {} pages of {} bytes, {} tables", layout.num_pages(), h.page_size, h.tables.len());
    println!(
        "  sequence={} next_unused_page={} unknown_0={} unknown_10={}",
        h.sequence, h.next_unused_page, h.unknown_0, h.unknown_10
    );
    println!("  {:18} {:>5} {:>5} {:>5} {:>5} {:>6}  page flags", "table", "first", "last", "empty", "pages", "rows");
    for table in &h.tables {
        let pages: Vec<Page> = layout.table_pages(table).into_iter().map(|i| layout.page(i)).collect();
        let rows: usize = pages.iter().map(|p| p.present).sum();
        let mut flags: BTreeMap<String, usize> = BTreeMap::new();
        for p in &pages {
            *flags.entry(format!("0x{:02x}", p.flags)).or_default() += 1;
        }
        println!(
            "  {:18} {:5} {:5} {:5} {:5} {:6}  {flags:?}",
            table_name(table.kind),
            table.first_page,
            table.last_page,
            table.empty_candidate,
            pages.len(),
            rows
        );
    }

    let names = |kind| -> HashMap<u32, String> {
        pdb.table(kind).map(|t| pdb.named_rows(t).into_iter().map(|r| (r.id, r.name)).collect()).unwrap_or_default()
    };
    let (artists, keys) = (names(PageType::Artists), names(PageType::Keys));
    let rows = tracks(&pdb).1;
    println!("\nfirst {limit_tracks} tracks:");
    for t in rows.iter().take(limit_tracks) {
        println!(
            "  #{} {} - {}  [{:.2} BPM, {}, {}s]",
            t.id,
            artists.get(&t.artist_id).map_or("?", String::as_str),
            t.title,
            f64::from(t.tempo_x100) / 100.0,
            keys.get(&t.key_id).map_or("-", String::as_str),
            t.duration_sec
        );
        println!("      file: {}", t.file_path);
        println!("      anlz: {}", t.analyze_path);
    }

    let tree = pdb.table(PageType::PlaylistTree).map(|t| pdb.playlist_nodes(t)).unwrap_or_default();
    let entries = pdb.table(PageType::PlaylistEntries).map(|t| pdb.playlist_entries(t)).unwrap_or_default();
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for e in &entries {
        *counts.entry(e.playlist_id).or_default() += 1;
    }
    println!("\nplaylists ({} nodes):", tree.len());
    fn show(tree: &[rbl_pdb::PlaylistNode], counts: &HashMap<u32, usize>, parent: u32, depth: usize) {
        let mut children: Vec<_> = tree.iter().filter(|n| n.parent_id == parent).collect();
        children.sort_by_key(|n| n.sort_order);
        for node in children {
            let suffix =
                if node.is_folder { "/".to_owned() } else { format!(" ({} tracks)", counts.get(&node.id).unwrap_or(&0)) };
            println!("  {}{}{suffix}", "  ".repeat(depth), node.name);
            show(tree, counts, node.id, depth + 1);
        }
    }
    show(&tree, &counts, 0, 0);

    println!("\nANLZ tags for first {limit_anlz} tracks:");
    for t in rows.iter().take(limit_anlz) {
        println!("  {}", t.title);
        let dat = on_usb(root, &t.analyze_path);
        for ext in ["DAT", "EXT", "2EX"] {
            match rbl_anlz::Anlz::read(&rbl_anlz::sibling(&dat, ext)) {
                Ok(a) => println!("    .{ext}: {}", a.sections.iter().map(|s| s.tag.as_str()).collect::<Vec<_>>().join(" ")),
                Err(_) => println!("    .{ext}: missing"),
            }
        }
    }
    println!();
    print_report(&validate(root)?);
    Ok(())
}
