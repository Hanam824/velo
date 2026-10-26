//! Velo Download Manager desktop app.

// No console window behind the app in Windows release builds (no-op elsewhere).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod categories;
#[cfg(test)]
mod snapshot;
mod toolbar;

use std::cell::RefCell;
use std::rc::Rc;

use slint::VecModel;
use velo_core::Settings;

use crate::categories::CategoryTree;

slint::include_modules!();

/// Creates the main window with the app's version and the Categories tree filled in.
fn create_window(settings: &Settings) -> Result<AppWindow, slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    let tree = Rc::new(RefCell::new(CategoryTree::new(&settings.categories)));
    let rows = Rc::new(VecModel::from(tree.borrow().visible()));
    ui.set_categories(rows.clone().into());
    ui.on_category_toggled(move |key| {
        tree.borrow_mut().toggle(&key);
        rows.set_vec(tree.borrow().visible());
    });
    toolbar::ToolbarState::new(None, &[]).apply(&ui);
    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    Ok(ui)
}

fn main() -> Result<(), slint::PlatformError> {
    create_window(&Settings::default())?.run()
}

#[cfg(test)]
mod tests {
    use slint::Model;

    use super::*;

    #[test]
    fn window_shows_the_version_categories_and_switches_appearance() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = create_window(&Settings::default()).unwrap();
        assert_eq!(ui.get_version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(ui.get_categories().row_data(0).unwrap().key, "all");
        let rows = ui.get_categories().row_count();
        ui.invoke_category_toggled("finished".into());
        assert!(ui.get_categories().row_count() > rows);
        assert_eq!(ui.get_selected_category(), "all");

        let theme = ui.global::<Theme>();
        assert_eq!(theme.get_appearance(), Appearance::System);
        theme.invoke_apply(Appearance::Dark);
        assert!(theme.get_dark());
        theme.invoke_apply(Appearance::Light);
        assert!(!theme.get_dark());
    }

    /// Writes `main-light.png` and `main-dark.png` to `$VELO_SNAPSHOT_DIR`; see [`snapshot`].
    #[test]
    #[ignore = "installs its own Slint platform; run alone with --ignored"]
    fn snapshot() {
        let dir = std::path::PathBuf::from(
            std::env::var_os("VELO_SNAPSHOT_DIR").expect("set VELO_SNAPSHOT_DIR"),
        );
        std::fs::create_dir_all(&dir).unwrap();
        let window = snapshot::init();
        let ui = create_window(&Settings::default()).unwrap();
        for (appearance, name) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
            ui.global::<Theme>().invoke_apply(appearance);
            snapshot::save(
                &window,
                &ui,
                1000,
                620,
                &dir.join(format!("main-{name}.png")),
            );
        }
    }
}
