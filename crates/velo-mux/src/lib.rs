//! Joins a video-only and an audio-only fragmented MP4 stream into one MP4 file, in place and
//! without re-encoding.
//!
//! YouTube serves high resolutions as separate DASH streams: fragmented MP4 files, each with
//! one track, a `moov` that describes it and a run of `moof` + `mdat` fragments. [`join`] turns
//! the downloaded video stream itself into an ordinary (unfragmented) MP4 holding both tracks:
//! the video's sample data stays where it was downloaded, the sound's `mdat`s are appended after
//! it, and one new `moov` at the end gives both tracks' sample tables, built from the
//! fragments' `trun`s. The video's own `moov`, `sidx` and `moof`s become `free` boxes. So a join
//! writes the sound and a small index, however large the video, instead of copying the whole
//! video into a new file.
//!
//! The new `moov` gives each track's length from its samples, once; YouTube's streams also give
//! it in their (empty) sample tables' headers, which some players would add to the samples'.
//!
//! Inputs it can't join (not fragmented, more than one track, absolute data offsets) fail with
//! [`Error::Unsupported`] before anything is written; [`join`] then tries an `ffmpeg` found on
//! `PATH`.

mod boxes;
#[cfg(feature = "testing")]
pub mod testing;

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::boxes::{
    BoxRef, boxes_in, child, children, descendant, make_box, read_box, set_u32, set_u64, top_level,
    u32_at, u64_at,
};

/// Errors from joining streams.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file couldn't be read or written.
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
    /// An input isn't a well-formed MP4 file.
    #[error("not a valid MP4 file: {0}")]
    Malformed(String),
    /// An input is MP4, but of a kind this joiner doesn't handle.
    #[error("can't join this kind of MP4 file: {0}")]
    Unsupported(String),
    /// The fallback `ffmpeg` failed.
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Bytes read and written at a time while appending the sound to the video.
///
/// Measured on an NVMe drive with a 411 MB video: 0.53 s to copy with 8 KiB (the `BufWriter`
/// default), 0.19 s with 1 MiB.
const COPY_BUFFER: usize = 1 << 20;

/// Most samples one `trun` may hold; more means a broken file, not a real fragment.
const MAX_RUN: u32 = 10_000_000;

/// Turns the video stream `video`, whose first `video_len` bytes are the downloaded stream, into
/// an MP4 file with `audio`'s sound too; if that can't be done here and an `ffmpeg` is on `PATH`,
/// it joins them into `video` instead.
///
/// See [`join_in_place`]. Blocks while it writes; run it off an async runtime's workers.
pub fn join(video: &Path, video_len: u64, audio: &Path) -> Result<()> {
    match join_in_place(video, video_len, audio) {
        // Raised before anything was written: `video` is still the stream.
        Err(error @ (Error::Unsupported(_) | Error::Malformed(_))) => {
            let Some(ffmpeg) = find_executable("ffmpeg") else {
                return Err(error);
            };
            tracing::info!(%error, "joining the streams with ffmpeg");
            let mut out = video.as_os_str().to_owned();
            out.push(".join");
            let out = PathBuf::from(out);
            let joined = mux_ffmpeg(&ffmpeg, video, audio, &out).and_then(|()| {
                OpenOptions::new().write(true).open(&out)?.sync_all()?;
                std::fs::rename(&out, video)?;
                Ok(())
            });
            if joined.is_err() {
                let _ = std::fs::remove_file(&out);
            }
            joined
        }
        result => result,
    }
}

