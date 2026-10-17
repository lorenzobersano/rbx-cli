# Player experiments

How the notes in [format.md](format.md) were established. Each experiment was
run on a **CDJ-2000NXS2** with a scratch stick formatted like the rekordbox one
(MBR, journaled HFS+, library in `.PIONEER`), never on the original stick. Most
were done with a Python prototype that read and wrote the PDB and ANLZ formats
directly. It is not published, because `rbx-cli` replaced it, but the results
apply to any writer. A typical test copies part of a rekordbox export, changes
one thing per group of tracks, puts each group in its own playlist, and
compares what the player shows.

## 1. Our PDB writer with rekordbox's data (2026-10-16)

A full copy of the rekordbox stick with `export.pdb` re-encoded by our writer:
compact pagination (215 pages instead of 350), sequences from 1, transaction
`(n, 0)`, empty index pages.

**Result:** the NXS2 reads the stick, browses every category (playlist,
artist, album, genre, key) and loads tracks with waveforms and beat grid. A
freshly paginated file is accepted as is.

## 2. Which ANLZ files and sections matter

| Group | `.DAT` | `.EXT` / `.2EX` | Result |
|---|---|---|---|
| A | only PPTH + PQTZ + PCOB | original | as with rekordbox; beat jump works, also on mp3 |
| B | only PPTH + PQTZ + PCOB | removed | the track loads, but **no waveform, grid or hot cues** |
| C | original | removed | only the blue overview; no scrolling waveform |

Then, without any `.EXT`:

| Group | `.DAT` | Grid lines | Hot cues | Blue overview | Beat jump |
|---|---|---|---|---|---|
| C | original | no | yes | yes | yes |
| D | without PWAV/PWV2 | no | yes | no | yes |
| E | without PVBR | no | yes | yes | yes |

- B and D hold the same tracks and differ **only** in PVBR (checked byte for
  byte): the NXS2 **ignores a `.DAT` with neither PVBR nor PWAV/PWV2**, and
  reads it with either.
- PVBR is all zero in every m4a and flac (1,172 files) and has data only in
  mp3s (38), so an m4a needs just PPTH + zero PVBR + PQTZ + PCOB.
- Without an `.EXT` grid lines never show (there is no scrolling waveform to
  draw them on); beat jump still works.

| Group | Change | Result |
|---|---|---|
| F | PQTZ shifted by half a beat, original `.EXT` | grid lines shifted by half a beat |
| G | `.EXT` with only PPTH PWV3 PWV5 PWV4, no `.2EX` | colour waveform, grid, hot cues in the right colour |
| H | `.EXT` with only PPTH PWV5 PWV4, no `.2EX` | same as G |

So the grid comes from the `.DAT`'s PQTZ (PQT2 is not used), and the `.EXT`
needs only PWV5 (scrolling colour waveform) and PWV4 (colour overview).

## 3. Cues

| Group | Change | Result |
|---|---|---|
| I | hot cues A–D in non-default colours with a comment, plus E only in PCO2 | **right colours**, but **no E** |
| J | hot cues A–D moved in PCO2 only (PCOB keeps the real positions) | cues at the **real positions**; one track lost D |
| K | tracks with a hot cue D, original files | D shows |
| L | as I, with E also in the `.DAT`'s PCOB | E does not show |
| M | `.DAT` PCOB without hot cues, original `.EXT` | no hot cues |

Re-reading the data showed that the `.EXT`'s PCOB **always holds exactly hot
cues D–H**. That explains every group: A–C come from the `.DAT`'s PCOB (M, J),
D–H from the `.EXT`'s PCOB (K; in I, J and L our writer had put A–C there),
the colours from PCO2 (I). With that fixed, 1,204 of the stick's 1,205 `.EXT`
files rebuild byte for byte from their cue lists (1,057 before), and a last
test confirmed it on the player:

| Group | Change | Result |
|---|---|---|
| N | as I | right colours, **E shows**, the original D stays |
| O | hot cues A–D moved to bars 1–4, written consistently in both PCOBs and PCO2 | A–D at the new positions |

Comments on the display were not checked.

## 4. Generated waveforms

