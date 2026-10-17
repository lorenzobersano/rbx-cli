# rbx-cli

Build rekordbox-compatible USB sticks for Pioneer CDJs from a folder of audio
files, from the command line, without rekordbox.

`rbx-cli` analyses each track (BPM, beat grid, key, waveforms), reads its tags
and embedded cover art, and writes a complete USB library: `export.pdb`,
`exportExt.pdb`, `exportLibrary.db`, the ANLZ analysis files, the artwork and
the audio. It also includes tools to inspect and validate an export, and to
compare its analysis with what rekordbox wrote on another stick.

It is a thin command-line layer over the Rust crates of
[rbxport](https://github.com/chrisle/rbxport), an open-source library manager
inspired by rekordbox. rbxport does the decoding, the analysis and the export;
`rbx-cli` adds the glue that rbxport's desktop app normally provides (reading
tags from the files instead of from rekordbox's `master.db`), plus the
inspection and comparison tools.

> **Status.** Tested on a **CDJ-2000NXS2** with an HFS+ stick (2026-10-17):
> browsing, track title and info, key, artwork, coloured waveforms, beat grid,
> sync, beat jump and loops all work, including on a 1,210-track library
> rebuilt with `rbx-cli rebuild`. Other players and FAT32/exFAT sticks have
> not been tested with `rbx-cli` (rbxport itself targets every player rekordbox
> supports). Use a spare stick and keep backups.

## Install

You need Rust (via [rustup](https://rustup.rs)) and a C compiler (Xcode
Command Line Tools on macOS). The first build downloads rbxport and compiles
SQLCipher and OpenSSL, which takes a couple of minutes.

```sh
git clone https://github.com/lorenzobersano/rbx-cli.git && cd rbx-cli
cargo build --release
# the binary is target/release/rbx-cli
```

rbxport's crates are pinned to the 1.9.0 commit in `Cargo.toml`: they have no
API stability promise, so update the `rev` deliberately.

## Usage

### Export a folder to a stick

```sh
rbx-cli export ~/Music/Saturday /Volumes/USB                 # one playlist per subfolder
rbx-cli export ~/Music/Saturday /Volumes/USB --playlist "Saturday"
```

- Audio: mp3, m4a/mp4 (AAC, ALAC), flac, wav, aiff. Symbolic links to files
  and folders are followed. Files that cannot be decoded are skipped and
  listed in the report.
- Playlists: one per subfolder of the source (named after its relative path);
  files directly in the source folder go into a playlist named after it.
  `--playlist NAME` puts every track in a single playlist.
- Keys are written in Camelot notation (`8A`); `--key classic` writes `Am`.
- `--preset normal` (default) fits one constant tempo per track, like
  rekordbox's Normal analysis; `--preset dynamic` follows tempo changes.
- `--root auto` (default) writes `.PIONEER` on HFS+ volumes and `PIONEER`
  elsewhere, as rekordbox does; `--root standard|hidden` forces one. The file
  system comes from `diskutil` on macOS and from `/proc/self/mounts` on Linux.
- `--cache DIR` keeps each track's analysis in DIR (one folder per track:
  tempo, key, duration, date added and the ANLZ files) and reuses it while the
  file's path, size and modification time are unchanged, so exporting a
  growing library again only analyses the new tracks. A cache hit touches the
  entry's `meta.json`: entries untouched for a while can be deleted by age.
- Hot cues, memory cues and loops set on a player survive the next export.
  A player keeps them in the track's ANLZ files, which an export writes
  again; rbxport's manifest on the stick records a hash of every ANLZ file it
  wrote, so `export` reads the cues back from the files whose bytes no longer
  match and exports them with the track. `rbx-cli player-edits USB` lists
  those tracks without writing anything. Grid edits made on a player are not
  kept.
- `--cues DIR` also keeps each track's cues in DIR (one JSON file per track)
  and exports them to every stick: a cue set on one stick reaches the others
  at their next export, and so does deleting it. A track changed on two
  sticks between exports keeps the cues of the stick exported last.
- Progress goes to stderr; a JSON report (tracks, playlists, files written,
  warnings, verification result) goes to stdout.
- Exporting again to a stick that already holds an rbxport export updates it
  incrementally (rbxport's sync behaviour). Exporting onto a stick written by
  rekordbox or another tool has not been tried with `rbx-cli`: start from an
  empty stick.

### Rebuild a rekordbox stick

```sh
rbx-cli rebuild /Volumes/REKORDBOX /Volumes/USB [--key camelot|classic] [--limit N]
```

Writes a rekordbox stick again, 1:1 in everything you curated: every track,
the playlist tree with its folders, playlist and track order, titles,
artists, albums, genres, labels, comments, ratings, colours, dates, covers and
hot and memory cues all come from the source stick. What an analysis produces
(beat grid, BPM, key, waveforms) is computed again by rbxport. `--limit N`
rebuilds only the first N tracks, to try it quickly.

The source must hold the audio (a rekordbox USB export does). Hot cues that
have no colour in rekordbox get rbxport's default colour for their slot.

### Analyse files

```sh
rbx-cli analyse track1.mp3 track2.m4a [--preset normal|dynamic] [--beats]
```

Prints one JSON object per file: `bpm`, `confidence`, `first_beat_ms`,
`first_beat_number`, `segments`, `key` (classic), `camelot`, `duration_secs`,
`elapsed_ms`, and with `--beats` every beat as `[time_ms, beat_number, bpm×100]`.

### Inspect and validate an export

```sh
rbx-cli inspect /Volumes/USB [--tracks 3] [--anlz 3]
rbx-cli validate /Volumes/USB [--json]
```

`inspect` prints the database header and every table (pages, rows, page
flags), the first tracks, the playlist tree, the ANLZ sections of the first
tracks, and the validation result. `validate` exits with status 1 if it finds
errors. It checks:

- page chains, page indices, page types and row counters of `export.pdb`;
- that every id a track or playlist points at exists;
- that audio, analysis and artwork files exist;
- that each track's analysis lives in the folder the player derives from the
  audio path (players ignore it anywhere else, see [docs/format.md](docs/format.md#anlz-folder));
- that each `.DAT`/`.EXT` parses, names the track's audio file in `PPTH`, and
  holds the sections a CDJ-2000NXS2 needs.

Both work on any rekordbox export, not only on `rbx-cli`'s.

### Compare with rekordbox

Given a stick exported by rekordbox (with its audio), these commands run
rbxport's analysis on the audio and compare it with rekordbox's:

```sh
rbx-cli compare-grid /Volumes/REKORDBOX [--tracks 40] [--skip 3] [--paths FILE...] [--json] [--verbose]
rbx-cli compare-waveform /Volumes/REKORDBOX [--tracks 20] [--skip 0] [--paths FILE...] [--png DIR]
```

- `compare-grid`: BPM (exact, close, double, half, …), phase error of the first
  beat, downbeat offset and key (compared in Camelot, whatever notation the
  stick uses). Double-time grids with the right phase are counted separately:
  for drum and bass they are arguably the right answer.
- `compare-waveform`: mean absolute error per channel for PWAV, PWV2, PWV3,
  PWV4, PWV5, PWV6 and PWV7. `--png` saves one image per track: rekordbox's
  PWV4 overview above ours, then 20 s of PWV5 detail, rekordbox above ours.
- Tracks are spread evenly over the library (after `--skip`), or named with
  `--paths` (files on the stick, or symbolic links to them).

On 20 house, techno and drum and bass tracks, rbxport's grid matched
rekordbox's (same BPM, phase within 25 ms, same downbeat) on 16; one more was
a double-time drum and bass grid. Its key matched rekordbox's on 8 of 20.

## Known limitations

- **Tracks without cover art.** With our earlier writer, a CDJ-2000NXS2 showed
  no title and no track info for a loaded track when the export's `artwork`
  and `keys` tables were empty (browsing still worked; see
  [experiments.md](docs/experiments.md#6-missing-title-and-info-2026-10-17)).
  rbxport writes an empty `artwork` table when no track has an embedded cover,
  and that case has not been tried on the player. If you hit it, embed covers
  in your files and export again.
- wav and aiff files are exported (rbxport supports them) but have not been
  tested on a player.
- Key detection agrees with rekordbox's less often than the beat grid does
  (8 of 20 tracks in one test).

## How it fits together

| Step | Crate |
|---|---|
| Decoding | `rbl-audio` (symphonia) |
| BPM, beat grid, key, waveforms | `rbl-analysis` |
| `.DAT` / `.EXT` / `.2EX` | `rbl-anlz` |
| Tags and embedded cover | [`lofty`](https://crates.io/crates/lofty) (rbxport reads these from rekordbox's database) |
| Databases, artwork, audio copy, verification | `rbl-export`, `rbl-pdb`, `rbl-onelibrary` |

`src/main.rs` holds the CLI, the analysis and the export glue, which follows
rbxport's own (`src-tauri/src/analysis.rs` and `commands.rs` in its
repository). `src/usb.rs` implements `inspect` and `validate`; it reads the
page structure of `export.pdb` itself, because `rbl-pdb` only decodes rows.
`src/compare.rs` implements the comparisons and `src/rebuild.rs` the rebuild.

Each cover is written at 80×80 and 240×240, the sizes rekordbox uses, as
`artwork_s.jpg`/`artwork_m.jpg` next to the source image, where `rbl-export`
looks for them.

## Documentation

- [docs/format.md](docs/format.md): what a rekordbox USB export contains and
  what a CDJ-2000NXS2 needs, from a byte-level study of a rekordbox 7 stick and
  player tests.
- [docs/experiments.md](docs/experiments.md): the player experiments behind
  those notes, including the dead ends.

## Credits

- [rbxport](https://github.com/chrisle/rbxport) by Chris Le: the crates that do
  the real work.
- [crate-digger](https://github.com/Deep-Symmetry/crate-digger) and
  [beat-link](https://github.com/Deep-Symmetry/beat-link) by Deep Symmetry: the
  reference descriptions of the PDB and ANLZ formats.
- [pyrekordbox](https://github.com/dylanljones/pyrekordbox), used as an
  independent decoder while studying the format.

## License

Copyright (C) 2026 Lorenzo Bersano.

Licensed under the GNU General Public License, version 2 or (at your option)
any later version (GPL-2.0-or-later, see [LICENSE](LICENSE)), the same as
rbxport, whose crates this program links.

rbx-cli is an independent project, not affiliated with, endorsed by or
sponsored by AlphaTheta, Pioneer DJ or rbxport. rekordbox, CDJ and PRO DJ LINK
are trademarks of their respective owners and are named here only to describe
compatibility. The software is provided as is, without warranty: it writes to
removable drives, so use a spare stick and keep backups of your music.
