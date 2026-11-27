use crate::ui::desktop::theme::TRAY_ICON_PNG;
use eframe::egui;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStatus {
    TrackingActive,
    Paused,
    SignInNeeded,
}

impl TrayStatus {
    pub fn derive(paused: bool, signed_in: bool) -> Self {
        if paused {
            Self::Paused
        } else if !signed_in {
            Self::SignInNeeded
        } else {
            Self::TrackingActive
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::TrackingActive => "Tracking Active",
            Self::Paused => "Paused",
            Self::SignInNeeded => "Sign In Needed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    OpenWindow,
    TogglePause,
    ToggleLaunchAtLogin,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayModel {
    pub status: TrayStatus,
    pub paused: bool,
    pub signed_in: bool,
    pub launch_at_login: bool,
    pub window_visible: bool,
    pub quit_requested: bool,
    pub active_game_label: Option<String>,
}

impl TrayModel {
    pub fn new(paused: bool, signed_in: bool, launch_at_login: bool, window_visible: bool) -> Self {
        Self {
            status: TrayStatus::derive(paused, signed_in),
            paused,
            signed_in,
            launch_at_login,
            window_visible,
            quit_requested: false,
            active_game_label: None,
        }
    }

    pub fn sync_flags(
        &mut self,
        paused: bool,
        signed_in: bool,
        launch_at_login: bool,
        active_game_label: Option<String>,
    ) {
        self.paused = paused;
        self.signed_in = signed_in;
        self.launch_at_login = launch_at_login;
        self.status = TrayStatus::derive(paused, signed_in);
        self.active_game_label = active_game_label;
    }

    /// Returns `true` when the native window should actually terminate, or `false` when
    /// closing the window should minimize/hide to the tray while tracking continues.
    pub fn handle_window_close_requested(&mut self) -> bool {
        if self.quit_requested {
            true
        } else {
            self.window_visible = false;
            false
        }
    }

    pub fn handle_open_requested(&mut self) {
        self.window_visible = true;
    }

    pub fn handle_quit_requested(&mut self) {
        self.quit_requested = true;
        self.window_visible = false;
    }

    pub fn tooltip_summary(&self) -> String {
        match (&self.active_game_label, self.status) {
            (Some(game), TrayStatus::TrackingActive) => format!("OneHundo — Tracking {}", game),
            (_, status) => format!("OneHundo — {}", status.label()),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux_tray {
    use super::*;
    use ksni::blocking::TrayMethods;
    use std::sync::LazyLock;

    static LINUX_ICON: LazyLock<Option<ksni::Icon>> = LazyLock::new(|| {
        let icon = eframe::icon_data::from_png_bytes(TRAY_ICON_PNG).ok()?;
        let mut data = icon.rgba;
        for px in data.chunks_exact_mut(4) {
            px.rotate_right(1); // RGBA -> ARGB network order expected by StatusNotifierItem
        }
        Some(ksni::Icon {
            width: icon.width as i32,
            height: icon.height as i32,
            data,
        })
    });

    pub struct OneHundoLinuxTray {
        pub model: TrayModel,
        pub tx: Sender<TrayCommand>,
        pub ctx_holder: Arc<Mutex<Option<egui::Context>>>,
    }

    impl OneHundoLinuxTray {
        fn wake_ui(&self) {
            if let Some(ctx) = self.ctx_holder.lock().unwrap().as_ref() {
                ctx.request_repaint();
            }
        }

        fn send_and_wake(&self, cmd: TrayCommand) {
            let _ = self.tx.send(cmd);
            if matches!(cmd, TrayCommand::OpenWindow) {
                if let Some(ctx) = self.ctx_holder.lock().unwrap().as_ref() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.request_repaint();
                }
            } else {
                self.wake_ui();
            }
        }
    }

    impl ksni::Tray for OneHundoLinuxTray {
        fn id(&self) -> String {
            "onehundo".to_string()
        }

        fn title(&self) -> String {
            self.model.tooltip_summary()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            LINUX_ICON.iter().cloned().collect()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "OneHundo".to_string(),
                description: self.model.status.label().to_string(),
                ..Default::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            self.send_and_wake(TrayCommand::OpenWindow);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::*;
            let pause_label = if self.model.paused {
                "Resume Tracking"
            } else {
                "Pause Tracking"
            };
            vec![
                StandardItem {
                    label: self.model.status.label().to_string(),
                    enabled: false,
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Open OneHundo".to_string(),
                    activate: Box::new(|this: &mut Self| {
                        this.send_and_wake(TrayCommand::OpenWindow);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: pause_label.to_string(),
                    activate: Box::new(|this: &mut Self| {
                        this.send_and_wake(TrayCommand::TogglePause);
                    }),
                    ..Default::default()
                }
                .into(),
                CheckmarkItem {
                    label: "Launch at Login".to_string(),
                    checked: self.model.launch_at_login,
                    activate: Box::new(|this: &mut Self| {
                        this.send_and_wake(TrayCommand::ToggleLaunchAtLogin);
                    }),
                    ..Default::default()
                }
                .into(),
                MenuItem::Separator,
                StandardItem {
                    label: "Quit".to_string(),
                    activate: Box::new(|this: &mut Self| {
                        this.send_and_wake(TrayCommand::Quit);
                        if let Some(ctx) = this.ctx_holder.lock().unwrap().as_ref() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            ctx.request_repaint();
                        }
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    pub struct TrayHandle(ksni::blocking::Handle<OneHundoLinuxTray>);

    impl TrayHandle {
        pub fn update(&self, model: &TrayModel) {
            let next = model.clone();
            let _ = self.0.update(move |tray| {
                tray.model = next;
            });
        }
    }

    pub fn spawn_platform_tray(
        model: TrayModel,
        tx: Sender<TrayCommand>,
        ctx_holder: Arc<Mutex<Option<egui::Context>>>,
    ) -> Option<TrayHandle> {
        let tray = OneHundoLinuxTray {
            model,
            tx,
            ctx_holder,
        };
        match tray.spawn() {
            Ok(handle) => Some(TrayHandle(handle)),
            Err(err) => {
                eprintln!("[onehundo-tray] StatusNotifierItem tray unavailable: {}", err);
                None
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod fallback_tray {
    use super::*;

    pub struct TrayHandle;

    impl TrayHandle {
        pub fn update(&self, _model: &TrayModel) {}
    }

    pub fn spawn_platform_tray(
        _model: TrayModel,
        _tx: Sender<TrayCommand>,
        _ctx_holder: Arc<Mutex<Option<egui::Context>>>,
    ) -> Option<TrayHandle> {
        None
    }
}

#[cfg(target_os = "linux")]
pub use linux_tray::{spawn_platform_tray, TrayHandle};

#[cfg(not(target_os = "linux"))]
pub use fallback_tray::{spawn_platform_tray, TrayHandle};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_status_derives_correctly_across_states() {
        assert_eq!(TrayStatus::derive(false, true), TrayStatus::TrackingActive);
        assert_eq!(TrayStatus::derive(false, false), TrayStatus::SignInNeeded);
        assert_eq!(TrayStatus::derive(true, true), TrayStatus::Paused);
        assert_eq!(TrayStatus::derive(true, false), TrayStatus::Paused);
        assert_eq!(TrayStatus::TrackingActive.label(), "Tracking Active");
        assert_eq!(TrayStatus::Paused.label(), "Paused");
        assert_eq!(TrayStatus::SignInNeeded.label(), "Sign In Needed");
    }

    #[test]
    fn window_close_hides_to_tray_until_quit_is_requested() {
        let mut model = TrayModel::new(false, true, false, true);
        assert!(model.window_visible);
        // Clicking window X hides window instead of quitting tracking
        assert!(!model.handle_window_close_requested());
        assert!(!model.window_visible);
        assert!(!model.quit_requested);

        // Clicking Open OneHundo brings window back
        model.handle_open_requested();
        assert!(model.window_visible);

        // Selecting Quit from tray authorizes true exit
        model.handle_quit_requested();
        assert!(model.handle_window_close_requested());
    }
}