/// Turns the fragmented video stream `video` into an unfragmented MP4 file that also holds the
/// fragmented sound stream `audio`, writing only the sound and a new index.
///
/// `video` is first cut back to `video_len` bytes, the stream as downloaded, so a join that
/// stopped part way can run again: the boxes it turned into `free` are still recognised by what
/// they hold. Both inputs are read and checked before anything is written, so an error other
/// than [`Error::Io`] leaves `video` as the stream it was. `video` is synced to disk before this
/// returns, and the time spent reading, writing and syncing is logged.
pub fn join_in_place(video: &Path, video_len: u64, audio: &Path) -> Result<()> {
    let started = Instant::now();
    let mut file = OpenOptions::new().read(true).write(true).open(video)?;
    if file.metadata()?.len() < video_len {
        return Err(Error::Malformed(
            "the video is shorter than its download".into(),
        ));
    }
    file.set_len(video_len)?;
    let pictures = Stream::read(&mut file)?;
    let mut sound_file = File::open(audio)?;
    let sound = Stream::read(&mut sound_file)?;

    // The sound's media boxes go after the video, in order.
    let mut moved = Vec::with_capacity(sound.mdats.len());
    let mut end = video_len;
    for mdat in &sound.mdats {
        moved.push(end);
        end += mdat.len();
    }
    let picture_offsets: Vec<u64> = pictures.track.chunks.iter().map(|c| c.offset).collect();
    let sound_offsets: Vec<u64> = sound
        .track
        .chunks
        .iter()
        .map(|c| moved[c.mdat] + (c.offset - sound.mdats[c.mdat].start))
        .collect();
    let wide = picture_offsets
        .iter()
        .chain(&sound_offsets)
        .any(|&offset| offset > u64::from(u32::MAX));
    let timescale = pictures.movie_timescale;
    let (picture_trak, picture_length) =
        classic_trak(&pictures, 1, timescale, &picture_offsets, wide)?;
    let (sound_trak, sound_length) = classic_trak(&sound, 2, timescale, &sound_offsets, wide)?;
    let mut mvhd = pictures.mvhd.clone();
    let root = boxes_in(&mvhd, 0, mvhd.len())?[0];
    let at = root.payload() as usize;
    let version_1 = mvhd[at] == 1;
    set_u32(&mut mvhd, at + if version_1 { 108 } else { 96 }, 3);
    set_length(
        &mut mvhd,
        at,
        if version_1 { 24 } else { 16 },
        picture_length.max(sound_length),
    )?;
    let moov = make_box(b"moov", &[mvhd, picture_trak, sound_trak].concat());
    let read = started.elapsed();

    let writing = Instant::now();
    if let Some(ftyp) = pictures.ftyp {
        file.seek(SeekFrom::Start(ftyp.payload()))?;
        file.write_all(b"isom")?;
    }
    for hidden in &pictures.hide {
        file.seek(SeekFrom::Start(hidden.start + 4))?;
        file.write_all(b"free")?;
    }
    file.seek(SeekFrom::Start(video_len))?;
    let mut writer = BufWriter::with_capacity(COPY_BUFFER, &mut file);
    for mdat in &sound.mdats {
        sound_file.seek(SeekFrom::Start(mdat.start))?;
        let copied = std::io::copy(&mut (&mut sound_file).take(mdat.len()), &mut writer)?;
        if copied != mdat.len() {
            return Err(Error::Malformed("a media box is cut off".into()));
        }
    }
    writer.write_all(&moov)?;
    writer.flush()?;
    drop(writer);
    let write = writing.elapsed();
    let syncing = Instant::now();
    file.sync_all()?;
    let sync = syncing.elapsed();
    tracing::info!(
        appended = end - video_len + moov.len() as u64,
        ?read,
        ?write,
        ?sync,
        "mp4 joined in place"
    );
    Ok(())
}

/// One input stream: its movie box and where its samples are.
struct Stream {
    /// The `mvhd` box.
    mvhd: Vec<u8>,
    /// The only `trak` box.
    trak: Vec<u8>,
    /// Ticks per second of the movie (`mvhd`).
    movie_timescale: u32,
    /// Ticks per second of the track's media (`mdhd`); sample durations are in it.
    media_timescale: u32,
    /// The samples, from the fragments.
    track: Track,
    /// The top-level `mdat` boxes, in order.
    mdats: Vec<BoxRef>,
    /// The `ftyp` box, if any.
    ftyp: Option<BoxRef>,
    /// The top-level boxes an unfragmented file can't have (`moov`, `sidx`, `moof`…), to become
    /// `free`.
    hide: Vec<BoxRef>,
}

