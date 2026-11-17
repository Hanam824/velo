//! Joining synthetic fragmented MP4 streams built here, box by box.

use std::path::{Path, PathBuf};

use velo_mux::testing::{mp4_box, mvhd, stream, trak};
use velo_mux::{Error, join_in_place};

fn be32(buf: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(buf[at..at + 4].try_into().unwrap())
}

/// The boxes in `buf`, as (type, start, end).
fn walk(buf: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < buf.len() {
        let size = be32(buf, at) as usize;
        out.push((buf[at + 4..at + 8].try_into().unwrap(), at, at + size));
        at += size;
    }
    out
}

/// The payload of the first `kind` box in `buf`.
fn find<'a>(buf: &'a [u8], kind: &[u8; 4]) -> &'a [u8] {
    let (_, start, end) = walk(buf).into_iter().find(|(k, ..)| k == kind).unwrap();
    &buf[start + 8..end]
}

/// A track of the joined file, read back from its sample tables.
struct Track {
    id: u32,
    /// `tkhd`'s and the single edit's length, in the movie timescale.
    lengths: (u32, u32),
    /// The sum of the sample durations, from `stts`.
    media_length: u32,
    /// Each sample's bytes, found through `stsc`, `stco` and `stsz`.
    samples: Vec<Vec<u8>>,
}

fn track(file: &[u8], trak: &[u8]) -> Track {
    let tkhd = find(trak, b"tkhd");
    let elst = find(find(trak, b"edts"), b"elst");
    let stbl = find(find(find(trak, b"mdia"), b"minf"), b"stbl");
    let stts = find(stbl, b"stts");
    let media_length = (0..be32(stts, 4) as usize)
        .map(|n| be32(stts, 8 + n * 8) * be32(stts, 12 + n * 8))
        .sum();
    let stsz = find(stbl, b"stsz");
    let sizes: Vec<usize> = (0..be32(stsz, 8) as usize)
        .map(|n| be32(stsz, 12 + n * 4) as usize)
        .collect();
    let stco = find(stbl, b"stco");
    let chunks: Vec<usize> = (0..be32(stco, 4) as usize)
        .map(|n| be32(stco, 8 + n * 4) as usize)
        .collect();
    let stsc = find(stbl, b"stsc");
    let entries: Vec<(usize, usize)> = (0..be32(stsc, 4) as usize)
        .map(|n| {
            (
                be32(stsc, 8 + n * 12) as usize,
                be32(stsc, 12 + n * 12) as usize,
            )
        })
        .collect();
    let mut samples = Vec::new();
    let mut sizes = sizes.iter();
    for (index, &offset) in chunks.iter().enumerate() {
        let per_chunk = entries
            .iter()
            .rev()
            .find(|(first, _)| *first <= index + 1)
            .unwrap()
            .1;
        let mut at = offset;
        for _ in 0..per_chunk {
            let size = *sizes.next().unwrap();
            samples.push(file[at..at + size].to_vec());
            at += size;
        }
    }
    assert!(sizes.next().is_none(), "every sample is in a chunk");
    Track {
        id: be32(tkhd, 12),
        lengths: (be32(tkhd, 20), be32(elst, 8)),
        media_length,
        samples,
    }
}

