//! Draws windows into PNG files with Slint's software renderer, to look at the UI without a
//! display (and in reviews), and compares them with reference images to catch a change in how
//! a window looks.
//!
//! The renderer is installed as the process's Slint platform, which can happen only once and
//! conflicts with the headless test backend, so the snapshot test is `#[ignore]`d and run on its
//! own:
//!
//! ```sh
//! VELO_SNAPSHOT_DIR=/tmp/velo cargo test -p velo-gui snapshot -- --ignored
//! ```
//!
//! With `VELO_SNAPSHOT_COMPARE=1` each image is also compared with its reference in
//! `crates/velo-gui/snapshots/`, and with `VELO_SNAPSHOT_UPDATE=1` the references are written
//! instead. Text is drawn with the system's fonts, so references only match the operating
//! system they were made on: they are made and compared on Windows.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};

/// The operating system the references are made and compared on (as in
/// [`std::env::consts::OS`]): text is drawn with the system's fonts, so another one never
/// draws the same picture.
const REFERENCE_OS: &str = "windows";

/// How far a colour channel may be off before a pixel counts as different. Small, so that a
/// theme colour that moved a few steps is caught: on one machine the renderer draws the same
/// picture every time.
const CHANNEL_TOLERANCE: u8 = 4;

/// How many pixels of a window may differ. The renderer draws the same picture every time on
/// one machine, and the smallest change worth catching, a wrong toolbar icon, changes about
/// 140 pixels.
const DIFFERENT_PIXELS_ALLOWED: usize = 32;

/// A platform whose only window is an offscreen software-rendered one.
struct SnapshotPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for SnapshotPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

/// What happens to each image besides being written to the output folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Nothing.
    Look,
    /// It is compared with its reference.
    Compare,
    /// It becomes the reference.
    Update,
}

/// Takes pictures of windows; see the [module](self).
pub struct Shots {
    /// The offscreen window every component is drawn in.
    window: Rc<MinimalSoftwareWindow>,
    /// Where every image is written.
    dir: PathBuf,
    /// Whether the images are compared with the references, or replace them.
    mode: Mode,
    /// The images that don't match their reference, and why.
    failures: Vec<String>,
}

impl Shots {
    /// Installs the offscreen platform; images go to `dir`.
    pub fn new(dir: PathBuf) -> Self {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
        slint::platform::set_platform(Box::new(SnapshotPlatform(window.clone())))
            .expect("the snapshot test must run on its own");
        let on = |name: &str| std::env::var_os(name).is_some_and(|value| value == "1");
        let mode = if on("VELO_SNAPSHOT_UPDATE") {
            Mode::Update
        } else if on("VELO_SNAPSHOT_COMPARE") {
            Mode::Compare
        } else {
            Mode::Look
        };
        assert!(
            mode != Mode::Update || std::env::consts::OS == REFERENCE_OS,
            "the reference pictures are made on {REFERENCE_OS}: this system draws text with \
             other fonts, so its pictures can't replace them"
        );
        Self {
            window,
            dir,
            mode,
            failures: Vec::new(),
        }
    }

    /// Shows `component` at `width`×`height`, writes what it draws to `<name>.png` and, when
    /// comparing, notes a difference from its reference.
    pub fn take(&mut self, component: &impl ComponentHandle, width: u32, height: u32, name: &str) {
        component.show().unwrap();
        self.window.set_size(PhysicalSize::new(width, height));
        let image = component.window().take_snapshot().unwrap();
        let pixels = image.as_bytes();
        let file = format!("{name}.png");
        write_png(&self.dir.join(&file), width, height, pixels);

        let reference = references().join(&file);
        match self.mode {
            Mode::Look => {}
            Mode::Update => {
                std::fs::create_dir_all(references()).unwrap();
                write_png(&reference, width, height, pixels);
            }
            Mode::Compare => {
                if let Err(why) = compare(&reference, width, height, pixels) {
                    self.failures.push(format!("{file}: {why}"));
                }
            }
        }
    }

    /// Panics if an image didn't match its reference, naming each one.
    pub fn finish(self) {
        let advice = if std::env::consts::OS == REFERENCE_OS {
            "If the change is meant, run again with VELO_SNAPSHOT_UPDATE=1.".to_string()
        } else {
            format!(
                "The references are made on {REFERENCE_OS}; this system draws text with other \
                 fonts, so a difference here says nothing."
            )
        };
        assert!(
            self.failures.is_empty(),
            "{} window(s) don't look like their reference; what was drawn is in {}:\n  {}\n{advice}",
            self.failures.len(),
            self.dir.display(),
            self.failures.join("\n  ")
        );
    }
}