/// The samples of a track, in decode order.
#[derive(Default)]
struct Track {
    /// Duration of each sample, in the media timescale.
    durations: Vec<u32>,
    /// Size of each sample in bytes.
    sizes: Vec<u32>,
    /// Composition time offset of each sample.
    offsets: Vec<i32>,
    /// Whether each sample is a sync sample (a key frame).
    sync: Vec<bool>,
    /// The runs of samples stored together, one per `trun`.
    chunks: Vec<Chunk>,
}

/// A run of samples stored back to back.
struct Chunk {
    /// The file offset of the first sample.
    offset: u64,
    /// How many samples.
    samples: u32,
    /// The sample description they use (1-based).
    description: u32,
    /// The index in [`Stream::mdats`] of the box that holds them.
    mdat: usize,
}

/// A sample's flags: it isn't a sync sample.
const NON_SYNC: u32 = 0x0001_0000;

impl Stream {
    /// Reads the structure of the stream in `file`.
    fn read(file: &mut File) -> Result<Self> {
        let top = top_level(file)?;
        // What each box is; a `free` box that holds an `mvhd` or `mfhd` was a `moov` or `moof`
        // until a join that stopped part way.
        let mut kinds = Vec::with_capacity(top.len());
        for b in &top {
            let mut kind = b.kind;
            if &kind == b"free" && b.len() >= b.header + 8 {
                let mut head = [0u8; 8];
                file.seek(SeekFrom::Start(b.payload()))?;
                file.read_exact(&mut head)?;
                match &head[4..8] {
                    b"mvhd" => kind = *b"moov",
                    b"mfhd" => kind = *b"moof",
                    _ => {}
                }
            }
            kinds.push(kind);
        }
        let moov_ref = top
            .iter()
            .zip(&kinds)
            .find(|(_, kind)| *kind == b"moov")
            .map(|(b, _)| *b)
            .ok_or_else(|| missing("moov"))?;
        let moov = read_box(file, &moov_ref)?;
        let root = boxes_in(&moov, 0, moov.len())?[0];
        let parts = children(&moov, &root)?;
        let bytes = |b: &BoxRef| moov[b.start as usize..b.end as usize].to_vec();
        let mvhd = parts
            .iter()
            .find(|b| &b.kind == b"mvhd")
            .ok_or_else(|| missing("mvhd"))?;
        let traks: Vec<&BoxRef> = parts.iter().filter(|b| &b.kind == b"trak").collect();
        if traks.len() != 1 {
            return Err(Error::Unsupported(format!(
                "{} tracks in one stream",
                traks.len()
            )));
        }
        let mdhd =
            descendant(&moov, traks[0], &[b"mdia", b"mdhd"])?.ok_or_else(|| missing("mdhd"))?;
        let mvex = parts
            .iter()
            .find(|b| &b.kind == b"mvex")
            .ok_or_else(|| Error::Unsupported("the stream isn't fragmented".into()))?;
        let trex = child(&moov, mvex, b"trex")?.ok_or_else(|| missing("trex"))?;
        let at = trex.payload() as usize;
        if trex.end < trex.payload() + 24 {
            return Err(Error::Malformed("trex is cut off".into()));
        }
        let defaults = Defaults {
            description: u32_at(&moov, at + 8),
            duration: u32_at(&moov, at + 12),
            size: u32_at(&moov, at + 16),
            flags: u32_at(&moov, at + 20),
        };

        let mut mdats = Vec::new();
        let mut track = Track::default();
        let mut time = 0u64;
        for (index, (b, kind)) in top.iter().zip(&kinds).enumerate() {
            if kind == b"mdat" {
                mdats.push(*b);
            }
            if kind != b"moof" {
                continue;
            }
            let mdat = top[index + 1..]
                .iter()
                .zip(&kinds[index + 1..])
                .take_while(|(_, kind)| *kind != b"moof")
                .find(|(_, kind)| *kind == b"mdat")
                .map(|(b, _)| *b)
                .ok_or_else(|| missing("mdat"))?;
            let moof = read_box(file, b)?;
            read_fragment(
                &moof,
                b.start,
                // The next one the loop pushes.
                (mdat, mdats.len()),
                &defaults,
                &mut time,
                &mut track,
            )?;
        }
        let hide = top
            .iter()
            .zip(&kinds)
            .filter(|(b, kind)| !matches!(*kind, b"ftyp" | b"mdat") && &b.kind != b"free")
            .map(|(b, _)| *b)
            .collect();
        let ftyp = top
            .iter()
            .find(|b| &b.kind == b"ftyp" && b.len() >= b.header + 4)
            .copied();
        Ok(Self {
            mvhd: bytes(mvhd),
            trak: bytes(traks[0]),
            movie_timescale: timescale(&moov, mvhd),
            media_timescale: timescale(&moov, &mdhd),
            track,
            mdats,
            ftyp,
            hide,
        })
    }
}

