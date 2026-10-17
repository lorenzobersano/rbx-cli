# The rekordbox USB export, as a CDJ-2000NXS2 reads it

Notes from a byte-level study of a stick exported by **rekordbox 7.0.9** to an
HFS+ volume (1,210 tracks, 69 playlist nodes), from player tests on a
**CDJ-2000NXS2** (see [experiments.md](experiments.md)), and from a comparison
with [rbxport](https://github.com/chrisle/rbxport) 1.9.0, whose code and
`docs/reference/usb-export-db.md` explain several fields. Statements that come
only from rbxport are marked *(rbxport)*. Field names such as `unknown_20`
follow [crate-digger](https://github.com/Deep-Symmetry/crate-digger)'s Kaitai
specifications.

## Layout of the stick

- rekordbox writes the library to **`.PIONEER`** (hidden) on HFS+, and every
  path in the database starts with `/.PIONEER/...`; on FAT32 it uses
  `PIONEER`. *(rbxport)*: players accept both spellings, tested on a
  CDJ-3000; on the NXS2 only `.PIONEER` has been tested.
- Audio goes to `/Contents/<artist>/<album>/<file>`. Characters not allowed in
  file names become `_` (an artist `Antonym/Kichta` becomes `Antonym_Kichta`).
  The folder is named after the track artist, not the album artist.
  *(rbxport)*: folder names are cut to 48 characters, file names to 120 bytes,
  and missing tags become `UnknownArtist`/`UnknownAlbum`.
- `.PIONEER/rekordbox/` holds `export.pdb` (the database every player reads),
  `exportExt.pdb` (My Tag) and `exportLibrary.db` (OneLibrary, an SQLCipher
  database for newer players and rekordbox). The NXS2 needs only `export.pdb`.
- `.PIONEER/USBANLZ/` holds the analysis files, `.PIONEER/Artwork/` the covers.
- *(rbxport)*: covers are `a<id>.jpg` at 80×80 and `a<id>_m.jpg` at 240×240,
  20 images per `Artwork/000nn` folder. The rekordbox stick has the same sizes.
- The player writes `DEVSETTING.DAT`, `CDJ/`, `log/` and `MPJ/` to `.PIONEER`
  itself. `MYSETTING.DAT`, `MYSETTING2.DAT`, `DJMMYSETTING.DAT` and
  `djprofile.nxs` hold player settings; *(rbxport)* copies them from
  rekordbox's settings when the stick lacks them. None of them are needed to
  browse and play.

## export.pdb

### File and pages

- Pages of 4096 bytes, 20 tables. Page 0 is the file header and the table list
  (type, empty candidate, first page, last page).
- Table `i` has its index page at `1+2i` and its first data page at `2+2i`;
  further pages are appended at the end.
- **Index page** (flag `0x64`): header with `free = used = 0`, transaction
  `0x1fff/0x1fff`, `0x24 = 1004` (available slots), `0x26` = used slots; then
  `page_index`, first data page (or `0x03ffffff`), `0x03ffffff`, `0`, `u16 n`,
  `u16 0x1fff`, 1004 slots of `0x1ffffff8` and 20 zero bytes. The slots list
  the data pages that had rows deleted (flag `0x34`; *(rbxport)*: deleted and
  re-inserted), so in a fresh export they are empty.
- **Data page** (flag `0x24`): rows in the heap from `0x28`, in order, aligned
  to 4 bytes; the row index grows from the end of the page in groups of 16
  (`0x24` bytes each: offsets, presence flags, transaction flags).
  `free + used + 4·groups + 2·rows = 4096 − 40`.
- The three bytes at `0x18` pack the row-index length (low 13 bits) and the
  number of present rows (top 11 bits).
- **Transaction fields.** For a page written in one go with up to 255 rows,
  the transaction is `(row count, 0)` and every flag is set. Pages with more
  rows differ: the rekordbox stick's 284-row `playlist_entries` pages have
  `(1, index)` (283 twice, 138 once); *(rbxport)* sets only that row's bit in
  the second mask, and the `(n, n)` form crashed rekordbox 6.8.6.
- The last page of each table points to an empty (all-zero) page, which is
  also the table's `empty_candidate` in the header. *(rbxport)* drops trailing
  zero pages from the file, so a candidate can lie past its end; the NXS2
  accepts this.
- Sequence numbers: the header's must exceed every page's *(rbxport)*: pages
  stamped with the header's number get rolled back when a player opens the
  file to write its history.
- Row `index_shift` = position of the row in its page × `0x20`. *(rbxport)*:
  NXS-era players read it.

### Strings

- Short ASCII up to 126 characters, **long ASCII (`0x40`)** beyond, UTF-16LE
  (`0x90`) for anything non-ASCII, without a terminating NUL. All 28,315
  strings of the rekordbox stick re-encode byte for byte.
- Long strings start 4-byte aligned from the start of the row. rekordbox
  leaves stale bytes in the padding; zeros work.
- Artist and album rows switch to a 16-bit name offset (`subtype | 0x04`) for
  long names: up to 228 bytes the offset is 8-bit, from 236 bytes 16-bit.
  *(rbxport always writes the 8-bit form.)*

### Track rows

crate-digger names unknown fields after their decimal offset.

| Offset | Name (crate-digger) | Meaning |
|---|---|---|
| `0x00` | `subtype` | `0x24`; *(rbxport)* required by players |
| `0x04` | `bitmask` | *(rbxport)* contentLink. Two values on the stick, `0xC0700` and `0xD0700` |
| `0x14` | `unknown_20` | *(rbxport)* masterContentId: the track's id in the source rekordbox library. Different for every track |
| `0x18` | `unknown_24` + `unknown_26` | *(rbxport)* masterDbId, read as one u32: the id of the source library |
| `0x56` | `unknown_86` | 41 (`0x29`) on every track; *(rbxport)* analysedBits |
| `0x5a` | `unknown_90` | file type: 1 mp3, 4 m4a, 5 flac, *(rbxport)* 11 wav, 12 aiff |
| `0x5c` | `unknown_92` | 3 on every track |

On the rekordbox stick the masterDbId has two values: `0xD0868292` on 1,173
tracks (1,167 m4a, 4 flac, 1 mp3) and `0x38FEED1E` on 37 mp3s. They are two
source libraries, not two file formats, although at first sight they line up
with the format. rbxport writes 0 to all three ids and the NXS2 does not mind.

Of the string slots, only slot 7 matters for playback ("ON": load hot cues
automatically). rekordbox also fills `unknown_string_2/3`, `analyze_date` and
`kuvo_public`; rbxport leaves them empty and the NXS2 does not mind.

### Other tables

- `columns` (16): the entries of the browse menu. `unknown_17`/`unknown_18`
  (17, 18): *(rbxport)* browse category and sort settings. All three, the
  colours and the My Tag defaults in `exportExt.pdb` are fixed tables that can
  be copied verbatim from a rekordbox export.
- Type 19 (`history` in crate-digger): *(rbxport)* the export's **property**
  row (device name, track count, date), not the play history. rekordbox adds a
  row for each change; the last one is live.
- Key names are shown by the player exactly as stored in `keys`. A rekordbox
  library keeps the notation in use when each key was first written, so one
  stick can mix Camelot (`8A`) and classic (`Am`) names.

## ANLZ analysis files

### ANLZ folder

The analysis of a track lives in
`USBANLZ/P<3 hex>/<8 hex>/ANLZ0000.DAT` (with `.EXT` and `.2EX` beside it).
**The player derives the folder from the track's audio path and ignores
`analyze_path`** (see the path experiment in [experiments.md](experiments.md)).
The folder is a hash of the path's UTF-16 code units:

```
h = 0
for u in utf16(file_path):          # e.g. "/Contents/Artist/Album/track.m4a"
    h = (h * 23497 + u) mod 2^32
    h = (h * 37813 + u) mod 2^32
h = h mod 200003
P = bits 0, 2, 6, 7, 9, 13, 16 of h, packed into bits 0..6
folder = "P%03X/%08X" % (P, h)
```

It matches all 1,210 tracks of the stick. rbxport uses the same function, in
one step: `h = h·0x34F5501D + u·0x93B6`, where `0x34F5501D` = 23497·37813 and
`0x93B6` = 37814. If two tracks hash to the same folder, rekordbox names the
second analysis `ANLZ0001`; rbxport renames the audio file instead.

### Containers

- Big-endian. A 28-byte `PMAI` header: after `len_file` come
  `00000001 00010000 00010000 00000000` (the first u4 is 0 in 51 files).
- `PPTH` is UTF-16BE with a terminating NUL, and always equals the track's
  `file_path` in the database.
- Sections in rekordbox's order: `.DAT` = PPTH PVBR PQTZ PWAV PWV2 PCOB PCOB;
  `.EXT` = PPTH PWV3 PCOB PCOB PCO2 PCO2 PQT2 PWV5 PWV4 (PVB2) PSSI; `.2EX`
  (CDJ-3000) = PPTH PWV7 PWV6 PWVC. 5 of 1,210 tracks have no `.EXT`, so it is
  optional.

### What the CDJ-2000NXS2 needs

- **`.DAT`**: PPTH, PVBR (all zero is enough for m4a and flac), PQTZ, PCOB with
  hot cues A–C, PCOB with memory cues. The player **ignores a `.DAT` that has
  neither PVBR nor PWAV/PWV2**. PWAV/PWV2 only draw the blue overview when the
  `.EXT` is missing.
- **`.EXT`**: PPTH, PWV5 (scrolling colour waveform), PWV4 (colour overview),
  PCOB with hot cues D–H, PCO2 with every cue (colours, comments). PWV3
  (monochrome) may matter with the blue waveform setting: untested.
- **`.2EX`**, PQT2 and PSSI: not needed. The beat grid comes from the `.DAT`'s
  PQTZ; the player does not read PQT2.
- Without an `.EXT` no beat-grid lines are drawn (there is no scrolling
  waveform to draw them on), but beat jump still works.

### Beat grid (PQTZ)

`u4 0`, `u4 0x80000`, `u4 n`, then `n × (u2 beat 1–4, u2 BPM×100, u4 ms)`.
*(rbxport)*: the low 16 bits of the second u4 are a signed grid offset in ms;
the one file with `0x8fcae` is shifted by −850 ms.

On the stick: BPM between 81 and 178 (default range 70–180), 93% integers;
99% of grids have one constant tempo; the grid starts at the first beat in
`[0, period)`; **bars are counted from that first beat**, which is beat 1 in
85% of tracks. Beat positions sit on the kick's attack (the energy above
200 Hz jumps exactly on the beat; 20–90 Hz is at its minimum there and peaks
~100 ms later, between kicks). *(rbxport)* places downbeats from the track's
structure instead; on the NXS2 both work.

### Cues (PCOB, PCPT, PCO2, PCP2)

- PCOB: always two sections, hot cues (type 1) then memory cues (type 0), even
  when empty. `memory_count` is `0xffffffff` but once 0; *(rbxport)*: it is the
  number of memory cues − 1.
- **The two PCOB pairs split the cues**: the `.DAT`'s holds hot cues A–C and
  the memory cues, the `.EXT`'s holds hot cues D–H (empty when there are none)
  and no memory cues. The NXS2 takes A–C and D–H from these lists and only the
  colours and comments from PCO2. *(rbxport)* writes D–P the same way.
- PCPT (56 bytes): the usual form has `status 0`, `u4 0x10000` at `0x14`,
  `order_first/last 0xffff`, `0x0003e8` after the type, a 16-byte zero tail and
  `loop_time 0xffffffff` for plain cues. About a quarter of entries have
  `status 1`, `0x14 = 0` and an `ffffffff…` tail instead, also for type-1 cues
  with a `loop_time`: meaning unknown. *(rbxport)*: `0x10 = 4` marks an active
  loop, and memory cues link to their neighbours' indices.
- PCO2 (`.EXT` only; two sections, hot then memory): PCP2 entries of 88 bytes
  (colour + 40 zero bytes), 48 (colour only) or 44 (no colour). The 44/48-byte
  forms go with `status 1` PCPT entries. Colour code → fixed RGB (21 green
  `00ff00`, 36 orange `ff8c00`, 43 red `ff0017`, 8 light blue `00c4ff`, 23
  `33ff00`, 60 `4d00ff`); *(rbxport)* code 0 takes its RGB from the slot, which
  is why it appears with different colours. Every loop is orange.
- Hot cue order in rekordbox's lists: descending slot in 64 lists, ascending
  in 18, the same in PCOB and PCO2 (probably creation order).

### Waveforms

| Section | Columns | One column | Last header u4 |
|---|---|---|---|
| PWAV (`.DAT`) | 400 | 1 byte: whiteness (3 bits) + height (5) | `0x10000` |
| PWV2 (`.DAT`) | 100 | height (4 bits, the rest zero) | `0x10000` |
| PWV3 (`.EXT`) | 150/s | 1 byte: whiteness (3) + height (5) | `0x00960000` |
| PWV4 (`.EXT`) | 1,200 | 6 bytes d0–d5 | `0` |
| PWV5 (`.EXT`) | 150/s | 2 bytes: R, G, B (3 bits each) + height (5) + 2 zero bits | `0x00960305` |
| PWV6/PWV7 (`.2EX`) | | 3 bytes | |

- Headers are constant; `0x96` = 150 columns per second. Every unused bit is
  zero, and all 6,035 sections of the stick round-trip byte for byte.
- A column is 294 samples (1/150 s at 44.1 kHz). PWV3 and PWV5 have the same
  column count (±1 on 3 tracks), about `duration × 150` + 0–8: rekordbox keeps
  the encoder delay (AAC priming of 2,048 samples or 0; ~1,103 for LAME mp3s),
  and so do the beat grid and the cues. *(rbxport)* decodes without gapless
  trimming, which matches.
- Heights over 1,203 `.DAT` files (5 are all zero): PWAV peaks at 25 in 882
  files (24 in 238, 23 in 58, lower in 20) and never goes below 2 in 1,142;
  PWV2 peaks at 15 in 1,111 and never goes below 1. PWAV whiteness ≤ 5, PWV3
  up to 31/7. *(rbxport)* uses the same bounds, because Nexus firmware rejects
  the whole preview if any PWV2 byte exceeds 15 and treats 0 as unfinished.
- PWV3/PWV5 height ≈ `floor(31.5 · peak²)` with peak = max |(L+R)/2| in the
  column (exact in 97% of PWV3 columns, 76% of PWV5's). On silence rekordbox
  writes colour (7, 7, 7).
- PWV5 colours: RMS of three bands (< 173 Hz, 215–1,861 Hz, > 1,861 Hz),
  scaled so that the strongest is 7.
- PWV4, per column: d0 ∝ peak, d1 ≈ 212 − 2.7 × height (inverse of the
  height), d2/d3 ∝ √RMS of 20–411 / 20–173 Hz, d4 ∝ RMS of 173–974 Hz,
  d5 ∝ RMS(3.5–16 kHz)^1.5. beat-link draws d3–d5 as RGB. *(rbxport writes d1
  and d2 as 0; the NXS2 does not seem to use them.)*
- pyrekordbox reads PWV5's red with the wrong shift (12 instead of 13) and
  cannot read `.EXT` files with 44-byte PCP2 entries.

## Differences between rbxport and rekordbox that do not matter on the NXS2

- 126-character ASCII strings are written long (`len < 0x7e`); rekordbox keeps
  them short.
- Artist and album names always use the 8-bit offset.
- PWV4 d1/d2 are 0; PWV3 height uses `round(31·p²)` and silence is black.
- The three library ids and the unused string slots are empty.
