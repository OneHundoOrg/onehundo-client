use crate::core::types::ProgressionMode;
use crate::diagnostics::{GameHealthSnapshot, LibraryGameProgress};
use crate::ui::desktop::auth::{open_in_browser, start_browser_auth, PendingBrowserAuth};
use crate::ui::desktop::backend::{BackendActionResult, DesktopBackend};
use crate::ui::desktop::theme::{
    apply_grid_theme, display_font, empty_state, grid_card, muted_label, primary_button,
    progress_bar, secondary_button, section_heading, semibold_font, status_badge, subtle_button,
    toggle_switch, wordmark_square, ERROR, GILT, INK, INK_2, INK_3, LINE, LINE_STRONG, PAPER,
    SURFACE, SURFACE_2, TEAL, WARNING,
};
use crate::ui::desktop::tray::{
    spawn_platform_tray, TrayCommand, TrayHandle, TrayModel,
};
use eframe::egui::{self, Align, CornerRadius, Margin, RichText, ScrollArea, Stroke, Vec2};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Status,
    Library,
    Account,
}

pub struct OneHundoApp {
    backend: Option<Arc<DesktopBackend>>,
    snapshot: Option<GameHealthSnapshot>,
    snapshot_error: Option<String>,
    last_refresh: Instant,
    tab: Tab,
    banner: Option<(BackendActionResult, Instant)>,

    // Tray & window visibility state
    tray_model: TrayModel,
    tray_rx: Receiver<TrayCommand>,
    tray_handle: Option<Arc<TrayHandle>>,
    tray_ctx_holder: Arc<Mutex<Option<egui::Context>>>,
    tray_stop: Arc<AtomicBool>,
    tray_thread: Option<JoinHandle<()>>,

    // Library / folders state
    selected_game_for_folder: String, // "" => Auto-detect
    library_filter: String,
    folder_picker_rx: Option<Receiver<Option<PathBuf>>>,

    // Account / OAuth state
    pending_auth: Option<PendingBrowserAuth>,
    show_manual_key: bool,
    manual_api_key: String,
    launch_at_login: bool,
}

