//! Reading and patching ISO base media (MP4) boxes.
//!
//! A box is a 32-bit size, a four-letter type and a payload; a size of 1 means a 64-bit size
//! follows the type, and 0 means the box runs to the end of its container.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use crate::{Error, Result};

/// Where a box is: inside a buffer, or inside a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxRef {
    /// The four-letter type.
    pub kind: [u8; 4],
    /// The offset of the box's first byte.
    pub start: u64,
    /// The length of the size and type fields.
    pub header: u64,
    /// The offset just past the box.
    pub end: u64,
}

impl BoxRef {
    /// The offset of the payload's first byte.
    pub fn payload(&self) -> u64 {
        self.start + self.header
    }

    /// The whole box's length.
    pub fn len(&self) -> u64 {
        self.end - self.start
    }
}

/// The boxes in `buf[from..to]`, in order.
pub fn boxes_in(buf: &[u8], from: usize, to: usize) -> Result<Vec<BoxRef>> {
    let mut found = Vec::new();
    let mut at = from;
    while at < to {
        if to - at < 8 {
            return Err(Error::Malformed("a box header is cut off".into()));
        }
        let size32 = u32_at(buf, at);
        let kind: [u8; 4] = buf[at + 4..at + 8].try_into().unwrap();
        let (header, size) = match size32 {
            0 => (8, (to - at) as u64),
            1 => {
                if to - at < 16 {
                    return Err(Error::Malformed("a box header is cut off".into()));
                }
                (16, u64_at(buf, at + 8))
            }
            n => (8, u64::from(n)),
        };
        let end = at as u64 + size;
        if size < header || end > to as u64 {
            return Err(Error::Malformed(format!(
                "box {} doesn't fit its container",
                name(kind)
            )));
        }
        found.push(BoxRef {
            kind,
            start: at as u64,
            header,
            end,
        });
        at = end as usize;
    }
    Ok(found)
}

/// The children of box `parent` in `buf`; the payload of a container box is only boxes.
pub fn children(buf: &[u8], parent: &BoxRef) -> Result<Vec<BoxRef>> {
    boxes_in(buf, parent.payload() as usize, parent.end as usize)
}

/// The first child of `parent` of type `kind`.
pub fn child(buf: &[u8], parent: &BoxRef, kind: &[u8; 4]) -> Result<Option<BoxRef>> {
    Ok(children(buf, parent)?.into_iter().find(|b| &b.kind == kind))
}

/// The box reached from `parent` through the types in `path`, such as `mdia/mdhd`.
pub fn descendant(buf: &[u8], parent: &BoxRef, path: &[&[u8; 4]]) -> Result<Option<BoxRef>> {
    let mut current = *parent;
    for kind in path {
        match child(buf, &current, kind)? {
            Some(found) => current = found,
            None => return Ok(None),
        }
    }
    Ok(Some(current))
}

/// The top-level boxes of `file`, read from their headers only.
pub fn top_level(file: &mut File) -> Result<Vec<BoxRef>> {
    let len = file.metadata()?.len();
    let mut found = Vec::new();
    let mut at = 0u64;
    while at < len {
        file.seek(SeekFrom::Start(at))?;
        let mut head = [0u8; 16];
        let available = (len - at).min(16) as usize;
        file.read_exact(&mut head[..available])?;
        if available < 8 {
            return Err(Error::Malformed("a box header is cut off".into()));
        }
        let kind: [u8; 4] = head[4..8].try_into().unwrap();
        let (header, size) = match u32_at(&head, 0) {
            0 => (8, len - at),
            1 if available == 16 => (16, u64_at(&head, 8)),
            1 => return Err(Error::Malformed("a box header is cut off".into())),
            n => (8, u64::from(n)),
        };
        if size < header || at + size > len {
            return Err(Error::Malformed(format!(
                "box {} runs past the end of the file",
                name(kind)
            )));
        }
        found.push(BoxRef {
            kind,
            start: at,
            header,
            end: at + size,
        });
        at += size;
    }
    Ok(found)
}

/// Reads the whole of box `at` from `file`.
pub fn read_box(file: &mut File, at: &BoxRef) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(at.start))?;
    let mut buf = vec![0; at.len() as usize];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// A box of type `kind` holding `payload`.
pub fn make_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// The big-endian `u32` at `at`.
pub fn u32_at(buf: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(buf[at..at + 4].try_into().unwrap())
}

/// The big-endian `u64` at `at`.
pub fn u64_at(buf: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(buf[at..at + 8].try_into().unwrap())
}

/// Writes `value` big-endian at `at`.
pub fn set_u32(buf: &mut [u8], at: usize, value: u32) {
    buf[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

/// Writes `value` big-endian at `at`.
pub fn set_u64(buf: &mut [u8], at: usize, value: u64) {
    buf[at..at + 8].copy_from_slice(&value.to_be_bytes());
}

/// A box type for messages, such as `moov`.
pub fn name(kind: [u8; 4]) -> String {
    String::from_utf8_lossy(&kind).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_boxes_and_large_sizes_are_read() {
        let inner = make_box(b"tkhd", &[1, 2, 3]);
        let outer = make_box(b"trak", &inner);
        let mut large = vec![0, 0, 0, 1];
        large.extend_from_slice(b"mdat");
        large.extend_from_slice(&20u64.to_be_bytes());
        large.extend_from_slice(&[9, 9, 9, 9]);
        let buf = [outer.clone(), large].concat();

        let top = boxes_in(&buf, 0, buf.len()).unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(&top[1].kind, b"mdat");
        assert_eq!((top[1].header, top[1].len()), (16, 20));
        let tkhd = descendant(&buf, &top[0], &[b"tkhd"]).unwrap().unwrap();
        assert_eq!(&buf[tkhd.payload() as usize..tkhd.end as usize], &[1, 2, 3]);
        assert!(child(&buf, &top[0], b"mdia").unwrap().is_none());
    }

    #[test]
    fn a_box_larger_than_its_container_is_malformed() {
        let mut buf = make_box(b"moov", &[0; 4]);
        set_u32(&mut buf, 0, 100);
        assert!(boxes_in(&buf, 0, buf.len()).is_err());
    }
}
