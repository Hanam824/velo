//! Joins a video-only and an audio-only MP4 stream into one MP4 file, without re-encoding.
//!
//! YouTube serves high resolutions as separate DASH streams: fragmented MP4 files, each with
//! one track, a `moov` that describes it and a run of `moof` + `mdat` fragments. [`mux`] writes
//! one fragmented MP4 holding both tracks: the two `trak`s in one `moov` with track IDs 1 and
//! 2, then the fragments of both interleaved by decode time. Sample data is copied as it is,
//! and every box keeps its size, so the offsets inside each fragment stay valid.
//!
//! YouTube's streams also give the whole duration in their (empty) sample tables' headers,
//! which some players add to the fragments' and so show twice the length. The joined file
//! gives it once, in `mehd`, as fragmented MP4 files should.
//!
//! Inputs it can't join (not fragmented, more than one track, absolute data offsets) fail with
//! [`Error::Unsupported`]; [`mux`] then tries an `ffmpeg` found on `PATH`.

mod boxes;
#[cfg(feature = "testing")]
pub mod testing;

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Joins `video` and `audio` into `out`; if they can't be joined here and an `ffmpeg` is on
/// `PATH`, it joins them instead.
///
/// `out` is overwritten. Blocks while it copies; run it off an async runtime's workers.
pub fn mux(video: &Path, audio: &Path, out: &Path) -> Result<()> {
    match mux_mp4(video, audio, out) {
        Ok(()) => Ok(()),
        Err(error) => match find_executable("ffmpeg") {
            Some(ffmpeg) => mux_ffmpeg(&ffmpeg, video, audio, out),
            None => Err(error),
        },
    }
}

/// Joins fragmented MP4 `video` and `audio` into fragmented MP4 `out`.
pub fn mux_mp4(video: &Path, audio: &Path, out: &Path) -> Result<()> {
    let mut video = Input::read(video)?;
    let mut audio = Input::read(audio)?;
    let movie_timescale = video.movie_timescale;

    let mut mvhd = video.mvhd.clone();
    let wide = mvhd[8] == 1;
    set_u32(&mut mvhd, 8 + if wide { 108 } else { 96 }, 3);
    // The longer of the two, in the video's movie timescale.
    let audio_duration = u128::from(audio.duration) * u128::from(movie_timescale)
        / u128::from(audio.movie_timescale.max(1));
    let duration = video.duration.max(audio_duration as u64);
    set_duration(&mut mvhd, 8, if wide { 24 } else { 16 }, wide, 0);
    let video_trak = track_with_id(&video.trak, 1)?;
    let audio_trak = track_with_id(&audio.trak, 2)?;
    let mut mvex_payload = make_box(
        b"mehd",
        &[&[1, 0, 0, 0], &duration.to_be_bytes()[..]].concat(),
    );
    for (trex, id) in [(&video.trex, 1), (&audio.trex, 2)] {
        let mut trex = trex.clone();
        set_u32(&mut trex, 12, id);
        mvex_payload.extend_from_slice(&trex);
    }
    let moov = make_box(
        b"moov",
        &[
            mvhd,
            video_trak,
            audio_trak,
            make_box(b"mvex", &mvex_payload),
        ]
        .concat(),
    );
    let ftyp = make_box(
        b"ftyp",
        &[
            b"isom".as_slice(),
            &0x200u32.to_be_bytes(),
            b"isom",
            b"iso2",
            b"iso6",
            b"mp41",
        ]
        .concat(),
    );

    // Every fragment, as (input, index), in decode-time order; video first on a tie.
    let mut order: Vec<(usize, usize)> = (0..video.fragments.len())
        .map(|i| (0, i))
        .chain((0..audio.fragments.len()).map(|i| (1, i)))
        .collect();
    let time = |input: &Input, i: usize| {
        (
            u128::from(input.fragments[i].time),
            u128::from(input.media_timescale),
        )
    };
    order.sort_by(|&(a_input, a), &(b_input, b)| {
        let (a_time, a_scale) = time(if a_input == 0 { &video } else { &audio }, a);
        let (b_time, b_scale) = time(if b_input == 0 { &video } else { &audio }, b);
        (a_time * b_scale).cmp(&(b_time * a_scale))
    });

    let mut writer = BufWriter::new(File::create(out)?);
    writer.write_all(&ftyp)?;
    writer.write_all(&moov)?;
    for (sequence, (input, index)) in order.into_iter().enumerate() {
        let (source, track) = if input == 0 {
            (&mut video, 1)
        } else {
            (&mut audio, 2)
        };
        let fragment = &source.fragments[index];
        let mut moof = fragment.moof.clone();
        let root = boxes_in(&moof, 0, moof.len())?[0];
        let mfhd = child(&moof, &root, b"mfhd")?.ok_or_else(|| missing("mfhd"))?;
        set_u32(&mut moof, mfhd.payload() as usize + 4, sequence as u32 + 1);
        let tfhd = descendant(&moof, &root, &[b"traf", b"tfhd"])?.ok_or_else(|| missing("tfhd"))?;
        set_u32(&mut moof, tfhd.payload() as usize + 4, track);
        writer.write_all(&moof)?;
        let mdat = fragment.mdat;
        source.file.seek(SeekFrom::Start(mdat.start))?;
        let copied = std::io::copy(&mut (&mut source.file).take(mdat.len()), &mut writer)?;
        if copied != mdat.len() {
            return Err(Error::Malformed("a media box is cut off".into()));
        }
    }
    writer.flush()?;
    writer
        .into_inner()
        .map_err(|e| e.into_error())?
        .sync_all()?;
    Ok(())
}

