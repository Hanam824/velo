//! Velo Download Manager desktop app.

// No console window behind the app in Windows release builds (no-op elsewhere).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

/// Creates the main window with the app's version filled in.
fn create_window() -> Result<AppWindow, slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    Ok(ui)
}

fn main() -> Result<(), slint::PlatformError> {
    create_window()?.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_shows_the_version_and_switches_appearance() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = create_window().unwrap();
        assert_eq!(ui.get_version(), env!("CARGO_PKG_VERSION"));

        let theme = ui.global::<Theme>();
        assert_eq!(theme.get_appearance(), Appearance::System);
        theme.invoke_apply(Appearance::Dark);
        assert!(theme.get_dark());
        theme.invoke_apply(Appearance::Light);
        assert!(!theme.get_dark());
    }
}
