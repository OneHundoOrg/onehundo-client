pub mod app;
pub mod auth;
pub mod backend;
pub mod theme;
pub mod tray;

use app::OneHundoApp;
use backend::DesktopBackend;
use eframe::egui;
use theme::load_app_icon;

pub fn launch_desktop_gui(start_minimized: bool) -> Result<(), String> {
    let backend = DesktopBackend::connect_or_start_embedded()?;

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("OneHundo")
        .with_inner_size([860.0, 620.0])
        .with_min_inner_size([680.0, 480.0])
        .with_visible(!start_minimized);

    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "OneHundo",
        options,
        Box::new(move |cc| Ok(Box::new(OneHundoApp::new(cc, backend, start_minimized)))),
    )
    .map_err(|e| format!("Failed launching OneHundo desktop window: {}", e))
}
