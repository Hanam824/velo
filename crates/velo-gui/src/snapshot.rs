//! Draws windows into PNG files with Slint's software renderer, to look at the UI without a
//! display (and in reviews).
//!
//! The renderer is installed as the process's Slint platform, which can happen only once and
//! conflicts with the headless test backend, so the snapshot test is `#[ignore]`d and run on its
//! own:
//!
//! ```sh
//! VELO_SNAPSHOT_DIR=/tmp/velo cargo test -p velo-gui snapshot -- --ignored
//! ```

use std::path::Path;
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, PhysicalSize};

/// A platform whose only window is an offscreen software-rendered one.
struct SnapshotPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for SnapshotPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

/// Installs the offscreen platform and returns its window.
pub fn init() -> Rc<MinimalSoftwareWindow> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(SnapshotPlatform(window.clone())))
        .expect("the snapshot test must run on its own");
    window
}

/// Shows `component` at `width`×`height` and writes what it draws to the PNG file `path`.
pub fn save(
    window: &MinimalSoftwareWindow,
    component: &impl ComponentHandle,
    width: u32,
    height: u32,
    path: &Path,
) {
    component.show().unwrap();
    window.set_size(PhysicalSize::new(width, height));
    let image = component.window().take_snapshot().unwrap();

    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(image.as_bytes())
        .unwrap();
}