/// The bytes of the samples `testing::stream` puts in its fragments `0..fragments`.
fn stream_samples(fill: u8, fragments: u8) -> Vec<Vec<u8>> {
    (0..fragments)
        .flat_map(|n| [vec![fill + n; 3 + n as usize], vec![fill + n; 2]])
        .collect()
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// A video at 90 kHz with fragments at 0, 2 and 4 s, and a sound at 44.1 kHz with fragments at
/// 0, 1.5, 3 and 4.5 s; returns their paths and the video's length.
fn streams(dir: &Path) -> (PathBuf, u64, PathBuf) {
    let video = stream(1, 1000, 90_000, b"vide", &[0, 180_000, 360_000], 0x10);
    let len = video.len() as u64;
    let audio = stream(
        1,
        44_100,
        44_100,
        b"soun",
        &[0, 66_150, 132_300, 198_450],
        0x80,
    );
    (
        write(dir, "video.mp4", &video),
        len,
        write(dir, "audio.m4a", &audio),
    )
}

#[test]
fn the_video_becomes_an_unfragmented_file_with_the_sound_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let (video, len, audio) = streams(dir.path());
    let before = std::fs::read(&video).unwrap();
    join_in_place(&video, len, &audio).unwrap();
    let file = std::fs::read(&video).unwrap();

    let top = walk(&file);
    let kinds: Vec<[u8; 4]> = top.iter().map(|(k, ..)| *k).collect();
    assert_eq!(&kinds[0], b"ftyp");
    assert_eq!(&file[8..12], b"isom", "an ordinary MP4 brand");
    assert_eq!(kinds.last().unwrap(), b"moov", "the index comes last");
    assert_eq!(kinds.iter().filter(|k| *k == b"moov").count(), 1);
    assert!(!kinds.iter().any(|k| k == b"moof" || k == b"sidx"));
    assert_eq!(kinds.iter().filter(|k| *k == b"mdat").count(), 3 + 4);
    // The video's samples stay where they were downloaded.
    for (kind, start, end) in walk(&before) {
        if &kind == b"mdat" {
            assert!(file[start..end] == before[start..end]);
        }
    }

    let (_, start, end) = *top.last().unwrap();
    let moov = &file[start + 8..end];
    let mvhd = find(moov, b"mvhd");
    assert_eq!(be32(mvhd, 96), 3, "next track ID");
    assert_eq!(be32(mvhd, 16), 4500, "the longer track, in ms");
    assert!(walk(moov).iter().all(|(k, ..)| k != b"mvex"));
    let tracks: Vec<Track> = walk(moov)
        .into_iter()
        .filter(|(k, ..)| k == b"trak")
        .map(|(_, start, end)| track(&file, &moov[start + 8..end]))
        .collect();
    assert_eq!(tracks.iter().map(|t| t.id).collect::<Vec<_>>(), [1, 2]);
    // Each fragment's gap to the next lengthens its last sample, so the tracks keep time.
    assert_eq!(tracks[0].media_length, 360_000);
    assert_eq!(tracks[0].lengths, (4000, 4000));
    assert_eq!(tracks[1].media_length, 198_450);
    assert_eq!(tracks[1].lengths, (4500, 4500));
    assert_eq!(tracks[0].samples, stream_samples(0x10, 3));
    assert_eq!(tracks[1].samples, stream_samples(0x80, 4));
}

#[test]
fn a_join_that_runs_again_gives_the_same_file() {
    let dir = tempfile::tempdir().unwrap();
    let (video, len, audio) = streams(dir.path());
    join_in_place(&video, len, &audio).unwrap();
    let first = std::fs::read(&video).unwrap();
    // As after a join that stopped part way: it starts again from the renamed boxes.
    join_in_place(&video, len, &audio).unwrap();
    assert!(std::fs::read(&video).unwrap() == first);
}

#[test]
fn streams_that_cant_be_joined_here_say_why_and_are_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (video, len, audio) = streams(dir.path());
    // Absolute data offsets: tfhd's base-data-offset flag.
    let mut bytes = std::fs::read(&video).unwrap();
    let tfhd = bytes.windows(4).position(|w| w == b"tfhd").unwrap();
    bytes[tfhd + 7] |= 1;
    std::fs::write(&video, &bytes).unwrap();
    assert!(matches!(
        join_in_place(&video, len, &audio),
        Err(Error::Unsupported(reason)) if reason.contains("absolute")
    ));
    assert!(
        std::fs::read(&video).unwrap() == bytes,
        "nothing was written"
    );

    // No mvex: an ordinary, unfragmented MP4.
    let plain = [
        mp4_box(b"ftyp", b"isom\0\0\0\0"),
        mp4_box(
            b"moov",
            &[mvhd(1000, 0), trak(1, 90_000, b"vide", 0)].concat(),
        ),
    ]
    .concat();
    let plain_path = write(dir.path(), "plain.mp4", &plain);
    assert!(matches!(
        join_in_place(&plain_path, plain.len() as u64, &audio),
        Err(Error::Unsupported(reason)) if reason.contains("fragmented")
    ));
    let garbage = write(dir.path(), "garbage.mp4", b"not an mp4 file at all");
    assert!(matches!(
        join_in_place(&garbage, 22, &audio),
        Err(Error::Malformed(_))
    ));
}