/// The folder of reference images, in the source tree.
fn references() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots")
}

/// Writes RGBA `pixels` of a `width`×`height` image to the PNG file `path`.
fn write_png(path: &Path, width: u32, height: u32, pixels: &[u8]) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(pixels)
        .unwrap();
}

/// Says how RGBA `pixels` of a `width`×`height` image differ from the PNG file `reference`,
/// if they do by more than the tolerances.
fn compare(reference: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<(), String> {
    let file = std::fs::File::open(reference)
        .map_err(|_| "it has no reference yet; make one with VELO_SNAPSHOT_UPDATE=1".to_string())?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|e| format!("the reference can't be read: {e}"))?;
    let size = reader
        .output_buffer_size()
        .ok_or("the reference is too large")?;
    let mut expected = vec![0; size];
    let info = reader
        .next_frame(&mut expected)
        .map_err(|e| format!("the reference can't be read: {e}"))?;
    if (info.color_type, info.bit_depth) != (png::ColorType::Rgba, png::BitDepth::Eight) {
        return Err(format!(
            "the reference is {:?} with {:?} bits, not 8-bit RGBA as this test writes it",
            info.color_type, info.bit_depth
        ));
    }
    if (info.width, info.height) != (width, height) {
        return Err(format!(
            "it is {width}×{height}, the reference {}×{}",
            info.width, info.height
        ));
    }
    let different = different_pixels(&expected[..info.buffer_size()], pixels);
    if different > DIFFERENT_PIXELS_ALLOWED {
        return Err(format!(
            "{different} pixels differ, more than the {DIFFERENT_PIXELS_ALLOWED} allowed"
        ));
    }
    Ok(())
}

/// How many RGBA pixels of `a` and `b` differ by more than [`CHANNEL_TOLERANCE`] in a channel.
fn different_pixels(a: &[u8], b: &[u8]) -> usize {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(a, b)| {
            a.iter()
                .zip(*b)
                .any(|(x, y)| x.abs_diff(*y) > CHANNEL_TOLERANCE)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_matches_itself_and_small_noise_but_not_a_changed_picture() {
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("window.png");
        // 100×10 grey pixels.
        let pixels = vec![128u8; 100 * 10 * 4];
        write_png(&reference, 100, 10, &pixels);
        assert_eq!(compare(&reference, 100, 10, &pixels), Ok(()));

        // Every channel a little off (font smoothing) is fine, and so are a few pixels far
        // off; one more than allowed is a change.
        let mut noisy: Vec<u8> = pixels.iter().map(|p| p + CHANNEL_TOLERANCE).collect();
        assert_eq!(compare(&reference, 100, 10, &noisy), Ok(()));
        for pixel in 0..DIFFERENT_PIXELS_ALLOWED {
            noisy[pixel * 4] = 255;
        }
        assert_eq!(compare(&reference, 100, 10, &noisy), Ok(()));
        noisy[DIFFERENT_PIXELS_ALLOWED * 4] = 255;
        let changed = compare(&reference, 100, 10, &noisy).unwrap_err();
        assert!(changed.starts_with("33 pixels differ"), "{changed}");

        let resized = compare(&reference, 50, 10, &pixels[..50 * 10 * 4]).unwrap_err();
        assert!(resized.contains("the reference 100×10"), "{resized}");
        // A reference some tool rewrote without its alpha channel.
        let rgb = dir.path().join("rgb.png");
        let file = std::io::BufWriter::new(std::fs::File::create(&rgb).unwrap());
        let mut encoder = png::Encoder::new(file, 100, 10);
        encoder.set_color(png::ColorType::Rgb);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&vec![128u8; 100 * 10 * 3]).unwrap();
        writer.finish().unwrap();
        let recoloured = compare(&rgb, 100, 10, &pixels).unwrap_err();
        assert!(recoloured.contains("not 8-bit RGBA"), "{recoloured}");

        let missing = compare(&dir.path().join("none.png"), 100, 10, &pixels).unwrap_err();
        assert!(missing.contains("no reference"), "{missing}");
    }
}