/// One input stream.
struct Input {
    file: File,
    /// The `mvhd` box.
    mvhd: Vec<u8>,
    /// The only `trak` box.
    trak: Vec<u8>,
    /// The `trex` box of the track.
    trex: Vec<u8>,
    /// Ticks per second of the movie (`mvhd`).
    movie_timescale: u32,
    /// The length in the movie timescale, from `mehd` or else `mvhd`.
    duration: u64,
    /// Ticks per second of the track's media (`mdhd`); fragment times are in it.
    media_timescale: u32,
    fragments: Vec<Fragment>,
}

/// A `moof` and the `mdat` after it.
struct Fragment {
    moof: Vec<u8>,
    mdat: BoxRef,
    /// The decode time of the fragment's first sample, from `tfdt`.
    time: u64,
}

impl Input {
    /// Reads the structure of the stream at `path`: its movie box and where its fragments are.
    fn read(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let top = top_level(&mut file)?;
        let moov_ref = top
            .iter()
            .find(|b| &b.kind == b"moov")
            .ok_or_else(|| missing("moov"))?;
        let moov = read_box(&mut file, moov_ref)?;
        let root = boxes_in(&moov, 0, moov.len())?[0];
        let parts = children(&moov, &root)?;
        let bytes = |b: &BoxRef| moov[b.start as usize..b.end as usize].to_vec();

        let mvhd = parts
            .iter()
            .find(|b| &b.kind == b"mvhd")
            .ok_or_else(|| missing("mvhd"))?;
        let movie_timescale = timescale(&moov, mvhd);
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
        let duration = match child(&moov, mvex, b"mehd")? {
            Some(mehd) => read_duration(&moov, mehd.payload() as usize, 4, 4),
            None => read_duration(&moov, mvhd.payload() as usize, 16, 24),
        };

        let mut fragments = Vec::new();
        for (index, moof_ref) in top.iter().enumerate().filter(|(_, b)| &b.kind == b"moof") {
            let mdat = top[index + 1..]
                .iter()
                .take_while(|b| &b.kind != b"moof")
                .find(|b| &b.kind == b"mdat")
                .copied()
                .ok_or_else(|| missing("mdat"))?;
            let moof = read_box(&mut file, moof_ref)?;
            let root = boxes_in(&moof, 0, moof.len())?[0];
            let trafs: Vec<BoxRef> = children(&moof, &root)?
                .into_iter()
                .filter(|b| &b.kind == b"traf")
                .collect();
            if trafs.len() != 1 {
                return Err(Error::Unsupported(format!(
                    "{} track fragments in one fragment",
                    trafs.len()
                )));
            }
            let tfhd = child(&moof, &trafs[0], b"tfhd")?.ok_or_else(|| missing("tfhd"))?;
            if u32_at(&moof, tfhd.payload() as usize) & 0x00_0001 != 0 {
                return Err(Error::Unsupported(
                    "fragments with absolute data offsets".into(),
                ));
            }
            let tfdt = child(&moof, &trafs[0], b"tfdt")?.ok_or_else(|| missing("tfdt"))?;
            let at = tfdt.payload() as usize;
            let time = if moof[at] == 1 {
                u64_at(&moof, at + 4)
            } else {
                u64::from(u32_at(&moof, at + 4))
            };
            fragments.push(Fragment { moof, mdat, time });
        }
        Ok(Self {
            mvhd: bytes(mvhd),
            trak: bytes(traks[0]),
            trex: bytes(&trex),
            movie_timescale,
            duration,
            media_timescale: timescale(&moov, &mdhd),
            fragments,
            file,
        })
    }
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

/// `trak` with its track ID set to `id` and its durations set to 0: its samples are all in
/// fragments, which give their own timing.
fn track_with_id(trak: &[u8], id: u32) -> Result<Vec<u8>> {
    let mut trak = trak.to_vec();
    let root = boxes_in(&trak, 0, trak.len())?[0];
    let tkhd = child(&trak, &root, b"tkhd")?.ok_or_else(|| missing("tkhd"))?;
    let at = tkhd.payload() as usize;
    let wide = trak[at] == 1;
    set_u32(&mut trak, at + if wide { 20 } else { 12 }, id);
    set_duration(&mut trak, at, if wide { 28 } else { 20 }, wide, 0);
    if let Some(mdhd) = descendant(&trak, &root, &[b"mdia", b"mdhd"])? {
        let at = mdhd.payload() as usize;
        let wide = trak[at] == 1;
        set_duration(&mut trak, at, if wide { 24 } else { 16 }, wide, 0);
    }
    if let Some(elst) = descendant(&trak, &root, &[b"edts", b"elst"])? {
        let at = elst.payload() as usize;
        let wide = trak[at] == 1;
        let count = u32_at(&trak, at + 4) as usize;
        for entry in 0..count {
            let offset = 8 + entry * if wide { 20 } else { 12 };
            set_duration(&mut trak, at, offset, wide, 0);
        }
    }
    Ok(trak)
}

/// Sets the duration field `offset` bytes into the full box whose payload starts at `at`: 64
/// bits if `wide`, else 32.
fn set_duration(buf: &mut [u8], at: usize, offset: usize, wide: bool, value: u64) {
    if wide {
        set_u64(buf, at + offset, value);
    } else {
        set_u32(buf, at + offset, value as u32);
    }
}

/// The duration field of the full box whose payload starts at `at`: at `narrow` bytes in
/// (32 bits) for version 0, at `wide` bytes in (64 bits) for version 1.
fn read_duration(buf: &[u8], at: usize, narrow: usize, wide: usize) -> u64 {
    if buf[at] == 1 {
        u64_at(buf, at + wide)
    } else {
        u64::from(u32_at(buf, at + narrow))
    }
}

/// The error for a box the joiner needs but didn't find.
fn missing(kind: &str) -> Error {
    Error::Malformed(format!("no {kind} box"))
}

/// Joins the streams with `ffmpeg`, copying both without re-encoding.
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
        Err(Error::Ffmpeg(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

/// The program `name` (with the system's executable suffix) in a folder on `PATH`.
pub fn find_executable(name: &str) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(&file))
        .find(|path| path.is_file())
}