impl OneHundoApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        backend: DesktopBackend,
        start_minimized: bool,
    ) -> Self {
        apply_grid_theme(&cc.egui_ctx);

        let backend = Arc::new(backend);
        let snapshot = backend.fetch_snapshot().ok();
        let paused = snapshot.as_ref().map(|s| s.tracking_paused).unwrap_or(false);
        let signed_in = snapshot.as_ref().map(|s| s.account_signed_in).unwrap_or(false);
        let launch_at_login = backend.is_launch_at_login();
        let (active_label, active_detail) = snapshot
            .as_ref()
            .map(Self::extract_active_tray_info)
            .unwrap_or((None, None));

        let (tray_tx, tray_rx) = mpsc::channel();
        let mut tray_model = TrayModel::new(paused, signed_in, launch_at_login, !start_minimized);
        tray_model.sync_state(paused, signed_in, launch_at_login, active_label, active_detail);
        let tray_ctx_holder = Arc::new(Mutex::new(Some(cc.egui_ctx.clone())));
        let tray_handle = spawn_platform_tray(
            tray_model.clone(),
            tray_tx,
            Arc::clone(&tray_ctx_holder),
        )
        .map(Arc::new);

        let tray_stop = Arc::new(AtomicBool::new(false));
        let tray_thread = tray_handle.as_ref().and_then(|handle| {
            let stop_flag = Arc::clone(&tray_stop);
            let bg_backend = Arc::clone(&backend);
            let bg_handle = Arc::clone(handle);
            let bg_ctx = Arc::clone(&tray_ctx_holder);
            let mut tracked = tray_model.clone();
            std::thread::Builder::new()
                .name("onehundo-tray-sync".to_string())
                .spawn(move || {
                    while !stop_flag.load(Ordering::Relaxed) {
                        for _ in 0..6 {
                            std::thread::sleep(Duration::from_millis(250));
                            if stop_flag.load(Ordering::Relaxed) {
                                return;
                            }
                        }
                        if let Ok(snap) = bg_backend.fetch_snapshot() {
                            let (label, detail) = Self::extract_active_tray_info(&snap);
                            let launch = bg_backend.is_launch_at_login();
                            let changed = tracked.paused != snap.tracking_paused
                                || tracked.signed_in != snap.account_signed_in
                                || tracked.launch_at_login != launch
                                || tracked.active_game_label != label
                                || tracked.active_tracking_detail != detail;
                            if changed {
                                tracked.sync_state(
                                    snap.tracking_paused,
                                    snap.account_signed_in,
                                    launch,
                                    label,
                                    detail,
                                );
                                bg_handle.update(&tracked);
                                if let Some(ctx) = bg_ctx.lock().unwrap().as_ref() {
                                    ctx.request_repaint();
                                }
                            }
                        }
                    }
                })
                .ok()
        });

        Self {
            backend: Some(backend),
            snapshot,
            snapshot_error: None,
            last_refresh: Instant::now() - Duration::from_secs(10),
            tab: Tab::Status,
            banner: None,
            tray_model,
            tray_rx,
            tray_handle,
            tray_ctx_holder,
            tray_stop,
            tray_thread,
            selected_game_for_folder: String::new(),
            library_filter: String::new(),
            folder_picker_rx: None,
            pending_auth: None,
            show_manual_key: false,
            manual_api_key: String::new(),
            launch_at_login,
        }
    }

    fn set_banner(&mut self, message: impl Into<String>, is_error: bool) {
        self.banner = Some((
            BackendActionResult {
                message: message.into(),
                is_error,
            },
            Instant::now(),
        ));
    }

    fn extract_active_tray_info(snap: &GameHealthSnapshot) -> (Option<String>, Option<String>) {
        if snap.tracking_paused {
            return (None, None);
        }
        if let Some(target) = snap.active_trackings.first() {
            return (
                Some(target.game_title.clone()),
                Some(format!("{} · {}", target.source, target.detail)),
            );
        }
        if let Some(game) = Self::active_game_from_snapshot(snap) {
            return (Some(game.title), Some(game.edition_or_pack));
        }
        (None, None)
    }

    fn refresh_snapshot(&mut self) {
        let Some(ref backend) = self.backend else {
            return;
        };
        match backend.fetch_snapshot() {
            Ok(snap) => {
                let (active_label, active_detail) = Self::extract_active_tray_info(&snap);
                self.tray_model.sync_state(
                    snap.tracking_paused,
                    snap.account_signed_in,
                    self.launch_at_login,
                    active_label,
                    active_detail,
                );
                if let Some(ref handle) = self.tray_handle {
                    handle.update(&self.tray_model);
                }
                self.snapshot = Some(snap);
                self.snapshot_error = None;
            }
            Err(err) => {
                self.snapshot_error = Some(err);
            }
        }
        self.last_refresh = Instant::now();
    }

    fn active_game_from_snapshot(snap: &GameHealthSnapshot) -> Option<LibraryGameProgress> {
        if snap.tracking_paused {
            return None;
        }
        if let Some(active) = snap.library_games.iter().find(|g| g.active) {
            return Some(active.clone());
        }
        if let Some(target) = snap.active_trackings.first() {
            return Some(LibraryGameProgress {
                canonical_game_id: target.canonical_game_id.clone(),
                pack_id: target.pack_id.clone(),
                title: target.game_title.clone(),
                mapped_oap_edition: None,
                unlocked_count: target.unlocked_count,
                total_achievements: target.total_achievements,
                completion_percent: target.completion_percent,
                active: true,
                edition_or_pack: target.edition_or_pack.clone(),
            });
        }
        if let Some(sess) = snap.sessions.iter().find(|s| s.active) {
            if let Some(lib) = snap
                .library_games
                .iter()
                .find(|g| g.canonical_game_id == sess.canonical_game_id)
            {
                return Some(lib.clone());
            }
            return Some(LibraryGameProgress {
                canonical_game_id: sess.canonical_game_id.clone(),
                pack_id: sess
                    .matched_pack_id
                    .clone()
                    .unwrap_or_else(|| sess.producer_id.clone()),
                title: sess.canonical_game_id.clone(),
                mapped_oap_edition: None,
                unlocked_count: 0,
                total_achievements: 0,
                completion_percent: 0,
                active: true,
                edition_or_pack: sess
                    .matched_pack_id
                    .clone()
                    .unwrap_or_else(|| sess.producer_id.clone()),
            });
        }
        if let Some(lg) = snap.launcher_games.iter().find(|g| g.active) {
            return Some(LibraryGameProgress {
                canonical_game_id: lg.canonical_game_id.clone(),
                pack_id: lg
                    .pack_id
                    .clone()
                    .unwrap_or_else(|| lg.launcher.clone()),
                title: lg
                    .game_title
                    .clone()
                    .unwrap_or_else(|| lg.canonical_game_id.clone()),
                mapped_oap_edition: None,
                unlocked_count: lg.unlocked_count,
                total_achievements: lg.total_achievements,
                completion_percent: lg.completion_percent,
                active: true,
                edition_or_pack: lg
                    .pack_id
                    .clone()
                    .unwrap_or_else(|| lg.launcher.clone()),
            });
        }
        None
    }

    fn toggle_pause(&mut self) {
        let next_paused = !self
            .snapshot
            .as_ref()
            .map(|s| s.tracking_paused)
            .unwrap_or(self.tray_model.paused);
        let res = self
            .backend
            .as_ref()
            .map(|b| b.set_tracking_paused(next_paused));
        match res {
            Some(Ok(actual)) => {
                self.set_banner(
                    if actual {
                        "Tracking paused"
                    } else {
                        "Tracking resumed"
                    },
                    false,
                );
                self.refresh_snapshot();
            }
            Some(Err(e)) => self.set_banner(e, true),
            None => {}
        }
    }

    fn toggle_launch_at_login(&mut self, target: Option<bool>) {
        let desired = target.unwrap_or(!self.launch_at_login);
        let res = self.backend.as_ref().map(|b| b.set_launch_at_login(desired));
        match res {
            Some(Ok(actual)) => {
                self.launch_at_login = actual;
                self.set_banner(
                    if actual {
                        "OneHundo will launch automatically at login"
                    } else {
                        "Launch at login disabled"
                    },
                    false,
                );
                self.refresh_snapshot();
            }
            Some(Err(e)) => self.set_banner(e, true),
            None => {}
        }
    }

    fn process_tray_and_async(&mut self, ctx: &egui::Context) {
        *self.tray_ctx_holder.lock().unwrap() = Some(ctx.clone());

        while let Ok(cmd) = self.tray_rx.try_recv() {
            match cmd {
                TrayCommand::OpenWindow => {
                    self.tray_model.handle_open_requested();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayCommand::TogglePause => {
                    self.toggle_pause();
                }
                TrayCommand::ToggleLaunchAtLogin => {
                    self.toggle_launch_at_login(None);
                }
                TrayCommand::Quit => {
                    self.tray_model.handle_quit_requested();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        // Intercept close button unless user selected Quit in tray or menu
        if ctx.input(|i| i.viewport().close_requested()) {
            if !self.tray_model.handle_window_close_requested() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                if let Some(ref handle) = self.tray_handle {
                    handle.update(&self.tray_model);
                }
            }
        }

        // Poll native folder picker result
        if let Some(ref rx) = self.folder_picker_rx {
            if let Ok(picked) = rx.try_recv() {
                self.folder_picker_rx = None;
                if let Some(path) = picked {
                    let game_id = if self.selected_game_for_folder.trim().is_empty() {
                        None
                    } else {
                        Some(self.selected_game_for_folder.trim().to_string())
                    };
                    if let Some(ref backend) = self.backend {
                        match backend.add_game_folder(game_id, &path) {
                            Ok(()) => {
                                self.set_banner(
                                    format!("Added game folder {}", path.display()),
                                    false,
                                );
                                self.refresh_snapshot();
                            }
                            Err(err) => self.set_banner(err, true),
                        }
                    }
                }
            }
        }

        // Poll browser OAuth loopback result
        if let Some(ref auth) = self.pending_auth {
            if let Ok(result) = auth.rx.try_recv() {
                self.pending_auth = None;
                match result {
                    Ok(creds) => {
                        let res = self
                            .backend
                            .as_ref()
                            .map(|b| b.save_account(creds.api_key, creds.username.clone()));
                        match res {
                            Some(Ok(())) => {
                                let label = creds
                                    .username
                                    .map(|u| format!("Signed in as @{}", u))
                                    .unwrap_or_else(|| "Signed in to OneHundo.org".to_string());
                                self.set_banner(label, false);
                                self.refresh_snapshot();
                            }
                            Some(Err(err)) => self.set_banner(err, true),
                            None => {}
                        }
                    }
                    Err(err) => self.set_banner(err, true),
                }
            }
        }

        if let Some((_, ts)) = &self.banner {
            if ts.elapsed() > Duration::from_secs(7) {
                self.banner = None;
            }
        }
    }

    fn render_header(&mut self, ui: &mut egui::Ui) {
        // Register the whole title-bar surface first. Child controls below keep their own
        // hit targets, while empty header space remains a reliable window drag handle.
        let drag_rect = egui::Rect::from_min_size(
            ui.cursor().min,
            Vec2::new(ui.available_width(), 52.0),
        );
        let drag = ui.interact(
            drag_rect,
            ui.scope_id().with("titlebar_drag"),
            egui::Sense::click_and_drag(),
        );
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, LINE))
            .inner_margin(Margin::symmetric(18, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    wordmark_square(ui);
                    ui.add_space(18.0);

                    for (tab, label) in [
                        (Tab::Status, "Status"),
                        (Tab::Library, "Library"),
                    ] {
                        let active = self.tab == tab;
                        let resp = header_menu_button(ui, label, active, 66.0, 32.0);
                        if active {
                            ui.painter().line_segment(
                                [resp.rect.left_bottom(), resp.rect.right_bottom()],
                                Stroke::new(2.0, INK),
                            );
                        }
                        if resp.clicked() {
                            self.tab = tab;
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        // The window buttons live in the custom title bar. Account stays the
                        // right-most navigation item, matching the platform header.
                        let close = header_window_button(ui, "×", 19.0);
                        if close.clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        let maximize = header_window_button(ui, "□", 14.0);
                        if maximize.clicked() {
                            let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                        }
                        let minimize = header_window_button(ui, "−", 17.0);
                        if minimize.clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }

                        let username = self.snapshot.as_ref()
                            .and_then(|s| s.account_username.as_deref());
                        let account_label = username.map(|name| truncate_chars(name, 10)).unwrap_or_else(|| "Account".to_string());
                        let account = header_menu_button(ui, &account_label, self.tab == Tab::Account, 92.0, 32.0);
                        if account.clicked() {
                            self.tab = Tab::Account;
                        }

                    });
                });
            });
    }

    fn render_banner(&mut self, ui: &mut egui::Ui) {
        let Some((banner, _)) = self.banner.clone() else {
            return;
        };
        let accent = if banner.is_error { ERROR } else { GILT };
        egui::Frame::new()
            .fill(SURFACE_2)
            .stroke(Stroke::new(1.0, accent))
            .corner_radius(CornerRadius::ZERO)
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&banner.message).size(13.0).color(INK));
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if subtle_button(ui, "Dismiss").clicked() {
                            self.banner = None;
                        }
                    });
                });
            });
        ui.add_space(12.0);
    }

    fn render_status_tab(&mut self, ui: &mut egui::Ui) {
        let snap = self.snapshot.clone();
        let paused = snap.as_ref().map(|s| s.tracking_paused).unwrap_or(false);
        let active_game = snap.as_ref().and_then(Self::active_game_from_snapshot);
        let is_actively_tracking = !paused && active_game.is_some();

        // Hero status card
        let hero_accent = if paused {
            Some(WARNING)
        } else if is_actively_tracking {
            Some(TEAL)
        } else {
            Some(LINE_STRONG)
        };

        grid_card(ui, hero_accent, |ui| {
            ui.set_width(ui.available_width());
            if paused {
                ui.label(
                    RichText::new("Tracking paused")
                        .font(display_font(24.0))
                        .color(INK),
                );
                ui.add_space(4.0);
                muted_label(ui, "Resume tracking to detect games and saves.");
            } else if let Some(ref game) = active_game {
                ui.horizontal(|ui| {
                    let (dot, _) =
                        ui.allocate_exact_size(Vec2::splat(7.0), egui::Sense::hover());
                    ui.painter().rect_filled(dot, CornerRadius::ZERO, TEAL);
                    ui.label(
                        RichText::new("Tracking")
                            .font(semibold_font(13.0))
                            .color(INK),
                    );
                });
                ui.add_space(6.0);
                ui.label(RichText::new(&game.title).font(display_font(24.0)).color(INK));
                ui.add_space(12.0);

                ui.add_space(14.0);

                ui.horizontal(|ui| {
                    let count_text = if game.total_achievements > 0 {
                        format!(
                            "{} / {} achievements · {}%",
                            game.unlocked_count, game.total_achievements, game.completion_percent
                        )
                    } else {
                        format!("{} unlocked", game.unlocked_count)
                    };
                    ui.label(
                        RichText::new(count_text)
                            .font(semibold_font(14.0))
                            .color(INK),
                    );
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let sync_note = match snap.as_ref() {
                            Some(s) if s.account_signed_in && s.sync_queue_depth > 0 => {
                                format!("{} queued to sync", s.sync_queue_depth)
                            }
                            Some(s) if s.account_signed_in => "Auto-sync active".to_string(),
                            _ => "Saved locally · Sign in to sync".to_string(),
                        };
                        ui.label(RichText::new(sync_note).size(12.0).color(INK_2));
                    });
                });
                ui.add_space(8.0);
                progress_bar(ui, game.unlocked_count, game.total_achievements.max(1), 8.0);
            } else {
                ui.label(
                    RichText::new("No game detected")
                        .font(display_font(24.0))
                        .color(INK),
                );
                ui.add_space(4.0);
                muted_label(ui, "Waiting for a supported game.");
            }
        });

        ui.add_space(14.0);

        // Quick controls bar
        grid_card(ui, None, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                if secondary_button(ui, "Send Test Notification").clicked() {
                    if let Some(ref backend) = self.backend {
                        match backend.send_test_notification() {
                            Ok(msg) => self.set_banner(msg, false),
                            Err(err) => self.set_banner(err, true),
                        }
                    }
                }
                let pause_label = if paused {
                    "Resume Tracking"
                } else {
                    "Pause Tracking"
                };
                if secondary_button(ui, pause_label).clicked() {
                    self.toggle_pause();
                }

                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    if let Some(ref s) = snap {
                        if s.account_signed_in {
                            let sync_label = if s.sync_queue_depth > 0 {
                                format!("Sync Now ({})", s.sync_queue_depth)
                            } else {
                                "Sync Now".to_string()
                            };
                            if subtle_button(ui, &sync_label).clicked() {
                                if let Some(ref backend) = self.backend {
                                    match backend.sync_now() {
                                        Ok(msg) => {
                                            self.set_banner(msg, false);
                                            self.refresh_snapshot();
                                        }
                                        Err(err) => self.set_banner(err, true),
                                    }
                                }
                            }
                        } else if subtle_button(ui, "Sign In to Sync").clicked() {
                            self.tab = Tab::Account;
                        }
                    }
                });
            });
        });

        ui.add_space(14.0);

        // Recent Unlocks
        grid_card(ui, None, |ui| {
            ui.set_width(ui.available_width());
            section_heading(ui, "Recent Unlocks");
            ui.add_space(10.0);

            let recent = snap
                .as_ref()
                .map(|s| s.recent_trophies.as_slice())
                .unwrap_or(&[]);
            if recent.is_empty() {
                empty_state(
                    ui,
                    "No recent unlocks yet",
                    "Unlocks will appear here.",
                );
            } else {
                for (idx, item) in recent.iter().take(3).enumerate() {
                    if idx > 0 {
                        ui.separator();
                    }
                    let row_width = ui.available_width();
                    ui.allocate_ui_with_layout(
                        Vec2::new(row_width, 44.0),
                        egui::Layout::left_to_right(Align::Center),
                        |row| {
                            let text_width = (row_width - 96.0).max(80.0);
                            row.allocate_ui_with_layout(
                                Vec2::new(text_width, 42.0),
                                egui::Layout::top_down(Align::Min),
                                |text| {
                                    text.add_sized(
                                        Vec2::new(text_width, 21.0),
                                        egui::Label::new(
                                            RichText::new(&item.achievement_title)
                                                .font(semibold_font(13.0))
                                                .color(INK),
                                        ).truncate(),
                                    );
                                    text.add_sized(
                                        Vec2::new(text_width, 18.0),
                                        egui::Label::new(
                                            RichText::new(&item.game_title).size(12.0).color(INK_2),
                                        ).truncate(),
                                    );
                                },
                            );
                            row.with_layout(egui::Layout::right_to_left(Align::Center), |date| {
                                let when = item
                                    .unlocked_at
                                    .as_deref()
                                    .map(|s| s.split('T').next().unwrap_or(s))
                                    .unwrap_or("Local");
                                date.add_sized(
                                    Vec2::new(84.0, 18.0),
                                    egui::Label::new(RichText::new(when).size(11.5).color(INK_3))
                                        .truncate(),
                                );
                            });
                        },
                    );
                }
            }
        });
    }

    fn render_library_tab(&mut self, ui: &mut egui::Ui) {
        let snap = self.snapshot.clone();

        // Custom folders card
        grid_card(ui, None, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                section_heading(ui, "Game Folders");
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let picking = self.folder_picker_rx.is_some();
                    ui.add_enabled_ui(!picking, |ui| {
                        if primary_button(ui, "+ Add Game Folder").clicked() {
                            let (tx, rx) = mpsc::channel();
                            self.folder_picker_rx = Some(rx);
                            let ctx = ui.ctx().clone();
                            std::thread::spawn(move || {
                                let picked = rfd::FileDialog::new()
                                    .set_title("Select Game or Save Folder")
                                    .pick_folder();
                                let _ = tx.send(picked);
                                ctx.request_repaint();
                            });
                        }
                    });
                });
            });
            ui.add_space(6.0);
            muted_label(
                ui,
                "Running games are detected automatically. Add custom install or save folders below for non-standard locations.",
            );
            ui.add_space(10.0);

            if let Some(ref s) = snap {
                let selected_label = if self.selected_game_for_folder.is_empty() {
                    "Auto-detect game".to_string()
                } else {
                    s.library_games
                        .iter()
                        .find(|g| g.canonical_game_id == self.selected_game_for_folder)
                        .map(|g| g.title.clone())
                        .unwrap_or_else(|| self.selected_game_for_folder.clone())
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Bind new folder to:").size(12.5).color(INK_2));
                    egui::ComboBox::from_id_salt("folder_game_bind")
                        .selected_text(selected_label)
                        .width(280.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.selected_game_for_folder,
                                String::new(),
                                "Auto-detect game",
                            );
                            for g in &s.library_games {
                                ui.selectable_value(
                                    &mut self.selected_game_for_folder,
                                    g.canonical_game_id.clone(),
                                    &g.title,
                                );
                            }
                        });
                });
                ui.add_space(10.0);

                if s.watched_dirs.is_empty() {
                    muted_label(ui, "No custom folders configured yet.");
                } else {
                    let mut remove_target: Option<PathBuf> = None;
                    for (idx, w) in s.watched_dirs.iter().enumerate() {
                        if idx > 0 {
                            ui.separator();
                        }
                        let game_name = w
                            .canonical_game_id
                            .as_deref()
                            .and_then(|gid| {
                                s.library_games
                                    .iter()
                                    .find(|g| g.canonical_game_id == gid)
                                    .map(|g| g.title.as_str())
                            })
                            .or(w.canonical_game_id.as_deref())
                            .unwrap_or("Auto-detect");
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(w.path.display().to_string())
                                        .font(semibold_font(13.0))
                                        .color(INK),
                                );
                                ui.label(RichText::new(game_name).size(12.0).color(INK_2));
                            });
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                if subtle_button(ui, "Remove").clicked() {
                                    remove_target = Some(w.path.clone());
                                }
                            });
                        });
                    }
                    if let Some(target) = remove_target {
                        if let Some(ref backend) = self.backend {
                            match backend.remove_game_folder(&target) {
                                Ok(_) => {
                                    self.set_banner(
                                        format!("Removed folder {}", target.display()),
                                        false,
                                    );
                                    self.refresh_snapshot();
                                }
                                Err(err) => self.set_banner(err, true),
                            }
                        }
                    }
                }
            }
        });

        ui.add_space(14.0);

        // Supported games & library list
        grid_card(ui, None, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                section_heading(ui, "Supported Games");
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.library_filter)
                            .hint_text("Filter games…")
                            .desired_width(210.0),
                    );
                });
            });
            ui.add_space(12.0);

            if let Some(ref s) = snap {
                let needle = self.library_filter.trim().to_ascii_lowercase();
                let games: Vec<&LibraryGameProgress> = s
                    .library_games
                    .iter()
                    .filter(|g| {
                        needle.is_empty()
                            || g.title.to_ascii_lowercase().contains(&needle)
                            || g.canonical_game_id.to_ascii_lowercase().contains(&needle)
                    })
                    .collect();

                if games.is_empty() {
                    empty_state(
                        ui,
                        "No matching games",
                        "Try clearing the filter or placing a .onehundo pack in your packs folder.",
                    );
                } else {
                    for (idx, g) in games.iter().enumerate() {
                        if idx > 0 {
                            ui.add_space(6.0);
                            ui.separator();
                            ui.add_space(6.0);
                        }
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    if g.active {
                                        let (dot, _) = ui.allocate_exact_size(
                                            Vec2::splat(7.0),
                                            egui::Sense::hover(),
                                        );
                                        ui.painter().rect_filled(dot, CornerRadius::ZERO, TEAL);
                                    }
                                    ui.label(
                                        RichText::new(&g.title)
                                            .font(semibold_font(14.0))
                                            .color(INK),
                                    );
                                    if g.active {
                                        ui.label(
                                            RichText::new("TRACKING")
                                                .font(semibold_font(10.5))
                                                .color(TEAL)
                                                .extra_letter_spacing(0.8),
                                        );
                                    }
                                });
                                ui.label(
                                    RichText::new(&g.edition_or_pack)
                                        .size(12.0)
                                        .color(INK_2),
                                );
                            });
                            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "{} / {} · {}%",
                                        g.unlocked_count, g.total_achievements, g.completion_percent
                                    ))
                                    .font(semibold_font(12.5))
                                    .color(if g.unlocked_count > 0 { INK } else { INK_2 }),
                                );
                            });
                        });
                        ui.add_space(4.0);
                        progress_bar(ui, g.unlocked_count, g.total_achievements.max(1), 4.0);
                    }
                }
            }
        });
    }

    fn render_account_tab(&mut self, ui: &mut egui::Ui) {
        let snap = self.snapshot.clone();
        let signed_in = snap.as_ref().map(|s| s.account_signed_in).unwrap_or(false);
        let username = snap
            .as_ref()
            .and_then(|s| s.account_username.clone());
        let device_registered = snap
            .as_ref()
            .map(|s| s.sync.device_registered)
            .unwrap_or(false);
        let last_sync_err = snap
            .as_ref()
            .and_then(|s| s.sync.last_sync_error.clone());
        let active_mode = snap
            .as_ref()
            .map(|s| s.active_progression_mode)
            .unwrap_or(ProgressionMode::Standard);

        grid_card(ui, if signed_in { Some(TEAL) } else { None }, |ui| {
            ui.set_width(ui.available_width());
            section_heading(ui, "OneHundo Account");
            ui.add_space(8.0);

            if let Some(ref auth) = self.pending_auth {
                let auth_url = auth.auth_url.clone();
                let fallback_url = auth.fallback_url.clone();
                let cancel_flag = std::sync::Arc::clone(&auth.cancel_flag);
                let mut cancel_requested = false;
                muted_label(
                    ui,
                    "Complete sign-in in your browser tab to link this computer to OneHundo.org.",
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if secondary_button(ui, "Reopen Browser").clicked() {
                        let _ = open_in_browser(&auth_url);
                    }
                    if subtle_button(ui, "Open Account Keys").clicked() {
                        let _ = open_in_browser(&fallback_url);
                    }
                    if subtle_button(ui, "Cancel").clicked() {
                        cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        cancel_requested = true;
                    }
                });
                if cancel_requested {
                    self.pending_auth = None;
                }
            } else if signed_in {
                ui.horizontal(|ui| {
                    let display_name = username
                        .as_deref()
                        .map(|u| format!("@{}", u))
                        .unwrap_or_else(|| "Connected Account".to_string());
                    ui.label(
                        RichText::new(display_name)
                            .font(display_font(20.0))
                            .color(INK),
                    );
                    ui.add_space(10.0);
                    status_badge(
                        ui,
                        if device_registered {
                            "Synced"
                        } else {
                            "Connected"
                        },
                        TEAL,
                    );
                });
                ui.add_space(6.0);
                if let Some(err) = last_sync_err {
                    ui.label(RichText::new(err).size(12.5).color(WARNING));
                    ui.add_space(6.0);
                } else {
                    muted_label(
                        ui,
                        "Achievements and completions sync automatically to OneHundo.org.",
                    );
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if secondary_button(ui, "Sync Now").clicked() {
                        if let Some(ref backend) = self.backend {
                            match backend.sync_now() {
                                Ok(msg) => {
                                    self.set_banner(msg, false);
                                    self.refresh_snapshot();
                                }
                                Err(err) => self.set_banner(err, true),
                            }
                        }
                    }
                    if subtle_button(ui, "Sign Out").clicked() {
                        if let Some(ref backend) = self.backend {
                            match backend.sign_out() {
                                Ok(()) => {
                                    self.set_banner("Signed out of OneHundo.org", false);
                                    self.refresh_snapshot();
                                }
                                Err(err) => self.set_banner(err, true),
                            }
                        }
                    }
                });
            } else {
                muted_label(
                    ui,
                    "Sign in once to sync your achievements across devices and your public profile.",
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if primary_button(ui, "Sign in with OneHundo.org").clicked() {
                        if let Some(ref backend) = self.backend {
                            let url = backend.platform_url(snap.as_ref());
                            let dev = backend.device_name().to_string();
                            match start_browser_auth(&url, &dev) {
                                Ok(pending) => {
                                    self.pending_auth = Some(pending);
                                }
                                Err(err) => self.set_banner(err, true),
                            }
                        }
                    }
                    let toggle_lbl = if self.show_manual_key {
                        "Hide API Key"
                    } else {
                        "Use API key instead"
                    };
                    if subtle_button(ui, toggle_lbl).clicked() {
                        self.show_manual_key = !self.show_manual_key;
                    }
                });

                if self.show_manual_key {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.manual_api_key)
                                .password(true)
                                .hint_text("oh_live_...")
                                .desired_width(320.0),
                        );
                        if secondary_button(ui, "Save Key").clicked() {
                            let key = self.manual_api_key.trim().to_string();
                            if let Some(ref backend) = self.backend {
                                match backend.save_account(key, None) {
                                    Ok(()) => {
                                        self.manual_api_key.clear();
                                        self.show_manual_key = false;
                                        self.set_banner("Account API key saved", false);
                                        self.refresh_snapshot();
                                    }
                                    Err(err) => self.set_banner(err, true),
                                }
                            }
                        }
                    });
                }
            }
        });

        ui.add_space(14.0);

        // Launch at login & mode card
        grid_card(ui, None, |ui| {
            ui.set_width(ui.available_width());
            section_heading(ui, "Preferences");
            ui.add_space(12.0);

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Launch at Login")
                            .font(semibold_font(14.0))
                            .color(INK),
                    );
                    muted_label(
                        ui,
                        "Start OneHundo silently in the system tray when you log in.",
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let mut val = self.launch_at_login;
                    if toggle_switch(ui, &mut val).changed() {
                        self.toggle_launch_at_login(Some(val));
                    }
                });
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(12.0);

            ui.horizontal_wrapped(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Progression Mode")
                            .font(semibold_font(14.0))
                            .color(INK),
                    );
                    let mode_desc = match active_mode {
                        ProgressionMode::Standard => "Standard — Normal persistent unlocks.",
                        ProgressionMode::Encore => {
                            "Encore — Record replay runs while keeping first unlocks."
                        }
                        ProgressionMode::Spectator => {
                            "Spectator — Notifications only; never alters progress."
                        }
                    };
                    muted_label(ui, mode_desc);
                });
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    for mode in [
                        ProgressionMode::Spectator,
                        ProgressionMode::Encore,
                        ProgressionMode::Standard,
                    ] {
                        let label = match mode {
                            ProgressionMode::Standard => "Standard",
                            ProgressionMode::Encore => "Encore",
                            ProgressionMode::Spectator => "Spectator",
                        };
                        let selected = active_mode == mode;
                        let resp = if selected {
                            primary_button(ui, label)
                        } else {
                            subtle_button(ui, label)
                        };
                        if resp.clicked() && !selected {
                            if let Some(ref backend) = self.backend {
                                match backend.set_progression_mode(mode) {
                                    Ok(()) => {
                                        self.set_banner(
                                            format!("Switched to {} mode", label),
                                            false,
                                        );
                                        self.refresh_snapshot();
                                    }
                                    Err(err) => self.set_banner(err, true),
                                }
                            }
                        }
                    }
                });
            });
        });
    }
}