/// The values a fragment's samples take when it doesn't give its own.
#[derive(Clone, Copy)]
struct Defaults {
    description: u32,
    duration: u32,
    size: u32,
    flags: u32,
}

/// Adds the samples of the fragment `moof`, which starts at `start` in its file and whose data
/// is in `mdat` (the stream's `mdat_index`th), to `track`; `time` is the decode time reached so
/// far.
///
/// A gap before the fragment's decode time (`tfdt`) lengthens the track's last sample, so the
/// samples stay on time.
fn read_fragment(
    moof: &[u8],
    start: u64,
    (mdat, mdat_index): (BoxRef, usize),
    stream: &Defaults,
    time: &mut u64,
    track: &mut Track,
) -> Result<()> {
    let root = boxes_in(moof, 0, moof.len())?[0];
    let trafs: Vec<BoxRef> = children(moof, &root)?
        .into_iter()
        .filter(|b| &b.kind == b"traf")
        .collect();
    if trafs.len() != 1 {
        return Err(Error::Unsupported(format!(
            "{} track fragments in one fragment",
            trafs.len()
        )));
    }
    let traf = trafs[0];
    let tfhd = child(moof, &traf, b"tfhd")?.ok_or_else(|| missing("tfhd"))?;
    let mut reader = Fields::new(moof, &tfhd);
    let flags = reader.u32()? & 0x00ff_ffff;
    if flags & 0x00_0001 != 0 {
        return Err(Error::Unsupported(
            "fragments with absolute data offsets".into(),
        ));
    }
    reader.u32()?; // track ID
    let mut defaults = *stream;
    if flags & 0x00_0002 != 0 {
        defaults.description = reader.u32()?;
    }
    if flags & 0x00_0008 != 0 {
        defaults.duration = reader.u32()?;
    }
    if flags & 0x00_0010 != 0 {
        defaults.size = reader.u32()?;
    }
    if flags & 0x00_0020 != 0 {
        defaults.flags = reader.u32()?;
    }
    let tfdt = child(moof, &traf, b"tfdt")?.ok_or_else(|| missing("tfdt"))?;
    let mut reader = Fields::new(moof, &tfdt);
    let decode = if reader.u32()? >> 24 == 1 {
        reader.u64()?
    } else {
        u64::from(reader.u32()?)
    };
    if decode > *time {
        if let Some(last) = track.durations.last_mut() {
            *last = last.saturating_add(u32::try_from(decode - *time).unwrap_or(u32::MAX));
        }
        // ponytail: a first fragment that starts late is shifted to 0 rather than given an
        // empty edit; YouTube's streams start at 0.
        *time = decode;
    }

    // Without its own offset, a run's data follows the previous run's, the first's the moof.
    let mut next = start;
    for trun in children(moof, &traf)?.iter().filter(|b| &b.kind == b"trun") {
        let mut reader = Fields::new(moof, trun);
        let run_flags = reader.u32()? & 0x00ff_ffff;
        let count = reader.u32()?;
        if count > MAX_RUN {
            return Err(Error::Malformed(format!("{count} samples in one run")));
        }
        let first = if run_flags & 0x00_0001 != 0 {
            let offset = i64::from(reader.u32()? as i32);
            u64::try_from(start as i64 + offset)
                .map_err(|_| Error::Malformed("a run starts before the file".into()))?
        } else {
            next
        };
        let first_flags = if run_flags & 0x00_0004 != 0 {
            Some(reader.u32()?)
        } else {
            None
        };
        let mut bytes = 0u64;
        for n in 0..count {
            let duration = if run_flags & 0x00_0100 != 0 {
                reader.u32()?
            } else {
                defaults.duration
            };
            let size = if run_flags & 0x00_0200 != 0 {
                reader.u32()?
            } else {
                defaults.size
            };
            let sample_flags = if run_flags & 0x00_0400 != 0 {
                reader.u32()?
            } else {
                first_flags.filter(|_| n == 0).unwrap_or(defaults.flags)
            };
            let offset = if run_flags & 0x00_0800 != 0 {
                // Version 0 stores it unsigned, but no real offset reaches 2^31.
                reader.u32()? as i32
            } else {
                0
            };
            track.durations.push(duration);
            track.sizes.push(size);
            track.offsets.push(offset);
            track.sync.push(sample_flags & NON_SYNC == 0);
            bytes += u64::from(size);
            *time += u64::from(duration);
        }
        if count == 0 {
            continue;
        }
        if first < mdat.payload() || first + bytes > mdat.end {
            return Err(Error::Unsupported(
                "samples outside their fragment's media box".into(),
            ));
        }
        track.chunks.push(Chunk {
            offset: first,
            samples: count,
            description: defaults.description,
            mdat: mdat_index,
        });
        next = first + bytes;
    }
    Ok(())
}

