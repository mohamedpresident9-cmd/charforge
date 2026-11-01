mod model;
mod pipeline;
mod store;
mod transport;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;

fn main() -> eframe::Result<()> {
    let catalog = model::Catalog::load();
    let root = library_root();

    let store = match store::Store::open(root.clone(), catalog) {
        Ok(store) => Arc::new(store),
        Err(error) => {
            eprintln!(
                "cannot open the local library at {}: {error}",
                root.display()
            );
            eprintln!("set CHARFORGE_LIBRARY to a writable directory and retry");
            std::process::exit(2);
        }
    };

    let source: Arc<dyn transport::Source> = Arc::new(transport::DemoSource);
    // Three concurrent transfers, three attempts per payload.
    let engine = pipeline::Engine::new(Arc::clone(&store), source, 3, 3);

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1180.0, 780.0]),
        ..Default::default()
    };

    eframe::run_native(
        "CharForge Studio",
        options,
        Box::new(move |_cc| Ok(Box::new(ui::App::new(store, engine)))),
    )
}

/// Library location, in order of preference:
/// 1. `CHARFORGE_LIBRARY` (explicit override, also used by the smoke test),
/// 2. Windows: `%APPDATA%\CharForge\library`,
/// 3. Linux/macOS: `$XDG_DATA_HOME/charforge/library` or `~/.local/share/charforge/library`,
/// 4. `.\charforge-library` next to the executable.
fn library_root() -> PathBuf {
    if let Ok(custom) = std::env::var("CHARFORGE_LIBRARY") {
        return PathBuf::from(custom);
    }

    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let candidate = PathBuf::from(appdata).join("CharForge").join("library");
            if std::fs::create_dir_all(&candidate).is_ok() {
                return candidate;
            }
        }
    }

    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            let candidate = PathBuf::from(xdg).join("charforge").join("library");
            if std::fs::create_dir_all(&candidate).is_ok() {
                return candidate;
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            let candidate = PathBuf::from(home).join(".local/share/charforge/library");
            if std::fs::create_dir_all(&candidate).is_ok() {
                return candidate;
            }
        }
    }

    PathBuf::from("charforge-library")
}
