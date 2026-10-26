//! Joining synthetic fragmented MP4 streams built here, box by box.

use std::path::Path;

use velo_mux::testing::{mp4_box, mvhd, stream, trak};
use velo_mux::{Error, mux_mp4};

/// What the joined file holds, read back with a parser written for this test.
struct Parsed {
    traks: Vec<u32>,
    trexes: Vec<u32>,
    next_track: u32,
    /// `mvhd`'s duration and `mehd`'s.
    durations: (u32, u64),
    /// For each fragment: sequence, track, sample count and its sample bytes.
    fragments: Vec<(u32, u32, u32, Vec<u8>)>,
    audio_edit: u32,
}

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

fn parse(file: &[u8]) -> Parsed {
    let top = walk(file);
    let kinds: Vec<[u8; 4]> = top.iter().map(|(k, ..)| *k).collect();
    assert_eq!(&kinds[..2], [*b"ftyp", *b"moov"]);
    assert!(!kinds.contains(b"sidx"));
    let moov = &file[top[1].1 + 8..top[1].2];
    let mvhd = find(moov, b"mvhd");
    let mut traks = Vec::new();
    let mut audio_edit = 0;
    for (kind, start, end) in walk(moov) {
        if &kind == b"trak" {
            let trak = &moov[start + 8..end];
            let id = be32(find(trak, b"tkhd"), 12);
            if id == 2 {
                audio_edit = be32(find(find(trak, b"edts"), b"elst"), 8);
            }
            traks.push(id);
        }
    }
    let trexes = walk(find(moov, b"mvex"))
        .into_iter()
        .filter(|(k, ..)| k == b"trex")
        .map(|(_, start, _)| be32(find(moov, b"mvex"), start + 8 + 4))
        .collect();
    let mut fragments = Vec::new();
    for (i, &(kind, start, end)) in top.iter().enumerate() {
        if &kind != b"moof" {
            continue;
        }
        let moof = &file[start + 8..end];
        let sequence = be32(find(moof, b"mfhd"), 4);
        let traf = find(moof, b"traf");
        let track = be32(find(traf, b"tfhd"), 4);
        let trun = find(traf, b"trun");
        let count = be32(trun, 4);
        let data_offset = be32(trun, 8) as usize;
        let (mdat_kind, mdat_start, mdat_end) = top[i + 1];
        assert_eq!(&mdat_kind, b"mdat");
        assert_eq!(
            start + data_offset,
            mdat_start + 8,
            "the data offset still points at the samples"
        );
        fragments.push((
            sequence,
            track,
            count,
            file[mdat_start + 8..mdat_end].to_vec(),
        ));
    }
    let mvex = find(moov, b"mvex");
    let mehd = find(mvex, b"mehd");
    assert_eq!(mehd[0], 1, "a 64-bit mehd");
    Parsed {
        traks,
        trexes,
        next_track: be32(mvhd, 96),
        durations: (
            be32(mvhd, 16),
            u64::from_be_bytes(mehd[4..12].try_into().unwrap()),
        ),
        fragments,
        audio_edit,
    }
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn video_and_audio_fragments_are_interleaved_by_time_in_one_file() {
    let dir = tempfile::tempdir().unwrap();
    // Video at 90 kHz: fragments at 0, 2 and 4 s. Audio at 44.1 kHz: 0, 1.5, 3 and 4.5 s.
    let video = write(
        dir.path(),
        "video.mp4",
        &stream(1, 1000, 90_000, b"vide", &[0, 180_000, 360_000], 0x10),
    );
    let audio = write(
        dir.path(),
        "audio.m4a",
        &stream(
            1,
            44_100,
            44_100,
            b"soun",
            &[0, 66_150, 132_300, 198_450],
            0x80,
        ),
    );
    let out = dir.path().join("joined.mp4");
    mux_mp4(&video, &audio, &out).unwrap();

    let parsed = parse(&std::fs::read(&out).unwrap());
    assert_eq!(parsed.traks, [1, 2]);
    assert_eq!(parsed.trexes, [1, 2]);
    assert_eq!(parsed.next_track, 3);
    // The length is given once, in mehd (5 s in the video's movie timescale); the tracks' own
    // durations are 0, since their samples are all in fragments.
    assert_eq!(parsed.durations, (0, 5 * 1000));
    assert_eq!(parsed.audio_edit, 0);

    let order: Vec<(u32, u32)> = parsed
        .fragments
        .iter()
        .map(|(sequence, track, ..)| (*sequence, *track))
        .collect();
    assert_eq!(
        order,
        [(1, 1), (2, 2), (3, 2), (4, 1), (5, 2), (6, 1), (7, 2)],
        "0 s video, 0 s audio, 1.5 s audio, 2 s video, 3 s audio, 4 s video, 4.5 s audio"
    );
    let samples: u32 = parsed.fragments.iter().map(|f| f.2).sum();
    assert_eq!(samples, 2 * (3 + 4));
    // The second video fragment's samples, copied byte for byte.
    assert_eq!(
        parsed.fragments[3].3,
        [vec![0x11; 4], vec![0x11; 2]].concat()
    );
    assert_eq!(
        parsed.fragments[6].3,
        [vec![0x83; 6], vec![0x83; 2]].concat()
    );
}

#[test]
fn streams_that_cant_be_joined_here_say_why() {
    let dir = tempfile::tempdir().unwrap();
    let audio = write(
        dir.path(),
        "audio.m4a",
        &stream(1, 1000, 44_100, b"soun", &[0], 0x80),
    );
    // No mvex: an ordinary, unfragmented MP4.
    let plain = write(
        dir.path(),
        "plain.mp4",
        &[
            mp4_box(b"ftyp", b"isom\0\0\0\0"),
            mp4_box(
                b"moov",
                &[mvhd(1000, 0), trak(1, 90_000, b"vide", 0)].concat(),
            ),
        ]
        .concat(),
    );
    let out = dir.path().join("out.mp4");
    assert!(matches!(
        mux_mp4(&plain, &audio, &out),
        Err(Error::Unsupported(reason)) if reason.contains("fragmented")
    ));
    let garbage = write(dir.path(), "garbage.mp4", b"not an mp4 file at all");
    assert!(matches!(
        mux_mp4(&garbage, &audio, &out),
        Err(Error::Malformed(_))
    ));
}