/// Reads a full box's fields in order, failing on one past its end.
struct Fields<'a> {
    buf: &'a [u8],
    at: usize,
    end: usize,
}

impl<'a> Fields<'a> {
    /// The fields of `of`, in `buf`, from its version and flags on.
    fn new(buf: &'a [u8], of: &BoxRef) -> Self {
        Self {
            buf,
            at: of.payload() as usize,
            end: of.end as usize,
        }
    }

    fn u32(&mut self) -> Result<u32> {
        if self.at + 4 > self.end {
            return Err(Error::Malformed("a box is shorter than its fields".into()));
        }
        self.at += 4;
        Ok(u32_at(self.buf, self.at - 4))
    }

    fn u64(&mut self) -> Result<u64> {
        if self.at + 8 > self.end {
            return Err(Error::Malformed("a box is shorter than its fields".into()));
        }
        self.at += 8;
        Ok(u64_at(self.buf, self.at - 8))
    }
}

/// `stream`'s `trak` as track `id` of an unfragmented file whose movie runs at
/// `movie_timescale`, with sample tables for its samples at `offsets` (one per chunk; 64-bit if
/// `wide`). Returns it and the track's length in the movie timescale.
fn classic_trak(
    stream: &Stream,
    id: u32,
    movie_timescale: u32,
    offsets: &[u64],
    wide: bool,
) -> Result<(Vec<u8>, u64)> {
    let media_length: u64 = stream.track.durations.iter().map(|&d| u64::from(d)).sum();
    let scale = |ticks: u64| {
        (u128::from(ticks) * u128::from(movie_timescale)
            / u128::from(stream.media_timescale.max(1))) as u64
    };
    let movie_length = scale(media_length);
    let mut trak = stream.trak.clone();
    let root = boxes_in(&trak, 0, trak.len())?[0];
    let tkhd = child(&trak, &root, b"tkhd")?.ok_or_else(|| missing("tkhd"))?;
    let at = tkhd.payload() as usize;
    let version_1 = trak[at] == 1;
    set_u32(&mut trak, at + if version_1 { 20 } else { 12 }, id);
    set_length(&mut trak, at, if version_1 { 28 } else { 20 }, movie_length)?;
    let mdhd = descendant(&trak, &root, &[b"mdia", b"mdhd"])?.ok_or_else(|| missing("mdhd"))?;
    let at = mdhd.payload() as usize;
    let version_1 = trak[at] == 1;
    set_length(&mut trak, at, if version_1 { 24 } else { 16 }, media_length)?;
    if let Some(elst) = descendant(&trak, &root, &[b"edts", b"elst"])? {
        let at = elst.payload() as usize;
        let version_1 = trak[at] == 1;
        // ponytail: only a single edit, YouTube's case, is fitted to the samples; longer lists
        // keep their lengths.
        if u32_at(&trak, at + 4) == 1 {
            let media_time = if version_1 {
                u64_at(&trak, at + 16) as i64
            } else {
                i64::from(u32_at(&trak, at + 12) as i32)
            };
            let shown = media_length.saturating_sub(media_time.max(0) as u64);
            set_length(&mut trak, at, 8, scale(shown))?;
        }
    }
    let tables = sample_tables(&stream.track, offsets, wide);
    let stbl = |buf: &[u8], stbl: &BoxRef| -> Result<Vec<u8>> {
        let mut payload = Vec::new();
        for part in children(buf, stbl)? {
            // The fragmented stream's empty tables, replaced below.
            if !matches!(
                &part.kind,
                b"stts" | b"ctts" | b"stss" | b"stsc" | b"stsz" | b"stz2" | b"stco" | b"co64"
            ) {
                payload.extend_from_slice(&buf[part.start as usize..part.end as usize]);
            }
        }
        payload.extend_from_slice(&tables);
        Ok(make_box(b"stbl", &payload))
    };
    let trak = rebuild(&trak, &root, &[b"mdia", b"minf", b"stbl"], &stbl)?;
    Ok((trak, movie_length))
}

