//! Applies the user's appearance to Velo's windows.
//!
//! Slint globals belong to each window, so every window applies the setting when it opens.

use slint::ComponentHandle;
use velo_core::Appearance as Setting;

use crate::{Appearance, Theme};

/// The UI's appearance for a setting.
pub fn to_ui(setting: Setting) -> Appearance {
    match setting {
        Setting::System => Appearance::System,
        Setting::Light => Appearance::Light,
        Setting::Dark => Appearance::Dark,
    }
}

/// The setting for a UI appearance.
pub fn from_ui(appearance: Appearance) -> Setting {
    match appearance {
        Appearance::System => Setting::System,
        Appearance::Light => Setting::Light,
        Appearance::Dark => Setting::Dark,
    }
}

/// Gives `window` the appearance `setting`.
pub fn apply<'a, C>(window: &'a C, setting: Setting)
where
    C: ComponentHandle,
    Theme<'a>: slint::Global<'a, C>,
{
    window.global::<Theme>().invoke_apply(to_ui(setting));
}