fn header_menu_button(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    width: f32,
    height: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::click());
    let fill = if selected || response.hovered() { SURFACE_2 } else { SURFACE };
    ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        semibold_font(13.0),
        if selected { INK } else { INK_2 },
    );
    response
}

fn header_window_button(ui: &mut egui::Ui, label: &str, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(34.0, 32.0), egui::Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, SURFACE_2);
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(size),
        if response.hovered() { INK } else { INK_2 },
    );
    response
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let collected: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{}…", collected.trim_end())
    } else {
        collected
    }
}

impl eframe::App for OneHundoApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.process_tray_and_async(ctx);

        if self.last_refresh.elapsed() >= Duration::from_secs(2) {
            self.refresh_snapshot();
        }
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("onehundo_top_bar")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                self.render_header(ui);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(PAPER)
                    .inner_margin(Margin::same(24)),
            )
            .show(ui, |ui| {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    self.render_banner(ui);
                    if let Some(ref err) = self.snapshot_error {
                        egui::Frame::new()
                            .fill(SURFACE_2)
                            .stroke(Stroke::new(1.0, ERROR))
                            .inner_margin(Margin::same(12))
                            .show(ui, |ui| {
                                ui.label(RichText::new(err).size(13.0).color(INK));
                            });
                        ui.add_space(12.0);
                    }

                    match self.tab {
                        Tab::Status => self.render_status_tab(ui),
                        Tab::Library => self.render_library_tab(ui),
                        Tab::Account => self.render_account_tab(ui),
                    }
                });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.tray_stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.tray_thread.take() {
            let _ = t.join();
        }
        if let Some(backend) = self.backend.take() {
            backend.shutdown();
        }
    }
}