/// Makes a new box from the box it is given, found in the buffer it is given.
type Replace<'a> = &'a dyn Fn(&[u8], &BoxRef) -> Result<Vec<u8>>;

/// `parent` with the box at `path` below it replaced by what `last` makes of it, and every box
/// on the way given its new size.
fn rebuild(buf: &[u8], parent: &BoxRef, path: &[&[u8; 4]], last: Replace) -> Result<Vec<u8>> {
    let Some((first, rest)) = path.split_first() else {
        return last(buf, parent);
    };
    let mut payload = Vec::new();
    let mut found = false;
    for part in children(buf, parent)? {
        if !found && &part.kind == *first {
            payload.extend(rebuild(buf, &part, rest, last)?);
            found = true;
        } else {
            payload.extend_from_slice(&buf[part.start as usize..part.end as usize]);
        }
    }
    if !found {
        return Err(missing(&boxes::name(**first)));
    }
    Ok(make_box(&parent.kind, &payload))
}

/// The sample table boxes for `track` with its chunks at `offsets`: `stts`, `ctts` (if any
/// sample has a composition offset), `stss` (if any sample isn't a sync sample), `stsc`, `stsz`
/// and `stco`, or `co64` if `wide`.
fn sample_tables(track: &Track, offsets: &[u64], wide: bool) -> Vec<u8> {
    let full = |kind: &[u8; 4], version: u8, entries: u32, body: &[u8]| {
        make_box(
            kind,
            &[&[version, 0, 0, 0][..], &entries.to_be_bytes(), body].concat(),
        )
    };
    let runs = |values: &[u32]| {
        let mut out: Vec<(u32, u32)> = Vec::new();
        for &value in values {
            match out.last_mut() {
                Some((count, last)) if *last == value => *count += 1,
                _ => out.push((1, value)),
            }
        }
        out
    };
    let pairs = |runs: &[(u32, u32)]| {
        runs.iter()
            .flat_map(|(a, b)| [a.to_be_bytes(), b.to_be_bytes()])
            .flatten()
            .collect::<Vec<u8>>()
    };

    let stts = runs(&track.durations);
    let mut tables = full(b"stts", 0, stts.len() as u32, &pairs(&stts));
    if track.offsets.iter().any(|&o| o != 0) {
        let negative = track.offsets.iter().any(|&o| o < 0);
        let as_bits: Vec<u32> = track.offsets.iter().map(|&o| o as u32).collect();
        let ctts = runs(&as_bits);
        tables.extend(full(
            b"ctts",
            u8::from(negative),
            ctts.len() as u32,
            &pairs(&ctts),
        ));
    }
    if track.sync.iter().any(|&s| !s) {
        let numbers: Vec<u8> = (1..)
            .zip(&track.sync)
            .filter(|(_, sync)| **sync)
            .flat_map(|(n, _): (u32, _)| n.to_be_bytes())
            .collect();
        let count = track.sync.iter().filter(|&&s| s).count() as u32;
        tables.extend(full(b"stss", 0, count, &numbers));
    }
    // One entry wherever the samples per chunk or their description change.
    let mut stsc: Vec<[u32; 3]> = Vec::new();
    for (number, chunk) in (1..).zip(&track.chunks) {
        if stsc
            .last()
            .is_none_or(|last| (last[1], last[2]) != (chunk.samples, chunk.description))
        {
            stsc.push([number, chunk.samples, chunk.description]);
        }
    }
    let stsc_body: Vec<u8> = stsc
        .iter()
        .flat_map(|entry| entry.map(u32::to_be_bytes))
        .flatten()
        .collect();
    tables.extend(full(b"stsc", 0, stsc.len() as u32, &stsc_body));
    let sizes: Vec<u8> = track.sizes.iter().flat_map(|s| s.to_be_bytes()).collect();
    // stsz has a default size (0: each sample has its own) before its count.
    tables.extend(make_box(
        b"stsz",
        &[
            &[0u8; 8][..],
            &(track.sizes.len() as u32).to_be_bytes(),
            &sizes,
        ]
        .concat(),
    ));
    let chunk_offsets: Vec<u8> = if wide {
        offsets.iter().flat_map(|o| o.to_be_bytes()).collect()
    } else {
        offsets
            .iter()
            .flat_map(|&o| (o as u32).to_be_bytes())
            .collect()
    };
    let kind = if wide { b"co64" } else { b"stco" };
    tables.extend(full(kind, 0, offsets.len() as u32, &chunk_offsets));
    tables
}

