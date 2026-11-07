//! Timewarp desktop shell (gpui-kit).
//!
//! The UI is a pure observer of the engine thread. It never draws over the
//! game; while a match is running you never need to look at it at all.

// gpui's derive macros refer to `gpui::…`; gpui-kit re-exports it.
extern crate gpui_kit as gpui;

#[cfg(target_os = "macos")]
mod clip;
#[cfg(target_os = "macos")]
mod clips;
#[cfg(target_os = "macos")]
mod condense;
#[cfg(target_os = "macos")]
mod merge;
mod filters;
mod games;
mod home;
#[cfg(target_os = "macos")]
mod hotkey;
mod icons;
mod about;
mod deleted;
mod updates;
mod draw;
mod listnav;
mod marquee;
mod matchups;
mod moments;
mod palette;
mod permissions;
mod setup;
#[cfg(target_os = "macos")]
mod player;
mod review;
mod settings;
mod store;
mod theme;
mod transfer;
mod timeline;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use timewarp_engine::{Command, Config, EngineHandle, Phase, SessionMeta, Status};

use crate::filters::{GameFilter, Side};
use crate::icons::Icons;
use crate::review::{Matchup, Review, ReviewEvent};
use crate::store::Store;
use crate::ui::{MomentFilter, icon};


pub const SIDEBAR_WIDTH: f32 = 232.;

gpui_kit::actions!(timewarp, [Quit, OpenSettings, HideApp, Minimize, GoHome, GoGames, GoMatchups, GoClips, GoBack, GoForward, OpenPalette, OpenAbout, CheckForUpdates]);

/// One place in the app, for Back/Forward (mouse buttons, ⌘[ ⌘]).
#[derive(Clone, PartialEq)]
pub struct View {
    pub page: Page,
    pub review: Option<PathBuf>,
    pub clip: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Setup,
    Home,
    Games,
    Moments,
    Matchups,
    Clips,
    Deleted,
    Settings,
    About,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResultFilter {
    All,
    Wins,
    Losses,
}

pub struct Game {
    pub path: PathBuf,
    pub meta: SessionMeta,
    pub bytes: u64,
}

/// Background "moments only" work, one game at a time.
#[derive(Default)]
pub struct Maintenance {
    pub queue: std::collections::VecDeque<PathBuf>,
    /// Split recordings of one match waiting to be joined (oldest first).
    pub joins: std::collections::VecDeque<Vec<PathBuf>>,
    pub joining: bool,
    /// Condenses the user asked for: the original goes to the Trash.
    pub manual: std::collections::HashSet<PathBuf>,
    /// Joined files already checked for the multi-track problem.
    pub track_checked: std::collections::HashSet<PathBuf>,
    /// Pieces that failed to join; not retried this session.
    pub join_failed: std::collections::HashSet<PathBuf>,
    pub active: Option<PathBuf>,
    pub done: usize,
    pub total: usize,
    pub saved_bytes: u64,
    pub last_error: Option<String>,
}

/// Back-to-back playback of moments, possibly across games.
pub struct Playlist {
    pub title: String,
    /// (game index, moment video time)
    pub items: Vec<(usize, f64)>,
    pub idx: usize,
}

pub struct TimewarpApp {
    pub engine: EngineHandle,
    pub config: Config,
    pub status: Status,
    pub games: Vec<Game>,
    pub page: Page,
    /// Shared by Games, Moments and Matchups.
    pub filter: GameFilter,
    /// Slot being browsed in the "filter by champion" picker.
    pub condition_draft: (Side, &'static str),
    pub moment_filter: MomentFilter,
    pub store: Entity<Store>,
    pub selected_matchup: Option<String>,
    /// Matchups page: notes open in the editor (otherwise shown rendered).
    pub matchup_note_editing: bool,
    pub matchup_editor: Option<(String, Entity<gpui_kit::component::input::TextareaState>, Subscription)>,
    pub review: Option<Entity<Review>>,
    /// Settings → Backup and transfer.
    pub transfer_parts: transfer::Parts,
    pub export_filtered_only: bool,
    pub pending_import: Option<(PathBuf, transfer::Manifest, transfer::Parts)>,
    pub transfer_busy: bool,
    /// Games page multi-select, and the row ⇧-click ranges start from.
    pub selected: std::collections::BTreeSet<PathBuf>,
    pub select_anchor: Option<usize>,
    pub selected_clips: std::collections::BTreeSet<PathBuf>,
    pub clip_anchor: Option<usize>,
    /// Rubber-band selection in progress, and the on-screen bounds of the
    /// rows/cards it tests against (refreshed every frame).
    pub marquee: Option<marquee::Marquee>,
    pub item_bounds: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<PathBuf, Bounds<Pixels>>>>,
    /// Visible area of the list/grid being shown (the rubber band stays inside).
    pub list_area: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// Set when a press turned into a drag, so the release doesn't open anything.
    pub suppress_click: bool,
    /// Keyboard focus row/card in Games and Clips (separate from selection),
    /// and their scroll positions so it stays in view.
    pub list_cursor: Option<PathBuf>,
    /// Recently deleted: a pending "Delete forever" confirmation.
    pub deleted_confirm: Option<deleted::Confirm>,
    pub about_tab: about::AboutTab,
    /// Acknowledgements: the library whose licence text is expanded.
    pub about_open: Option<usize>,
    pub games_scroll: ScrollHandle,
    pub clips_scroll: ScrollHandle,
    /// ⌘K command palette, and the `?` shortcut sheet.
    pub palette: Option<palette::Palette>,
    pub show_shortcuts: bool,
    /// Setup checks (permissions, folder, disk), refreshed every few seconds.
    pub checks: Vec<permissions::Check>,
    checks_at: std::time::Instant,
    /// Updates: shared with the download thread; when we last checked, the
    /// change we last showed, and when the app started (first check waits).
    pub updater: updates::Shared,
    pub update_checked: Option<std::time::Instant>,
    pub update_seen: u64,
    pub started_at: std::time::Instant,
    /// Keyboard focus for page-level shortcuts (⌘A/Ctrl+A, Esc, Delete).
    pub focus: FocusHandle,
    /// Dev screenshot aid (TIMEWARP_PAGE=games-marquee).
    pub dev_marquee_to: Option<Point<Pixels>>,
    /// Back/Forward history.
    history: Vec<View>,
    future: Vec<View>,
    /// Index into `games` of the open review.
    pub review_game: Option<usize>,
    pub playlist: Option<Playlist>,
    pub maintenance: Maintenance,
    pub clips: Vec<clip::Clip>,
    pub toasts: Vec<ui::Toast>,
    next_toast: u64,
    pub clip_view: Option<Entity<clips::ClipViewer>>,
    thumbs_requested: std::collections::HashSet<PathBuf>,
    /// Games waiting for the user to confirm "keep moments only".
    pub confirm_condense: Option<Vec<PathBuf>>,
    pub icons: Arc<Icons>,
    _poll: Task<()>,
    _quit: Subscription,
}

impl TimewarpApp {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut config = Config::load();
        if std::env::var_os("TIMEWARP_PAGE").is_some() {
            // screenshot/dev runs never start recordings on their own
            config.auto_record = false;
        }
        if let Some(dir) = std::env::var_os("TIMEWARP_DIR") {
            // dev: browse another library without touching saved settings
            config.output_dir = dir.into();
        }
        let engine = timewarp_engine::spawn(config.clone());