Six tracks with every waveform (PWAV, PWV2, PWV3, PWV4, PWV5) generated from
the audio and everything else from rekordbox, compared on the NXS2 with the
same tracks from the original stick: **they look and read like rekordbox's,
with plausible colours, and the blue waveform appears too.** The generator's
formulas are in [format.md](format.md#waveforms); they were fitted on 8 tracks
and gave the same errors on 24 others (mean error 0.6 / 1.1 / 0.7 on PWV5's
R/G/B, 3–11 out of 127 on PWV4). Our green was systematically higher in
mid-heavy passages.

## 5. The ANLZ folder (2026-10-16)

The first full build from audio (20 tracks) put the analysis files in
sequentially numbered folders, with `analyze_path` pointing there. The player
read the stick and played the tracks, but showed **no waveforms**. Three
playlists were added to the same build:

| Group | Analysis files | Result |
|---|---|---|
| X | ours, in the folder rekordbox uses for that audio file | **waveforms and grid** |
| Y | ours, in small-number folders, `analyze_path` updated | nothing |
| Z | rekordbox's own, in our folder | nothing |

The player **ignores `analyze_path` and derives the folder from the audio
path**. With the folders computed from that hash (see
[format.md](format.md#anlz-folder)), the rebuilt stick showed waveforms and
grid on every track.

## 6. Missing title and info (2026-10-17)

Our own build browsed correctly, but a loaded track showed neither its title
in the top bar nor any metadata in info mode. That build had empty `keys` and
`artwork` tables and `key_id`/`artwork_id` 0 on every track. One experiment
copied rekordbox's whole `keys` table (38 rows) plus the `artwork` rows and
images of groups 3 and 5, then gave each group one field from rekordbox's row
for the same file:

| Group | Field from rekordbox | Title and info | Key | Cover |
|---|---|---|---|---|
| 0 (control) | none | yes | no | no |
| 1 | `unknown_20` (masterContentId) | yes | no | no |
| 2 | `key_id` | yes | yes | no |
| 3 | `artwork_id` + rows and images | yes | no | yes |
| 4 | `label_id` = 0 | yes | no | no |
| 5 | all of the above | yes | yes | yes |

- Title and info came back **even in the control group**, so the row fields
  were not the cause; the only global changes were the two tables.
  `unknown_20` and `label_id` do not matter. An empty `labels` table is fine
  (rekordbox always leaves it empty).
- Counter-tests on the original build: `keys` filled (24 rows, no track using
  them) with `artwork` empty, and `artwork` filled (20 rows with images, no
  track using them) with `keys` empty: title and info were **still missing**
  in both. The files the player writes to `.PIONEER` (`DEVSETTING.DAT`, `CDJ/`,
  `log/`, `MPJ/`) were present in every test, so they are not the cause either.
- Not tested: both tables filled with no track using them. rbxport writes
  empty tables exactly as rekordbox and our writer did, so **`rbx-cli` can hit
  the same problem**: its exports fill `keys` and `artwork` only when tracks
  have a detected key and an embedded cover. A source folder without any
  cover art gives an empty `artwork` table; this has not been tried on the
  player.
- Keys are shown exactly as named in the table, so group 5 showed a mix of
  Camelot and classic names, copied from a rekordbox library whose notation
  had changed over time.

## 7. rbx-cli (2026-10-17)

`rbx-cli export` of the same 20 tracks (`.PIONEER`, 3 playlists, keys, covers,
`exportLibrary.db`): browsing, title and info, key, cover, waveforms, grid,
sync, beat jump and loops all work. Every source track had an embedded cover.
A drum and bass track came out at 174 BPM where rekordbox says 87; for mixing
drum and bass the double tempo is arguably the right one.

## 8. A whole library with rbx-cli rebuild (2026-10-17)

`rbx-cli rebuild` of the whole rekordbox stick: 1,210 tracks, 69 folders and
playlists (the largest has 206 tracks), 1,209 covers and 240 hot and memory
cues, written in 26 minutes (about 0.9 s of analysis per track, then 6.5 GB of
audio copied over USB 3). Compared with the source stick:

- the playlist tree, the order of folders and playlists, and every playlist's
  tracks and their order are identical;
- every title, artist, album, genre, label, comment, rating, colour, year,
  date added and track number is identical, and so are the covers, byte for
  byte;
- every cue has the same slot, type, position, loop end and comment. Hot cues
  that have no colour in rekordbox get rbxport's default colour for their
  slot;
- BPM: the same as rekordbox on 1,136 tracks, double on 36 (drum and bass and
  jungle), half on 3, different on 35 (8 of which rekordbox had not
  analysed);
- rbxport renamed 33 audio files (to avoid two tracks sharing an ANLZ folder)
  and dropped trailing dots from folder names, for FAT compatibility. Players
  find them through the database, so this is invisible.

On the NXS2 everything checked behaves as on the original stick: the tree,
the order, title and info, covers, keys, waveforms, cues, and browsing by
artist, album, genre and key.

## Beat grid estimation

Before switching to rbxport's analysis, the prototype had its own constant-tempo
estimator. What was learnt about rekordbox's grids may help others.

- Averaging band energy around rekordbox's beats over 30 tracks: above 200 Hz
  energy jumps exactly on the beat (+1 ms), the **kick's attack**; 90–200 Hz
  peaks at +35 ms (the kick's body); **20–90 Hz is at its minimum on the beat
  and peaks at +100 ms**, as the sub-bass plays between kicks. One band below
  200 Hz mixes the two and lands grids on the off-beat.
- Around rekordbox's downbeats, claps and snares (800–8,000 Hz attack) favour
  beats 2 and 4, and sub-bass re-entries, crashes and hi-hats favour beat 1.
  Yet a score built on these cues matched rekordbox's bar only 71–84% of the
  time (fills and risers make beat 4 look like a start), while **"the first beat
  is beat 1" matched 72 of 79 tracks**: rekordbox counts bars from the track's
  first beat.
- Best result of the prototype on 40 tracks: exact BPM 36/40 (39/40 on another
  40), phase within 25 ms 89% (92%), median phase error 2.0 ms, right bar 29/36.
  rbxport's analysis gave a similar grid accuracy (16/20 identical to rekordbox
  on the player-test set) at about 1.5 s per track instead of 20.