/// The timescale of an `mvhd` or `mdhd` box, which share the layout up to it.
fn timescale(buf: &[u8], full_box: &BoxRef) -> u32 {
    let at = full_box.payload() as usize;
    if buf[at] == 1 {
        u32_at(buf, at + 20)
    } else {
        u32_at(buf, at + 12)
    }
}

/// Sets the length field `offset` bytes into the full box whose payload starts at `at`: 64 bits
/// for version 1, else 32, which a length past `u32::MAX` doesn't fit.
fn set_length(buf: &mut [u8], at: usize, offset: usize, value: u64) -> Result<()> {
    if buf[at] == 1 {
        set_u64(buf, at + offset, value);
    } else {
        let value = u32::try_from(value)
            .map_err(|_| Error::Unsupported("a length too long for its box".into()))?;
        set_u32(buf, at + offset, value);
    }
    Ok(())
}

/// The error for a box the joiner needs but didn't find.
fn missing(kind: &str) -> Error {
    Error::Malformed(format!("no {kind} box"))
}

/// Joins the streams with `ffmpeg` into `out`, copying both without re-encoding.
fn mux_ffmpeg(ffmpeg: &Path, video: &Path, audio: &Path, out: &Path) -> Result<()> {
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(video)
        .arg("-i")
        .arg(audio)
        .args(["-map", "0:v:0", "-map", "1:a:0", "-c", "copy", "-f", "mp4"])
        .arg(out)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        tracing::warn!(%error, "ffmpeg couldn't join the streams");
        Err(Error::Ffmpeg(error))
    }
}

/// The program `name` (with the system's executable suffix) in a folder on `PATH`.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(&file))
        .find(|path| path.is_file())
}