        // Poll the engine's status snapshot twice a second. Cheap, and keeps the
        // engine free of any UI callbacks.
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                if this.update(cx, |app, cx| app.refresh(cx)).is_err() {
                    break;
                }
            }
        });

        let store = cx.new(|_| Store::load());
        cx.observe(&store, |_, _, cx| cx.notify()).detach();

        // A kill (SIGTERM) or Ctrl-C must not cut a recording off: finish the
        // file the same way ⌘Q does, then exit.
        #[cfg(unix)]
        {
            let engine = engine.clone();
            std::thread::Builder::new()
                .name("signals".into())
                .spawn(move || {
                    use signal_hook::consts::{SIGINT, SIGTERM};
                    if let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGTERM, SIGINT]) {
                        if signals.forever().next().is_some() {
                            log::info!("stopping: finishing any recording first");
                            engine.shutdown();
                            std::process::exit(0);
                        }
                    }
                })
                .ok();
        }

        // Finalize an in-progress recording and flush notes before the process exits.
        let quit = cx.on_app_quit(|this: &mut Self, cx| {
            this.engine.shutdown();
            this.store.update(cx, |s, _| s.flush());
            async {}
        });

        let mut app = Self {
            engine,
            games: vec![],
            config,
            status: Status::default(),
            page: Page::Home,
            filter: GameFilter::default(),
            condition_draft: (Side::Enemy, "TOP"),
            moment_filter: MomentFilter::All,
            store,
            selected_matchup: None,
            matchup_note_editing: false,
            matchup_editor: None,
            review: None,
            transfer_parts: transfer::Parts::default(),
            export_filtered_only: false,
            pending_import: None,
            transfer_busy: false,
            selected: Default::default(),
            select_anchor: None,
            selected_clips: Default::default(),
            clip_anchor: None,
            marquee: None,
            item_bounds: Default::default(),
            list_area: Default::default(),
            suppress_click: false,
            dev_marquee_to: None,
            focus: cx.focus_handle(),
            checks: vec![],
            palette: None,
            list_cursor: None,
            deleted_confirm: None,
            about_tab: Default::default(),
            about_open: None,
            games_scroll: ScrollHandle::new(),
            clips_scroll: ScrollHandle::new(),
            show_shortcuts: false,
            checks_at: std::time::Instant::now(),
            updater: Default::default(),
            update_checked: None,
            update_seen: 0,
            started_at: std::time::Instant::now(),
            history: vec![],
            future: vec![],
            review_game: None,
            playlist: None,
            maintenance: Maintenance::default(),
            clips: vec![],
            toasts: vec![],
            next_toast: 0,
            clip_view: None,
            thumbs_requested: Default::default(),
            confirm_condense: None,
            icons: Icons::new(),
            _poll: poll,
            _quit: quit,
        };
        app.reload_games();
        app.reload_clips();
        app.checks = permissions::checks(&app.config);
        // Something required is missing: start on the Setup screen.
        if app.checks.iter().any(|c| c.blocking()) {
            app.page = Page::Setup;
        }
        app.apply_hotkey();
        app.run_maintenance(cx);
        app
    }

    /// Ranges a game would keep if condensed now (empty = nothing worth keeping).
    #[cfg(target_os = "macos")]
    pub fn condense_plan(&self, game: &Game, cx: &App) -> Vec<(f64, f64)> {
        condense::plan(&game.meta, self.store.read(cx).game(game), &self.config.storage.keep)
    }

    /// Whether a game can be condensed at all right now.
    fn condensable(&self, game: &Game, cx: &App) -> bool {
        game.meta.condensed.is_none()
            && self.status.current_file.as_deref() != Some(game.path.as_path())
            && !self.store.read(cx).game(game).is_some_and(|n| n.pinned)
    }

    /// Games the policy wants condensed now: old enough, not kept, and with
    /// at least one moment worth keeping.
    pub fn condense_candidates(&self, cx: &App) -> Vec<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            let Some(days) = self.config.storage.condense_after_days else { return vec![] };
            let now = chrono::Local::now();
            self.games
                .iter()
                .filter(|g| (now - g.meta.started_at).num_hours() >= i64::from(days) * 24)
                .filter(|g| self.condensable(g, cx) && !self.condense_plan(g, cx).is_empty())
                .map(|g| g.path.clone())
                .collect()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = cx;
            vec![]
        }
    }

    /// Bytes a set of games would free if condensed, estimated from kept length.
    pub fn condense_savings(&self, paths: &[PathBuf], cx: &App) -> u64 {
        #[cfg(target_os = "macos")]
        {
            paths
                .iter()
                .filter_map(|p| self.games.iter().find(|g| &g.path == p))
                .map(|g| {
                    let kept: f64 = self.condense_plan(g, cx).iter().map(|(a, b)| b - a).sum();
                    let frac = (kept / g.meta.duration_secs.max(1.0)).clamp(0.0, 1.0);
                    (g.bytes as f64 * (1.0 - frac)) as u64
                })
                .sum()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (paths, cx);
            0
        }
    }

    /// Queue games for "moments only". Games being recorded, kept or already
    /// condensed are skipped.
    /// Manual "keep moments only": like `queue_condense`, but the full
    /// videos go to the Trash so it can be undone.
    pub fn queue_condense_manual(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.maintenance.manual.extend(paths.iter().cloned());
        self.queue_condense(paths, cx);
    }

    pub fn queue_condense(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let m = &mut self.maintenance;
        for p in paths {
            if !m.queue.contains(&p) && m.active.as_ref() != Some(&p) {
                m.queue.push_back(p);
                m.total += 1;
            }
        }
        self.process_condense(cx);
    }

    /// Join the next group of split recordings, if any. Returns true when a
    /// join started (other work waits for it).
    #[cfg(target_os = "macos")]
    fn process_join(&mut self, cx: &mut Context<Self>) -> bool {
        if self.maintenance.joining {
            return true;
        }
        let Some(paths) = self.maintenance.joins.pop_front() else { return false };
        let busy = |p: &PathBuf| {
            self.status.current_file.as_ref() == Some(p)
                || self.review_game.and_then(|i| self.games.get(i)).is_some_and(|g| &g.path == p)
        };
        let parts: Vec<(PathBuf, SessionMeta)> = paths
            .iter()
            .filter_map(|p| self.games.iter().find(|g| &g.path == p).map(|g| (g.path.clone(), g.meta.clone())))
            .collect();
        if paths.iter().any(busy) {
            // Try again at the next maintenance run.
            for p in &paths {
                self.maintenance.track_checked.remove(p);
            }
            return self.process_join(cx);
        }
        // A single path is a repair of an already-joined file.
        if paths.len() == 1 {
            self.maintenance.joining = true;
            cx.notify();
            let path = paths[0].clone();
            let task = cx.background_executor().spawn({
                let path = path.clone();
                async move { merge::repair(&path) }
            });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |app, cx| {
                    app.maintenance.joining = false;
                    match result {
                        Ok(()) => app.toast(IconName::ListVideo, "Repaired a joined game so the whole video plays.", None, cx),
                        Err(e) => {
                            app.maintenance.join_failed.insert(path.clone());
                            app.toast(IconName::X, format!("Couldn't repair a joined game: {e}"), None, cx);
                        }
                    }
                    app.reload_games();
                    app.process_condense(cx);
                });
            })
            .detach();
            return true;
        }
        if parts.len() < 2 {
            return self.process_join(cx);
        }
        self.maintenance.joining = true;
        cx.notify();
        let attempted = paths.clone();
        let task = cx.background_executor().spawn(async move { merge::run(&parts) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |app, cx| {
                app.maintenance.joining = false;
                match result {
                    Ok(joined) => {
                        let into = joined.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        app.store.update(cx, |s, cx| {
                            s.merge_games(&joined.parts, &into);
                            cx.notify();
                        });
                        // Clips cut from the pieces now point at the joined video.
                        for c in clip::list(&app.config.output_dir) {
                            if let Some((_, off)) = joined.parts.iter().find(|(k, _)| *k == c.meta.source) {
                                let mut m = c.meta.clone();
                                m.source = into.clone();
                                m.start += off;
                                m.end += off;
                                m.moment_time = m.moment_time.map(|t| t + off);
                                clip::write_meta(&c.path, &m);
                            }
                        }
                        log::info!("joined {} recordings into {}", joined.parts.len(), joined.path.display());
                        app.toast(
                            IconName::ListVideo,
                            format!("Joined {} recordings of one game into one video.", joined.parts.len()),
                            None,
                            cx,
                        );
                    }
                    Err(e) => {
                        app.maintenance.join_failed.extend(attempted);
                        log::error!("joining recordings: {e}");
                        app.toast(IconName::X, format!("Couldn't join split recordings: {e}"), None, cx);
                    }
                }
                app.reload_games();
                app.reload_clips();
                if app.maintenance.joins.is_empty() {
                    // All joined: now the usual cleanup and condensing.
                    app.run_maintenance(cx);
                } else {
                    app.process_condense(cx);
                }
            });
        })
        .detach();
        true
    }

    fn process_condense(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            if self.maintenance.active.is_some() || self.process_join(cx) {
                return;
            }
            let Some(path) = self.maintenance.queue.pop_front() else {
                // Queue drained: reset progress for next time.
                self.maintenance.done = 0;
                self.maintenance.total = 0;
                return;
            };
            let Some(game) = self.games.iter().find(|g| g.path == path) else { return self.process_condense(cx) };
            let open_here = self.review_game.and_then(|i| self.games.get(i)).is_some_and(|g| g.path == path);
            let ranges = self.condense_plan(game, cx);
            if !self.condensable(game, cx) || open_here || ranges.is_empty() {
                self.maintenance.done += 1;
                return self.process_condense(cx);
            }
            let (meta, bytes, key) = (game.meta.clone(), game.bytes, store::game_key(game));
            self.maintenance.active = Some(path.clone());
            cx.notify();
            let task = cx.background_executor().spawn({
                let path = path.clone();
                let keep_original = self.maintenance.manual.remove(&path);
                async move { condense::run(&path, &meta, bytes, &ranges, keep_original) }
            });
            cx.spawn(async move |this, cx| {
                let result = task.await;
                let _ = this.update(cx, |app, cx| {
                    app.maintenance.active = None;
                    app.maintenance.done += 1;
                    match result {
                        Ok(new_meta) => {
                            if let Some(info) = new_meta.condensed.clone() {
                                app.store.update(cx, |s, cx| {
                                    s.remap_notes(&key, |t| info.map(t));
                                    cx.notify();
                                });
                            }
                            let after = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(bytes);
                            app.maintenance.saved_bytes += bytes.saturating_sub(after);
                            log::info!("condensed {} ({} -> {})", path.display(), ui::human_bytes(bytes), ui::human_bytes(after));
                            let freed = bytes.saturating_sub(after);
                            app.toast(IconName::Scissors, format!("Kept the moments only ({} smaller).", ui::human_bytes(freed)), None, cx);
                        }
                        Err(e) => {
                            log::error!("condensing {}: {e}", path.display());
                            app.maintenance.last_error = Some(format!("Couldn't condense a game: {e}"));
                        }
                    }
                    app.reload_games();
                    app.process_condense(cx);
                    cx.notify();
                });
            })
            .detach();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
    }

    /// Apply the whole storage policy: delete what's due, then condense what's due.
    pub fn run_maintenance(&mut self, cx: &mut Context<Self>) {
        // First put split recordings of the same match back together.
        #[cfg(target_os = "macos")]
        {
            let queued: Vec<PathBuf> = self.maintenance.joins.iter().flatten().cloned().collect();
            // Joined videos from before mixed resolutions were handled can
            // have two video tracks (black after the first ends): repair them.
            for g in self.games.iter().filter(|g| g.meta.ended_at.is_some()) {
                if self.maintenance.track_checked.insert(g.path.clone()) && merge::video_track_count(&g.path) > 1 {
                    self.maintenance.joins.push_back(vec![g.path.clone()]);
                }
            }
            for group in merge::groups(&self.games) {
                let paths: Vec<PathBuf> = group.iter().map(|&i| self.games[i].path.clone()).collect();
                let failed = paths.iter().any(|p| self.maintenance.join_failed.contains(p));
                let recording = paths.iter().any(|p| self.status.current_file.as_ref() == Some(p));
                if !failed && !recording && !paths.iter().any(|p| queued.contains(p)) {
                    self.maintenance.joins.push_back(paths);
                }
            }
            if !self.maintenance.joins.is_empty() {
                self.process_condense(cx);
                // Cleanup and condensing run once the joins have finished.
                return;
            }
        }
        self.run_cleanup(cx);
        let due = self.condense_candidates(cx);
        if !due.is_empty() {
            self.queue_condense(due, cx);
        }
    }

    /// (Re)register the in-game bookmark key from config.
    pub fn apply_hotkey(&self) {
        #[cfg(target_os = "macos")]
        {
            let engine = self.engine.clone();
            // Holding the key auto-repeats; one bookmark per 1.5 s.
            let last = std::sync::Mutex::new(None::<std::time::Instant>);
            hotkey::set(self.config.bookmark_key.as_deref(), move || {
                let mut last = last.lock().unwrap();
                if last.is_some_and(|t| t.elapsed() < Duration::from_millis(1500)) {
                    return;
                }
                *last = Some(std::time::Instant::now());
                engine.send(Command::Bookmark);
                // Audible confirmation. Only League's audio is captured, so
                // this never ends up in the recording.
                let _ = std::process::Command::new("afplay")
                    .args(["-v", "0.35", "/System/Library/Sounds/Pop.aiff"])
                    .spawn();
            });
        }
    }

    /// Recordings the storage policy would delete right now.
    pub fn cleanup_plan(&self, cx: &App) -> Vec<PathBuf> {
        let store = self.store.read(cx);
        let recordings: Vec<_> =
            self.games.iter().map(|g| (g.path.clone(), g.bytes, g.meta.started_at)).collect();
        let current = self.status.current_file.clone();
        timewarp_engine::plan_cleanup(&recordings, &self.config.storage, |p| {
            current.as_deref() == Some(p)
                || self
                    .games
                    .iter()
                    .find(|g| g.path == p)
                    .and_then(|g| store.game(g))
                    .is_some_and(|n| n.is_protected())
        })
    }

    /// Manual delete: video and sidecar go to the Trash, so it's recoverable.
    pub fn delete_game(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.status.current_file.as_deref() == Some(path.as_path()) {
            self.toast(IconName::X, "That game is still recording.", None, cx);
            return;
        }
        if self.review_game.and_then(|i| self.games.get(i)).is_some_and(|g| g.path == path) {
            self.review = None;
            self.review_game = None;
            self.playlist = None;
        }
        let moved = deleted::trash(deleted::Kind::Game, None, "", &[path.clone(), path.with_extension("json")]);
        self.reload_games();
        if moved.is_empty() {
            self.toast(IconName::X, "Couldn't move that game to the Trash.", None, cx);
        } else {
            self.toast(IconName::Trash, "Game moved to the Trash.", Some(ui::ToastAction::Undo(moved)), cx);
        }
    }

    /// Set your role for a game by hand; saved in its sidecar.
    pub fn set_role(&mut self, path: &std::path::Path, role: Option<String>, cx: &mut Context<Self>) {
        let Some(g) = self.games.iter_mut().find(|g| g.path == path) else { return };
        g.meta.role_override = role.clone();
        if let Err(e) = timewarp_engine::write_sidecar(path, &g.meta) {
            log::warn!("saving role: {e}");
        }
        self.reload_games();
        let label = role.as_deref().map(filters::role_label).unwrap_or("automatic");
        self.toast(IconName::Swords, format!("Role set to {label}."), None, cx);
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.status.phase, Phase::Recording | Phase::Finalizing | Phase::WaitingForWindow)
    }

    /// Put trashed files back where they were.
    pub fn undo_trash(&mut self, files: Vec<(PathBuf, PathBuf)>, cx: &mut Context<Self>) {
        let restored = files.iter().filter(|(orig, in_trash)| std::fs::rename(in_trash, orig).is_ok()).count();
        deleted::forget_files(&files);
        self.reload_games();
        self.reload_clips();
        if restored == files.len() {
            self.toast(IconName::Undo2, "Restored.", None, cx);
        } else {
            self.toast(IconName::X, "Couldn't restore everything. Check the Trash.", None, cx);
        }
    }

    /// Move several games to the Trash at once (one Undo for all of them).
    pub fn trash_games(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let recording = self.status.current_file.clone();
        let paths: Vec<PathBuf> = paths.into_iter().filter(|p| Some(p) != recording.as_ref()).collect();
        if paths.is_empty() {
            return;
        }
        if self.review_game.and_then(|i| self.games.get(i)).is_some_and(|g| paths.contains(&g.path)) {
            self.review = None;
            self.review_game = None;
            self.playlist = None;
        }
        // One entry per game in Recently deleted, one Undo for all of them.
        let moved: Vec<(PathBuf, PathBuf)> = paths
            .iter()
            .flat_map(|p| deleted::trash(deleted::Kind::Game, None, "", &[p.clone(), p.with_extension("json")]))
            .collect();
        let games = moved.iter().filter(|(o, _)| o.extension().is_some_and(|e| e == "mp4")).count();
        for p in &paths {
            self.selected.remove(p);
        }
        self.reload_games();
        self.toast(
            IconName::Trash,
            match games {
                1 => "Game moved to the Trash.".to_string(),
                n => format!("{n} games moved to the Trash."),
            },
            Some(ui::ToastAction::Undo(moved)),
            cx,
        );
    }

    /// Keep (or stop keeping) several games.
    pub fn set_kept(&mut self, paths: &[PathBuf], kept: bool, cx: &mut Context<Self>) {
        let keys: Vec<String> = self.games.iter().filter(|g| paths.contains(&g.path)).map(store::game_key).collect();
        self.store.update(cx, |s, cx| {
            for k in &keys {
                s.game_mut(k).pinned = kept;
            }
            cx.notify();
        });
        self.toast(
            IconName::Pin,
            if kept { format!("Keeping {} games. Cleanup won't touch them.", keys.len()) } else { "No longer kept.".into() },
            None,
            cx,
        );
    }

    pub fn toggle_keep_game(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(g) = self.games.iter().find(|g| g.path == path) else { return };
        let key = store::game_key(g);
        let mut now_kept = false;
        self.store.update(cx, |s, cx| {
            let n = s.game_mut(&key);
            n.pinned = !n.pinned;
            now_kept = n.pinned;
            cx.notify();
        });
        self.toast(
            IconName::Pin,
            if now_kept { "Kept. Cleanup will never touch this game." } else { "No longer kept." },
            None,
            cx,
        );
    }

    /// Apply the storage policy: delete the video and its sidecar.
    pub fn run_cleanup(&mut self, cx: &mut Context<Self>) {
        let doomed = self.cleanup_plan(cx);
        if doomed.is_empty() {
            return;
        }
        for path in &doomed {
            log::info!("storage cleanup: deleting {}", path.display());
            let _ = std::fs::remove_file(path);
            let _ = std::fs::remove_file(path.with_extension("json"));
        }
        self.review = None;
        self.reload_games();
        cx.notify();
    }

    pub fn reload_games(&mut self) {
        self.selected.retain(|p| p.exists());
        self.games = timewarp_engine::list_recordings(&self.config.output_dir)
            .into_iter()
            .filter(|(p, _)| p.file_stem().is_some_and(|s| s != "test-recording"))
            .map(|(path, meta)| {
                let fs_meta = std::fs::metadata(&path).ok();
                let meta = meta.unwrap_or_else(|| SessionMeta {
                    started_at: fs_meta
                        .as_ref()
                        .and_then(|m| m.modified().ok())
                        .map(chrono::DateTime::<chrono::Local>::from)
                        .unwrap_or_default(),
                    ..Default::default()
                });
                let mut meta = meta;
                // Older recordings and Practice Tool games: work out the role.
                meta.roster.retain(|r| !r.champion.is_empty());
                // Holding the bookmark key used to add a burst of bookmarks.
                let mut last_bookmark = f64::NEG_INFINITY;
                meta.markers.retain(|m| {
                    if m.marker.kind != timewarp_league::MarkerKind::Bookmark {
                        return true;
                    }
                    let keep = m.video_time - last_bookmark >= 1.5;
                    last_bookmark = m.video_time;
                    keep
                });
                meta.infer_role();
                Game { path, meta, bytes: fs_meta.map(|m| m.len()).unwrap_or(0) }
            })
            .collect();
    }

    pub fn toast(&mut self, icon: IconName, text: impl Into<String>, action: Option<ui::ToastAction>, cx: &mut Context<Self>) {
        self.next_toast += 1;
        self.toasts.push(ui::Toast {
            id: self.next_toast,
            text: text.into(),
            icon,
            action,
            until: std::time::Instant::now() + Duration::from_secs(5),
        });
        // Keep it calm: at most three at once.
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(to) = self.dev_marquee_to {
            self.marquee_move(to, cx);
        }
        if self.checks_at.elapsed() >= Duration::from_secs(3) {
            self.checks_at = std::time::Instant::now();
            self.prune_deleted(cx);
            self.poll_updates(cx);
            let fresh = permissions::checks(&self.config);
            for c in &fresh {
                let was = self.checks.iter().find(|o| o.id == c.id).map(|o| o.state);
                if was.is_some_and(|w| w != permissions::State::Ok) && c.state == permissions::State::Ok {
                    self.toast(IconName::ShieldCheck, format!("{}: all set.", c.title), None, cx);
                }
            }
            if fresh.iter().map(|c| c.state).ne(self.checks.iter().map(|c| c.state)) {
                cx.notify();
            }
            self.checks = fresh;
        }
        let before = self.toasts.len();
        let now = std::time::Instant::now();
        self.toasts.retain(|t| t.until > now);
        if self.toasts.len() != before {
            cx.notify();
        }
        // Notes are written at most twice a second.
        self.store.update(cx, |s, _| s.flush());
        if self.page == Page::Clips && self.refresh_thumbs() {
            cx.notify();
        }
        let status = self.engine.status();
        let finished = status.last_finished != self.status.last_finished;
        if finished {
            self.reload_games();
        }
        let changed = status.phase != self.status.phase
            || status.elapsed.as_secs() != self.status.elapsed.as_secs()
            || status.markers != self.status.markers
            || status.champion != self.status.champion
            || status.last_error != self.status.last_error;
        let new_bookmarks = status.bookmarks.saturating_sub(self.status.bookmarks);
        self.status = status;
        if new_bookmarks > 0 {
            self.toast(IconName::Flag, "Bookmark added. You'll find it in this game's moments.", None, cx);
        }
        if finished {
            self.run_maintenance(cx);
        }
        // Outside review, also re-render so champion portraits appear as they download.
        if changed || self.review.is_none() {
            cx.notify();
        }
    }

    pub fn update_config(&mut self, f: impl FnOnce(&mut Config), cx: &mut Context<Self>) {
        let old_dir = self.config.output_dir.clone();
        let old_key = self.config.bookmark_key.clone();
        f(&mut self.config);
        if self.config.bookmark_key != old_key {
            self.apply_hotkey();
        }
        if let Err(e) = self.config.save() {
            log::warn!("saving config: {e}");
        }
        self.engine.send(Command::SetConfig(self.config.clone()));
        if self.config.output_dir != old_dir {
            self.reload_games();
        }
        cx.notify();
    }

    /// Games of the same matchup (your role, your champion, their champion).
    pub fn matchup_games(&self, key: &str) -> usize {
        self.games.iter().filter(|g| store::matchup_key(&g.meta).as_deref() == Some(key)).count()
    }

    pub fn open_review(&mut self, idx: usize, at: Option<f64>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_review_at(idx, at, true, window, cx);
    }

    pub fn open_review_at(&mut self, idx: usize, at: Option<f64>, record: bool, window: &mut Window, cx: &mut Context<Self>) {
        if idx >= self.games.len() {
            return;
        }
        if record {
            self.record_history(cx);
        }
        let game = &self.games[idx];
        let matchup = store::matchup_key(&game.meta).map(|key| Matchup {
            label: store::matchup_label(&game.meta).unwrap_or_default(),
            games: self.matchup_games(&key),
            key,
        });
        let (path, key, meta) = (game.path.clone(), store::game_key(game), game.meta.clone());
        let (store, icons, clips) = (self.store.clone(), self.icons.clone(), self.config.output_dir.clone());
        let (clip_opts, layout) = (self.config.clips, self.config.layout);
        let review = cx.new(|cx| Review::new(path, key, meta, matchup, at, store, icons, clips, clip_opts, layout, window, cx));
        cx.subscribe_in(&review, window, move |this, _, ev: &ReviewEvent, window, cx| match ev {
            ReviewEvent::Back => {
                if this.history.is_empty() {
                    this.review = None;
                    this.review_game = None;
                    this.playlist = None;
                    cx.notify();
                } else {
                    this.go_back(window, cx);
                }
            }
            ReviewEvent::PlayAll(times, title) => {
                let items = times.iter().map(|&t| (idx, t)).collect();
                this.start_playlist(title.clone(), items, window, cx);
            }
            ReviewEvent::ItemEnded => this.step_playlist(1, window, cx),
            ReviewEvent::Jumped { t, moment } => this.follow_jump(*t, *moment, cx),
            ReviewEvent::ClipStarted => {
                this.toast(IconName::LoaderCircle, "Saving clip…", None, cx);
                // Stays up until the clip is done.
                if let Some(t) = this.toasts.last_mut() {
                    t.until = std::time::Instant::now() + Duration::from_secs(120);
                }
            }
            ReviewEvent::ClipSaved(path) => {
                this.toasts.retain(|t| t.text != "Saving clip…");
                this.reload_clips();
                this.toast(
                    IconName::Scissors,
                    "Clip saved and copied. Paste it anywhere.",
                    Some(ui::ToastAction::OpenClip(path.clone())),
                    cx,
                );
            }
            ReviewEvent::OpenClip(path) => this.open_clip_path(path, window, cx),
            ReviewEvent::Layout(layout) => {
                let layout = *layout;
                this.update_config(|c| c.layout = layout, cx);
            }
            ReviewEvent::Delete => {
                if let Some(p) = this.games.get(idx).map(|g| g.path.clone()) {
                    this.delete_game(p, cx);
                }
            }
            ReviewEvent::Toast(text) => {
                this.toasts.retain(|t| t.text != "Saving clip…");
                this.toast(IconName::ClipboardCheck, text.clone(), None, cx)
            }
            ReviewEvent::Condense => {
                // Release the file before rewriting it.
                let path = this.games.get(idx).map(|g| g.path.clone());
                this.review = None;
                this.review_game = None;
                this.playlist = None;
                if let Some(p) = path {
                    this.queue_condense_manual(vec![p], cx);
                }
            }
        })
        .detach();
        self.review = Some(review);
        self.review_game = Some(idx);
        cx.notify();
    }

    pub fn start_playlist(&mut self, title: String, items: Vec<(usize, f64)>, window: &mut Window, cx: &mut Context<Self>) {
        if items.is_empty() {
            return;
        }
        self.record_history(cx);
        self.playlist = Some(Playlist { title, items, idx: 0 });
        self.play_current(window, cx);
    }

    fn play_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((game, t)) = self.playlist.as_ref().and_then(|p| p.items.get(p.idx).copied()) else { return };
        // Same game: just jump; otherwise swap the video.
        if self.review_game != Some(game) || self.review.is_none() {
            let playlist = self.playlist.take();
            self.open_review_at(game, None, false, window, cx);
            self.playlist = playlist;
        }
        if let Some(r) = &self.review {
            r.update(cx, |r, cx| r.play_item(t, cx));
        }
        cx.notify();
    }

    /// The user moved the playhead during Play all. Picking a moment in the
    /// playlist makes it the current one; small skips within the current
    /// moment keep going; going anywhere else ends Play all.
    fn follow_jump(&mut self, t: f64, moment: bool, cx: &mut Context<Self>) {
        let (Some(p), Some(game), Some(review)) = (&mut self.playlist, self.review_game, self.review.clone()) else { return };
        if moment {
            if let Some(i) = p.items.iter().position(|&(g, at)| g == game && (at - t).abs() < 0.05) {
                p.idx = i;
                review.update(cx, |r, cx| r.play_item(t, cx));
                cx.notify();
                return;
            }
        } else if let Some(&(g, at)) = p.items.get(p.idx) {
            if g == game && t >= at - review::LEAD_IN - 1.0 && t <= at + review::TAIL {
                return;
            }
        }
        self.playlist = None;
        review.update(cx, |r, _| r.end_item());
        cx.notify();
    }

    pub fn step_playlist(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &mut self.playlist else { return };
        let next = p.idx as isize + delta;
        if next < 0 || next as usize >= p.items.len() {
            // Finished: leave the last clip on screen, paused.
            self.playlist = None;
            cx.notify();
            return;
        }
        p.idx = next as usize;
        self.play_current(window, cx);
    }

    fn playlist_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = self.playlist.as_ref()?;
        let game = self.games.get(p.items[p.idx].0);
        let champ = game.and_then(|g| g.meta.champion.clone()).unwrap_or_default();
        Some(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_3()
                .h(px(40.))
                .px_4()
                .bg(theme::victory().opacity(0.14))
                .border_b_1()
                .border_color(theme::victory().opacity(0.4))
                .child(icon(IconName::ListVideo, 15., theme::victory()))
                .child(div().font_weight(FontWeight::MEDIUM).child(p.title.clone()))
                .child(div().text_sm().text_color(theme::muted()).child(format!("{} of {}", p.idx + 1, p.items.len())))
                .when(!champ.is_empty(), |d| d.child(div().text_sm().text_color(theme::muted()).child(champ)))
                .child(div().flex_1())
                .child(
                    ui::ghost_button("pl-prev")
                        .child(icon(IconName::SkipBack, 13., theme::muted()))
                        .child("Previous")
                        .on_click(cx.listener(|this, _, window, cx| this.step_playlist(-1, window, cx))),
                )
                .child(
                    ui::ghost_button("pl-next")
                        .child("Next")
                        .child(icon(IconName::SkipForward, 13., theme::muted()))
                        .on_click(cx.listener(|this, _, window, cx| this.step_playlist(1, window, cx))),
                )
                .child(ui::icon_button("pl-stop", IconName::X, 13.).on_click(cx.listener(|this, _, _, cx| {
                    this.playlist = None;
                    cx.notify();
                }))),
        )
    }

    /// "Games | Moments": two views of the same library.
    pub fn games_view_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().downgrade();
        ui::segmented("games-view", &[(Page::Games, "Games"), (Page::Moments, "Moments")], self.page, move |p, _, cx| {
            if let Some(app) = entity.upgrade() {
                app.update(cx, |app, cx| app.go(p, cx));
            }
        })
    }

    pub fn current_view(&self, cx: &App) -> View {
        View {
            page: self.page,
            review: self.review_game.and_then(|i| self.games.get(i)).map(|g| g.path.clone()),
            #[cfg(target_os = "macos")]
            clip: self.clip_view.as_ref().map(|v| v.read(cx).path().to_path_buf()),
            #[cfg(not(target_os = "macos"))]
            clip: None,
        }
    }

    /// Remember where we are before navigating somewhere else.
    pub fn record_history(&mut self, cx: &App) {
        let here = self.current_view(cx);
        if self.history.last() != Some(&here) {
            self.history.push(here);
            if self.history.len() > 100 {
                self.history.remove(0);
            }
        }
        self.future.clear();
    }

    fn apply_view(&mut self, v: View, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.page = v.page;
        self.review = None;
        self.review_game = None;
        self.playlist = None;
        self.clip_view = None;
        if let Some(i) = v.review.as_ref().and_then(|p| self.games.iter().position(|g| &g.path == p)) {
            self.open_review_at(i, None, false, window, cx);
        }
        #[cfg(target_os = "macos")]
        if let Some(i) = v.clip.as_ref().and_then(|p| self.clips.iter().position(|c| &c.path == p)) {
            self.open_clip_at(i, false, window, cx);
        }
        cx.notify();
    }

    pub fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prev) = self.history.pop() else { return };
        let here = self.current_view(cx);
        self.future.push(here);
        self.apply_view(prev, window, cx);
    }

    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(next) = self.future.pop() else { return };
        let here = self.current_view(cx);
        self.history.push(here);
        self.apply_view(next, window, cx);
    }

    pub fn go(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page != page || self.review.is_some() || self.clip_view.is_some() {
            self.record_history(cx);
        }
        if page != self.page {
            self.selected.clear();
            self.selected_clips.clear();
        }
        self.page = page;
        self.review = None;
        self.review_game = None;
        self.playlist = None;
        self.clip_view = None;
        cx.notify();
    }

    // -- sidebar -------------------------------------------------------------

    fn nav_item(&self, page: Page, name: IconName, label: &'static str, count: Option<usize>, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.page == page || (page == Page::Games && self.page == Page::Moments);
        div()
            .id(label)
            .flex()
            .items_center()
            .gap_2p5()
            .h(px(32.))
            .px_2p5()
            .rounded_md()
            .cursor_pointer()
            .text_color(if active { theme::text() } else { theme::muted() })
            .when(active, |d| d.bg(theme::raised()))
            .when(!active, |d| d.hover(|s| s.bg(theme::raised().opacity(0.5)).text_color(theme::text())))
            .child(icon(name, 16., if active { theme::victory() } else { theme::muted() }))
            .child(div().flex_1().child(label))
            .children(count.filter(|&n| n > 0).map(|n| div().text_sm().text_color(theme::muted()).child(n.to_string())))
            .when(page == Page::About && self.update_badge(), |d| {
                d.child(div().size(px(7.)).rounded_full().bg(theme::accent()))
            })
            .tooltip(ui::tip(match page {
                Page::Home => "Home  ⌘1",
                Page::Games | Page::Moments => "Games  ⌘2",
                Page::Matchups => "Matchups  ⌘3",
                Page::Clips => "Clips  ⌘4",
                Page::Settings => "Settings  ⌘,",
                Page::Deleted => "Games and clips in the Trash",
                Page::About if self.update_badge() => "An update is ready",
                Page::About => "About Timewarp",
                Page::Setup => "Permissions and setup",
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.go(page, cx)))
    }

    /// Compact recorder status: one line, plus a detail line only when it matters.
    fn recorder_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.status;
        let recording = matches!(s.phase, Phase::Recording | Phase::Finalizing);
        let armed = s.phase == Phase::Armed;
        let (title, detail): (String, Option<String>) = match s.phase {
            Phase::Armed => ("Waiting for League".into(), Some("Records your game once it opens".into())),
            Phase::Recording => (
                format!("Recording {}", theme::fmt_clock(s.elapsed.as_secs_f64())),
                Some(match (&s.champion, s.markers) {
                    (Some(c), 0) => c.clone(),
                    (Some(c), 1) => format!("{c}, 1 moment"),
                    (Some(c), n) => format!("{c}, {n} moments"),
                    (None, _) => "Waiting for game data".into(),
                }),
            ),
            Phase::Finalizing => ("Saving game".into(), None),
            Phase::WaitingForWindow => ("Match running".into(), Some("Switch to League to start recording".into())),
            Phase::Skipped => ("Skipping this game".into(), Some("Not in \"Which games\"".into())),
            Phase::Idle if self.config.auto_record => ("Ready".into(), Some("Records your next match".into())),
            Phase::Idle => ("Auto-record off".into(), None),
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2p5()
            .rounded_lg()
            .bg(theme::panel())
            .border_1()
            .border_color(if recording { theme::live().opacity(0.45) } else { theme::line() })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(8.)).flex_none().rounded_full().bg(if recording || armed {
                        theme::live()
                    } else if self.config.auto_record {
                        theme::assist()
                    } else {
                        theme::muted().opacity(0.5)
                    }))
                    .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(title))
                    .child(
                        div()
                            .id("record")
                            .size(px(26.))
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .bg(if recording { theme::live() } else { theme::raised() })
                            .hover(|st| st.opacity(0.85))
                            .child(icon(
                                if recording { IconName::Square } else { IconName::Circle },
                                11.,
                                if recording { theme::text() } else { theme::live() },
                            ))
                            .when(armed, |d| d.bg(theme::live().opacity(0.25)))
                            .tooltip(ui::tip(if recording {
                                "Stop recording"
                            } else if armed {
                                "Cancel"
                            } else {
                                "Record League now, or as soon as a game opens"
                            }))
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.engine.send(if recording || armed { Command::Stop } else { Command::StartManual })
                            })),
                    ),
            )
            .children(detail.map(|d| div().pl(px(16.)).text_xs().text_color(theme::muted()).truncate().child(d)))
            .children(s.last_error.clone().map(|e| div().pl(px(16.)).text_xs().text_color(theme::death()).child(e)))
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let deleted_count = deleted::count();
        let matchup_count = self
            .games
            .iter()
            .filter_map(|g| store::matchup_key(&g.meta))
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let bytes: u64 = self.games.iter().map(|g| g.bytes).sum();

        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .bg(theme::sidebar())
            .border_r_1()
            .border_color(theme::line())
            // traffic-light row doubles as a drag handle
            .child(div().flex().h(px(52.)).flex_none().child(ui::drag_area()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .px_4()
                    .pb_5()
                    .child(
                        div()
                            .size(px(26.))
                            .rounded_md()
                            .bg(theme::victory())
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(IconName::Rewind, 14., theme::bg())),
                    )
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Timewarp")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_2p5()
                    .when(self.checks.iter().any(|c| c.needs_attention()), |d| {
                        d.child(self.nav_item(Page::Setup, IconName::ShieldAlert, "Setup", None, cx))
                    })
                    .child(self.nav_item(Page::Home, IconName::House, "Home", None, cx))
                    .child(self.nav_item(Page::Games, IconName::Gamepad2, "Games", Some(self.games.len()), cx))
                    .child(self.nav_item(Page::Matchups, IconName::Swords, "Matchups", Some(matchup_count), cx))
                    .child(self.nav_item(Page::Clips, IconName::Scissors, "Clips", Some(self.clips.len()), cx)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .px_2p5()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    // Only while there's something to restore.
                    .when(deleted_count > 0 || self.page == Page::Deleted, |d| {
                        d.child(self.nav_item(Page::Deleted, IconName::Trash, "Recently deleted", Some(deleted_count), cx))
                    })
                    .child(self.nav_item(Page::Settings, IconName::Settings, "Settings", None, cx))
                    .child(self.nav_item(Page::About, IconName::Info, "About", None, cx)),
            )
            .child(div().px_2p5().child(self.recorder_card(cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(icon(IconName::HardDrive, 14., theme::muted()))
                    .child(div().flex_1().min_w_0().truncate().child(match (&self.maintenance.active, self.maintenance.total) {
                        _ if self.maintenance.joining => "Joining recordings…".to_string(),
                        (Some(_), total) => format!("Keeping moments only, {} of {}", self.maintenance.done + 1, total.max(1)),
                        _ => format!("{} in {} recordings", ui::human_bytes(bytes), self.games.len()),
                    })),
            )
    }
}

impl Render for TimewarpApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match (&self.review, self.page) {
            _ if self.clip_view.is_some() => self.clip_view.clone().unwrap().into_any_element(),
            (Some(r), _) => r.clone().into_any_element(),
            (None, Page::Clips) => self.clips_page(cx).into_any_element(),
            (None, Page::Home) => self.home_page(cx).into_any_element(),
            (None, Page::Setup) => self.setup_page(cx).into_any_element(),
            (None, Page::Games) => self.games_page(cx).into_any_element(),
            (None, Page::Moments) => self.moments_page(cx).into_any_element(),
            (None, Page::Matchups) => self.matchups_page(window, cx).into_any_element(),
            (None, Page::Settings) => self.settings_page(cx).into_any_element(),
            (None, Page::Deleted) => self.deleted_page(cx).into_any_element(),
            (None, Page::About) => self.about_page(cx).into_any_element(),
        };
        div()
            .relative()
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .bg(theme::bg())
            .text_color(theme::text())
            .on_action(cx.listener(|this, _: &GoHome, _, cx| this.go(Page::Home, cx)))
            .on_action(cx.listener(|this, _: &GoGames, _, cx| this.go(Page::Games, cx)))
            .on_action(cx.listener(|this, _: &GoMatchups, _, cx| this.go(Page::Matchups, cx)))
            .on_action(cx.listener(|this, _: &GoClips, _, cx| this.go(Page::Clips, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.go(Page::Settings, cx)))
            .on_action(cx.listener(|this, _: &OpenAbout, _, cx| this.go(Page::About, cx)))
            .on_action(cx.listener(|this, _: &CheckForUpdates, _, cx| {
                this.about_tab = about::AboutTab::Overview;
                this.go(Page::About, cx);
                this.check_for_updates(false, cx);
            }))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(cx.listener(|this, _: &GoBack, window, cx| this.go_back(window, cx)))
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| this.open_palette(window, cx)))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let k = &ev.keystroke;
                // `?` shows every shortcut (only when not typing in a field).
                if this.show_shortcuts && (k.key == "escape" || k.key == "?") {
                    this.show_shortcuts = false;
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if k.key == "?" && this.palette.is_none() && this.focus.is_focused(window) {
                    this.show_shortcuts = true;
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                // Games list / Clips grid: ⌘A or Ctrl+A select all, Esc clear, Delete/Backspace trash.
                if this.review.is_some() || this.clip_view.is_some() {
                    return;
                }
                if this.page == Page::Clips {
                    if this.clip_list_key(k, window, cx) {
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    match k.key.as_str() {
                        "a" if k.modifiers.platform || k.modifiers.control => {
                            let all: Vec<PathBuf> = this.clips.iter().map(|c| c.path.clone()).collect();
                            this.selected_clips.extend(all);
                        }
                        "escape" if !this.selected_clips.is_empty() => this.selected_clips.clear(),
                        "backspace" | "delete" if !this.selected_clips.is_empty() => {
                            let paths: Vec<PathBuf> = this.selected_clips.iter().cloned().collect();
                            this.trash_clips(paths, cx);
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if this.page != Page::Games {
                    return;
                }
                if this.game_list_key(k, window, cx) {
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                match k.key.as_str() {
                    "a" if k.modifiers.platform || k.modifiers.control => {
                        let all: Vec<PathBuf> = this.visible_games().into_iter().map(|i| this.games[i].path.clone()).collect();
                        this.selected.extend(all);
                    }
                    "escape" if !this.selected.is_empty() => this.selected.clear(),
                    "backspace" | "delete" if !this.selected.is_empty() => {
                        let paths: Vec<PathBuf> = this.selected.iter().cloned().collect();
                        this.trash_games(paths, cx);
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| this.go_forward(window, cx)))
            // Rubber-band selection follows the mouse anywhere in the window.
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if this.marquee.is_some() && ev.pressed_button == Some(MouseButton::Left) {
                    this.marquee_move(ev.position, cx);
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.marquee_up(cx)))
            // Mouse back/forward buttons.
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| this.go_back(window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| this.go_forward(window, cx)),
            )
            .child(self.sidebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .children(self.setup_banner(cx))
                    .children(self.playlist_bar(cx))
                    .child(div().flex_1().min_h_0().flex().flex_col().child(content)),
            )
            .children(self.marquee_overlay())
            .children(self.palette_overlay(cx))
            .children(self.shortcuts_overlay(cx))
            .child(
                // toasts, bottom right
                div()
                    .absolute()
                    .bottom_4()
                    .right_4()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap_2()
                    .children(self.toasts.iter().map(|t| {
                        let id = t.id;
                        let (e1, e2) = (cx.entity().downgrade(), cx.entity().downgrade());
                        ui::toast_view(
                            t,
                            move |_, cx| {
                                if let Some(app) = e1.upgrade() {
                                    app.update(cx, |app, cx| {
                                        app.toasts.retain(|t| t.id != id);
                                        cx.notify();
                                    });
                                }
                            },
                            move |path, window, cx| {
                                if let Some(app) = e2.upgrade() {
                                    app.update(cx, |app, cx| {
                                        app.toasts.retain(|t| t.id != id);
                                        app.open_clip_path(&path, window, cx);
                                    });
                                }
                            },
                            {
                                let e3 = cx.entity().downgrade();
                                move |files, _, cx| {
                                    if let Some(app) = e3.upgrade() {
                                        app.update(cx, |app, cx| {
                                            app.toasts.retain(|t| t.id != id);
                                            app.undo_trash(files, cx);
                                        });
                                    }
                                }
                            },
                            {
                                let e4 = cx.entity().downgrade();
                                move |page, _, cx| {
                                    if let Some(app) = e4.upgrade() {
                                        app.update(cx, |app, cx| {
                                            app.toasts.retain(|t| t.id != id);
                                            app.go(page, cx);
                                        });
                                    }
                                }
                            },
                        )
                    })),
            )
    }
}

fn apply_theme(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let t = Theme::global_mut(cx);
    t.background = theme::bg();
    t.foreground = theme::text();
    t.border = theme::line();
    t.muted = theme::raised();
    t.muted_foreground = theme::muted();
    t.primary = theme::victory();
    t.primary_hover = theme::victory();
    t.primary_foreground = theme::bg();
    t.switch = theme::line();
    t.popover = theme::panel();
    t.popover_foreground = theme::text();
    t.scrollbar_thumb = theme::line();
    t.ring = theme::accent();
    // Keep gpui-kit's own controls on the same near-black + azure palette.
    t.primary_active = theme::accent();
    t.accent = theme::raised();
    t.accent_foreground = theme::text();
    t.secondary = theme::raised();
    t.secondary_hover = theme::line();
    t.secondary_foreground = theme::text();
    t.input = theme::line();
    t.list_hover = theme::raised();
    t.list_active = theme::accent().opacity(0.18);
    t.selection = theme::accent().opacity(0.35);
    t.link = theme::accent();
    t.link_hover = theme::accent();
    t.title_bar = theme::sidebar();
    t.sidebar = theme::sidebar();
    t.window_border = theme::line();
    t.caret = theme::accent();
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    gpui_kit::application().with_assets(ui::AppAssets).run(|cx| {
        gpui_kit::init(cx);
        apply_theme(cx);
        // Standard Mac menu and shortcuts (⌘Q finalizes any recording first).
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &HideApp, cx| cx.hide());
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-h", HideApp, None),
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-1", GoHome, None),
            KeyBinding::new("cmd-2", GoGames, None),
            KeyBinding::new("cmd-3", GoMatchups, None),
            KeyBinding::new("cmd-4", GoClips, None),
            KeyBinding::new("cmd-k", OpenPalette, None),
            KeyBinding::new("ctrl-k", OpenPalette, None),
            KeyBinding::new("cmd-[", GoBack, None),
            KeyBinding::new("cmd-]", GoForward, None),
        ]);
        cx.set_menus([
            Menu {
                name: "Timewarp".into(),
                items: vec![
                    MenuItem::action("About Timewarp", OpenAbout),
                    MenuItem::action("Check for Updates…", CheckForUpdates),
                    MenuItem::separator(),
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Hide Timewarp", HideApp),
                    MenuItem::separator(),
                    MenuItem::action("Quit Timewarp", Quit),
                ],
                disabled: false,
            },
            Menu {
                name: "Go".into(),
                items: vec![
                    MenuItem::action("Search and Commands…", OpenPalette),
                    MenuItem::separator(),
                    MenuItem::action("Back", GoBack),
                    MenuItem::action("Forward", GoForward),
                    MenuItem::separator(),
                    MenuItem::action("Home", GoHome),
                    MenuItem::action("Games", GoGames),
                    MenuItem::action("Matchups", GoMatchups),
                    MenuItem::action("Clips", GoClips),
                ],
                disabled: false,
            },
            Menu { name: "Window".into(), items: vec![MenuItem::action("Minimize", Minimize)], disabled: false },
        ]);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1240.), px(800.)), cx))),
            window_min_size: Some(size(px(960.), px(560.))),
            titlebar: Some(TitlebarOptions {
                title: Some("Timewarp".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(19.))),
            }),
            ..Default::default()
        };
        let (window, app) = gpui_kit::open_window(options, cx, |_, cx| cx.new(TimewarpApp::new)).expect("open window");
        // Dev aid for screenshots: TIMEWARP_PAGE=games|moments|settings|review[:N]
        if let Ok(page) = std::env::var("TIMEWARP_PAGE") {
            let _ = window.update(cx, |_, window, cx| {
                app.update(cx, |app, cx| match page.split(':').next() {
                    Some("games") => app.go(Page::Games, cx),
                    Some("setup") => app.go(Page::Setup, cx),
                    Some("games-marquee") => {
                        // Dev: a rubber band dragged from the list up past its top edge; the
                        // poll tick runs the real hit test once rows have bounds.
                        app.go(Page::Games, cx);
                        app.marquee_down(marquee::SelectTarget::Games, point(px(300.), px(420.)), Modifiers::default());
                        app.dev_marquee_to = Some(point(px(900.), px(10.)));
                    }
                    Some("games-select") => {
                        app.go(Page::Games, cx);
                        let two: Vec<PathBuf> = app.games.iter().take(2).map(|g| g.path.clone()).collect();
                        app.selected.extend(two);
                    }
                    Some("moments") => app.go(Page::Moments, cx),
                    Some("matchups") => app.go(Page::Matchups, cx),
                    Some("clips") => app.go(Page::Clips, cx),
                    Some("clip") => {
                        app.go(Page::Clips, cx);
                        app.open_clip(0, window, cx)
                    }
                    Some("playlist") => {
                        app.go(Page::Moments, cx);
                        let items: Vec<(usize, f64)> = app
                            .games
                            .iter()
                            .enumerate()
                            .flat_map(|(i, g)| g.meta.markers.iter().take(3).map(move |m| (i, m.video_time.min(10.0))))
                            .collect();
                        app.start_playlist("Deaths".into(), items, window, cx)
                    }
                    Some("settings") => app.go(Page::Settings, cx),
                    Some("review") => {
                        let idx = page.split(':').nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
                        app.open_review(idx, None, window, cx)
                    }
                    _ => {}
                })
            });
        }
        let _ = window.update(cx, |_, window, cx| {
            let focus = app.read(cx).focus.clone();
            window.focus(&focus, cx);
            // The close button must never cut a recording off. While a game is
            // being recorded it hides the app (recording continues; click the
            // Dock icon to come back); otherwise it quits the safe way, like ⌘Q.
            let app = app.clone();
            window.on_window_should_close(cx, move |_, cx| {
                let busy = matches!(
                    app.read(cx).status.phase,
                    Phase::Recording | Phase::Finalizing | Phase::WaitingForWindow | Phase::Armed
                );
                if busy {
                    cx.hide();
                } else {
                    cx.quit();
                }
                false
            });
        });
        cx.activate(true);
    });
}
