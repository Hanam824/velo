//! Builders of small fragmented MP4 streams, for tests of the joiner and of what uses it.
//!
//! Only built with the `testing` feature.

/// A box of type `kind` holding `payload`.
pub fn mp4_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    [
        &((payload.len() + 8) as u32).to_be_bytes()[..],
        kind,
        payload,
    ]
    .concat()
}

/// Version 0 and no flags.
pub const FULL: [u8; 4] = [0; 4];

/// An `mvhd` box with `timescale` and `duration`.
pub fn mvhd(timescale: u32, duration: u32) -> Vec<u8> {
    let mut p = FULL.to_vec();
    p.extend_from_slice(&[0; 8]); // creation and modification time
    p.extend_from_slice(&timescale.to_be_bytes());
    p.extend_from_slice(&duration.to_be_bytes());
    p.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate
    p.extend_from_slice(&0x0100u16.to_be_bytes()); // volume
    p.extend_from_slice(&[0; 10]);
    p.extend_from_slice(&[0; 36]); // matrix
    p.extend_from_slice(&[0; 24]);
    p.extend_from_slice(&2u32.to_be_bytes()); // next track ID
    mp4_box(b"mvhd", &p)
}

/// A `trak` box for track `id` whose media runs at `timescale`, with a `handler` such as
/// `vide`, and an edit `edit_duration` long in the movie timescale.
pub fn trak(id: u32, timescale: u32, handler: &[u8; 4], edit_duration: u32) -> Vec<u8> {
    let mut tkhd = vec![0, 0, 0, 3];
    tkhd.extend_from_slice(&[0; 8]);
    tkhd.extend_from_slice(&id.to_be_bytes());
    tkhd.extend_from_slice(&[0; 4]);
    tkhd.extend_from_slice(&edit_duration.to_be_bytes());
    tkhd.extend_from_slice(&[0; 8 + 8 + 36 + 8]);
    let mut mdhd = FULL.to_vec();
    mdhd.extend_from_slice(&[0; 8]);
    mdhd.extend_from_slice(&timescale.to_be_bytes());
    mdhd.extend_from_slice(&[0; 8]);
    let mut hdlr = FULL.to_vec();
    hdlr.extend_from_slice(&[0; 4]);
    hdlr.extend_from_slice(handler);
    hdlr.extend_from_slice(&[0; 13]);
    let mut elst = FULL.to_vec();
    elst.extend_from_slice(&1u32.to_be_bytes());
    elst.extend_from_slice(&edit_duration.to_be_bytes());
    elst.extend_from_slice(&0i32.to_be_bytes());
    elst.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    let stbl = mp4_box(b"stbl", &mp4_box(b"stsd", &[0; 8]));
    mp4_box(
        b"trak",
        &[
            mp4_box(b"tkhd", &tkhd),
            mp4_box(b"edts", &mp4_box(b"elst", &elst)),
            mp4_box(
                b"mdia",
                &[
                    mp4_box(b"mdhd", &mdhd),
                    mp4_box(b"hdlr", &hdlr),
                    mp4_box(b"minf", &stbl),
                ]
                .concat(),
            ),
        ]
        .concat(),
    )
}

/// A `moof` + `mdat` of `samples` (their bytes) starting at decode time `time`.
pub fn fragment(sequence: u32, track: u32, time: u64, samples: &[Vec<u8>]) -> Vec<u8> {
    let build = |data_offset: u32| {
        let mut tfhd = vec![0, 2, 0, 0]; // default-base-is-moof
        tfhd.extend_from_slice(&track.to_be_bytes());
        let mut tfdt = vec![1, 0, 0, 0];
        tfdt.extend_from_slice(&time.to_be_bytes());
        let mut trun = vec![0, 0, 2, 1]; // data offset and sample sizes
        trun.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        trun.extend_from_slice(&data_offset.to_be_bytes());
        for sample in samples {
            trun.extend_from_slice(&(sample.len() as u32).to_be_bytes());
        }
        let mut mfhd = FULL.to_vec();
        mfhd.extend_from_slice(&sequence.to_be_bytes());
        mp4_box(
            b"moof",
            &[
                mp4_box(b"mfhd", &mfhd),
                mp4_box(
                    b"traf",
                    &[
                        mp4_box(b"tfhd", &tfhd),
                        mp4_box(b"tfdt", &tfdt),
                        mp4_box(b"trun", &trun),
                    ]
                    .concat(),
                ),
            ]
            .concat(),
        )
    };
    let moof_len = build(0).len() as u32;
    [build(moof_len + 8), mp4_box(b"mdat", &samples.concat())].concat()
}

/// A one-track fragmented stream: fragments at `times` (in `timescale` ticks), each with two
/// samples whose bytes are `fill` plus the fragment's number.
pub fn stream(
    track: u32,
    movie_timescale: u32,
    timescale: u32,
    handler: &[u8; 4],
    times: &[u64],
    fill: u8,
) -> Vec<u8> {
    let mut trex = FULL.to_vec();
    trex.extend_from_slice(&track.to_be_bytes());
    trex.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let moov = mp4_box(
        b"moov",
        &[
            mvhd(movie_timescale, movie_timescale * 5),
            trak(track, timescale, handler, movie_timescale * 5),
            mp4_box(b"mvex", &mp4_box(b"trex", &trex)),
        ]
        .concat(),
    );
    let mut file = [mp4_box(b"ftyp", b"dash\0\0\0\0iso6mp41"), moov].concat();
    // A segment index, which points into this file and has to go.
    file.extend(mp4_box(b"sidx", &[0; 24]));
    for (n, &time) in times.iter().enumerate() {
        let samples = [vec![fill + n as u8; 3 + n], vec![fill + n as u8; 2]];
        file.extend(fragment(n as u32 + 1, track, time, &samples));
    }
    file
}
