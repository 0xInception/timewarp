//! Review screen: watch a recording, jump between moments, and turn it into
//! study material — timestamped notes, saved moments, game and matchup notes.
//!
//! Keyboard (when the video has focus): space play/pause, ←/→ 5s,
//! shift+←/→ previous/next moment, N add a note here, esc back.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use timewarp_engine::SessionMeta;

use crate::icons::Icons;
use crate::player::VideoPlayer;
use crate::store::{self, Moment, MomentKind, Store};
use crate::ui::{self, MomentFilter, icon};
use crate::draw::{self, Sketch};
use crate::{theme, timeline};

/// Playback speeds offered in the menu and stepped through with < and >.
const SPEEDS: [f32; 6] = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0];

fn speed_label(s: f32) -> String {
    let t = format!("{s}");
    format!("{}×", t.trim_start_matches('0'))
}

/// Start this many seconds before a moment so you see the setup.
pub const LEAD_IN: f64 = 6.0;
/// Keep playing this long after a moment in playlists and clips.
pub const TAIL: f64 = 4.0;

pub enum ReviewEvent {
    Back,
    /// "Play all" for the moments currently shown: (video times, title).
    PlayAll(Vec<f64>, String),
    /// A playlist item reached its end time.
    ItemEnded,
    /// The user moved the playhead: to a moment (`moment`: its time), or
    /// anywhere else (the new position). The playlist follows or ends.
    Jumped { t: f64, moment: bool },
    /// Cut this game down to its moments (the app closes the review first).
    Condense,
    /// A clip was exported (app refreshes the library and offers to watch it).
    ClipSaved(PathBuf),
    /// Watch a clip in the app.
    OpenClip(PathBuf),
    /// A clip started saving (the app shows progress until ClipSaved/Toast).
    ClipStarted,
    /// Move this game to the Trash.
    Delete,
    /// Show a confirmation toast.
    Toast(String),
    /// Side panel resized or shown/hidden; the app remembers it.
    Layout(timewarp_engine::Layout),
}

/// Which long-form note is open in its editor (others show rendered).
#[derive(Clone, Copy, PartialEq, Eq)]
enum NoteField {
    Game,
    Matchup,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Moments,
    Clips,
    Notes,
}

pub struct Matchup {
    pub key: String,
    pub label: String,
    pub games: usize,
}

pub struct Review {
    path: PathBuf,
    game_key: String,
    meta: SessionMeta,
    matchup: Option<Matchup>,
    store: Entity<Store>,
    player: VideoPlayer,
    filter: MomentFilter,
    tab: Tab,
    focus: FocusHandle,
    icons: Arc<Icons>,
    /// Keep pulling frames briefly after a seek, even while paused.
    wake_until: Instant,
    /// Video time a new note is being written for.
    adding_note_at: Option<f64>,
    note_input: Entity<InputState>,
    game_notes: Entity<TextareaState>,
    matchup_notes: Option<Entity<TextareaState>>,
    /// Playlist mode: stop and hand over to the next item at this time.
    play_until: Option<f64>,
    /// Ignore the end check until the seek for a new item has landed.
    item_guard: Instant,
    clip_in: Option<f64>,
    clip_out: Option<f64>,
    /// Latest clip export status and the file it produced.
    clip_status: Option<(String, Option<PathBuf>)>,
    clips_dir: PathBuf,
    clip_opts: timewarp_engine::ClipSettings,
    /// Clips made from this game.
    clips: Vec<crate::clip::Clip>,
    /// Timeline hover position (for the preview label).
    hover_t: Option<f64>,
    /// Timeline zoom: visible span in seconds (None = whole game) and where
    /// the visible window starts.
    view_span: Option<f64>,
    view_start: f64,
    editing: Option<NoteField>,
    /// Side panel (moments, clips, notes): width, visibility, drag state.
    layout: timewarp_engine::Layout,
    resizing: bool,
    confirm_condense: bool,
    /// Playback speed (`play()` uses it; J/K/L shuttle changes it briefly).
    speed: f32,
    /// Draw mode: on while Some.
    sketch: Option<Sketch>,
    /// Where the video element is on screen, for mapping the pen.
    video_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Second, muted player for timeline hover pictures (made on first hover)
    /// and the time it was last asked for.
    preview: Option<VideoPlayer>,
    preview_at: f64,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ReviewEvent> for Review {}

impl Review {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: PathBuf,
        game_key: String,
        meta: SessionMeta,
        matchup: Option<Matchup>,
        start_at: Option<f64>,
        store: Entity<Store>,
        icons: Arc<Icons>,
        clips_dir: PathBuf,
        clip_opts: timewarp_engine::ClipSettings,
        layout: timewarp_engine::Layout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let player = VideoPlayer::open(&path);
        if let Some(t) = start_at {
            player.seek((t - LEAD_IN).max(0.0));
        }
        player.play();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        let (game_text, matchup_text) = {
            let s = store.read(cx);
            (
                s.games.get(&game_key).map(|g| g.note.clone()).unwrap_or_default(),
                matchup.as_ref().and_then(|m| s.matchup(&m.key)).map(|m| m.notes.clone()).unwrap_or_default(),
            )
        };

        let note_input = cx.new(|cx| InputState::new(window, cx).placeholder("What happened here? Enter to save"));
        let game_notes = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("What went well, what to fix next game…")
                .default_value(game_text)
                .auto_grow(4, 14)
        });
        let matchup_notes = matchup.as_ref().map(|m| {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(format!("Tips for {}: power spikes, trades to take or avoid, builds…", m.label))
                    .default_value(matchup_text)
                    .auto_grow(4, 14)
            })
        });

        let mut subs = vec![cx.subscribe_in(&note_input, window, |this, input, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                let mut text = input.read(cx).value().trim().to_string();
                // A drawing goes with the note; a drawing alone is a note too.
                let drawing = this.sketch.as_ref().map(|s| s.strokes.clone()).unwrap_or_default();
                if text.is_empty() && !drawing.is_empty() {
                    text = "Drawing".into();
                }
                if let (Some(t), false) = (this.adding_note_at, text.is_empty()) {
                    let key = this.game_key.clone();
                    this.store.update(cx, |s, cx| {
                        s.add_note(&key, t, text, drawing);
                        cx.notify();
                    });
                    this.sketch = None;
                }
                this.finish_note(window, cx);
            }
        })];
        subs.push(cx.subscribe_in(&game_notes, window, |this, state, ev: &InputEvent, _, cx| {
            if let InputEvent::Blur = ev {
                if this.editing == Some(NoteField::Game) {
                    this.editing = None;
                    cx.notify();
                }
            }
            if let InputEvent::Change = ev {
                let text = state.read(cx).value().to_string();
                let key = this.game_key.clone();
                this.store.update(cx, |s, cx| {
                    s.game_mut(&key).note = text;
                    cx.notify();
                });
            }
        }));
        if let (Some(m), Some(editor)) = (&matchup, &matchup_notes) {
            let key = m.key.clone();
            subs.push(cx.subscribe_in(editor, window, move |this, state, ev: &InputEvent, _, cx| {
                if let InputEvent::Blur = ev {
                    if this.editing == Some(NoteField::Matchup) {
                        this.editing = None;
                        cx.notify();
                    }
                }
                if let InputEvent::Change = ev {
                    let text = state.read(cx).value().to_string();
                    let key = key.clone();
                    this.store.update(cx, |s, cx| {
                        s.set_matchup_notes(&key, text);
                        cx.notify();
                    });
                }
            }));
        }
        subs.push(cx.observe(&store, |_, _, cx| cx.notify()));

        let mut this = Self {
            path,
            game_key,
            meta,
            matchup,
            store,
            player,
            filter: MomentFilter::All,
            tab: Tab::Moments,
            focus,
            icons,
            wake_until: Instant::now() + Duration::from_millis(600),
            adding_note_at: None,
            note_input,
            game_notes,
            matchup_notes,
            play_until: None,
            item_guard: Instant::now(),
            clip_in: None,
            clip_out: None,
            clip_status: None,
            clips_dir,
            clip_opts,
            clips: vec![],
            hover_t: None,
            // Dev aid for screenshots: TIMEWARP_ZOOM=<seconds visible>.
            view_span: std::env::var("TIMEWARP_ZOOM").ok().and_then(|v| v.parse().ok()),
            view_start: 0.0,
            editing: None,
            layout,
            resizing: false,
            confirm_condense: false,
            speed: 1.0,
            sketch: None,
            video_bounds: Rc::new(Cell::new(Bounds::default())),
            preview: None,
            preview_at: f64::NAN,
            _subs: subs,
        };
        this.reload_clips();
        this
    }

    fn reload_clips(&mut self) {
        let key = self.game_key.clone();
        self.clips = crate::clip::list(&self.clips_dir).into_iter().filter(|c| c.meta.source == key).collect();
    }

    pub fn pause(&mut self) {
        self.player.pause();
    }

    /// The clip already made around this moment, if any.
    fn clip_for(&self, video_time: f64) -> Option<PathBuf> {
        self.clips
            .iter()
            .find(|c| c.meta.moment_time.is_some_and(|t| (t - video_time).abs() < 0.5))
            .map(|c| c.path.clone())
    }

    fn clips_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.clips.is_empty() {
            return div()
                .p_4()
                .text_sm()
                .text_color(theme::muted())
                .child("No clips from this game yet. Use Clip under the video, or the scissors on any moment.")
                .into_any_element();
        }
        div()
            .id("review-clips")
            .flex_1()
            .overflow_y_scroll()
            .p_2()
            .children(self.clips.iter().enumerate().map(|(i, c)| {
                let path = c.path.clone();
                div()
                    .id(("review-clip", i))
                    .flex()
                    .items_center()
                    .gap_3()
                    .p_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::raised()))
                    .child(
                        div()
                            .relative()
                            .w(px(112.))
                            .h(px(63.))
                            .flex_none()
                            .rounded(px(5.))
                            .overflow_hidden()
                            .bg(theme::panel())
                            .children(c.thumb.clone().map(|t| img(t).size_full().object_fit(ObjectFit::Cover)))
                            .child(
                                div()
                                    .absolute()
                                    .bottom_1()
                                    .right_1()
                                    .px_1()
                                    .rounded(px(3.))
                                    .bg(gpui_kit::black().opacity(0.7))
                                    .text_xs()
                                    .child(theme::fmt_clock(c.duration())),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_sm().truncate().child(c.title()))
                            .children(c.meta.game_time.map(|t| {
                                div().text_xs().text_color(theme::muted()).child(format!("At {} in game", theme::fmt_clock(t)))
                            })),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(ReviewEvent::OpenClip(path.clone()))))
            }))
            .into_any_element()
    }

    /// Playlist item: play from the lead-in to `t + TAIL`, then emit ItemEnded.
    pub fn play_item(&mut self, t: f64, cx: &mut Context<Self>) {
        self.play_until = Some(t + TAIL);
        self.item_guard = Instant::now() + Duration::from_millis(500);
        self.seek((t - LEAD_IN).max(0.0));
        self.player.play();
        cx.notify();
    }

    /// Leave playlist mode: play on freely from here.
    pub fn end_item(&mut self) {
        self.play_until = None;
    }

    /// A seek the user made (timeline, arrow keys): let the playlist know.
    fn user_seek(&mut self, t: f64, cx: &mut Context<Self>) {
        self.seek(t);
        if self.play_until.is_some() {
            cx.emit(ReviewEvent::Jumped { t, moment: false });
        }
    }

    /// Open at a specific source time (from a clip's "Open in game").
    pub fn seek_to(&mut self, t: f64, cx: &mut Context<Self>) {
        self.seek(t.max(0.0));
        self.player.play();
        cx.notify();
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    fn export_clip(&mut self, start: f64, end: f64, label: String, moment: Option<&Moment>, opts: timewarp_engine::ClipSettings, cx: &mut Context<Self>) {
        let (src, at) = (self.path.clone(), start);
        let dst = crate::clip::clip_path(&self.clips_dir, &self.path, &label, at + LEAD_IN.min(end - start));
        let m = &self.meta;
        let meta = crate::clip::ClipMeta {
            title: match (&m.champion, moment) {
                (Some(c), Some(_)) => format!("{c}: {label}"),
                _ => label.clone(),
            },
            kind: moment.and_then(|mo| match mo.kind {
                MomentKind::Event(k) => Some(k),
                MomentKind::Note => None,
            }),
            note: moment.is_some_and(|mo| mo.kind == MomentKind::Note),
            source: self.game_key.clone(),
            start,
            end,
            moment_time: moment.map(|mo| mo.video_time),
            game_time: moment.and_then(|mo| mo.game_time),
            champion: m.champion.clone(),
            champion_key: m.player.as_ref().map(|p| p.champion_key.clone()),
            opponent: m.opponent.as_ref().map(|o| o.champion.clone()),
            result: m.result.clone(),
            queue: m.queue.clone(),
            created_at: Some(chrono::Local::now()),
        };
        cx.emit(ReviewEvent::ClipStarted);
        cx.notify();
        let task = cx.background_executor().spawn({
            let dst = dst.clone();
            async move {
                crate::clip::export_with(&src, &dst, start, end, &opts)?;
                crate::clip::write_meta(&dst, &meta);
                crate::clip::make_thumbnail(&dst);
                Ok::<(), String>(())
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.clip_status = None;
                match result {
                    Ok(()) => {
                        crate::clip::copy_file_to_clipboard(&dst);
                        this.reload_clips();
                        cx.emit(ReviewEvent::ClipSaved(dst));
                    }
                    Err(e) => cx.emit(ReviewEvent::Toast(format!("Couldn't save the clip: {e}"))),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn export_moment(&mut self, m: &Moment, cx: &mut Context<Self>) {
        let t = m.video_time;
        self.export_clip((t - LEAD_IN).max(0.0), t + TAIL, m.label.clone(), Some(m), self.clip_opts, cx);
    }

    fn export_range(&mut self, opts: timewarp_engine::ClipSettings, cx: &mut Context<Self>) {
        if let (Some(a), Some(b)) = (self.clip_in, self.clip_out) {
            let (a, b) = (a.min(b), a.max(b));
            let label = format!("Clip at {}", theme::fmt_clock(a));
            self.export_clip(a, b, label, None, opts, cx);
        }
        self.clip_in = None;
        self.clip_out = None;
    }

    /// Quick picks on the Save clip button, besides the Settings default.
    fn clip_presets(&self) -> Vec<(String, timewarp_engine::ClipSettings)> {
        use timewarp_engine::{ClipFormat, ClipFps, ClipResolution, ClipSettings};
        let mine = self.clip_opts;
        let describe = |o: &ClipSettings| {
            let mut parts = vec![];
            if let Some(h) = o.resolution.max_height() {
                parts.push(format!("{h}p"));
            }
            if o.fps == ClipFps::Fps30 {
                parts.push("30 fps".into());
            }
            if let Some(mb) = o.max_mb {
                parts.push(format!("under {mb} MB"));
            }
            if parts.is_empty() {
                parts.push(if o.format == ClipFormat::Original { "original quality".into() } else { "full quality".into() });
            }
            parts.join(", ")
        };
        // Presets re-encode, so they use your codec (HEVC unless you chose H.264).
        let format = if mine.format == ClipFormat::Shareable { ClipFormat::Shareable } else { ClipFormat::Compact };
        let shareable = |max_mb, fps, resolution| ClipSettings { format, max_mb, fps, resolution };
        vec![
            (format!("My settings ({})", describe(&mine)), mine),
            ("Discord: under 10 MB, 30 fps".into(), shareable(Some(10), ClipFps::Fps30, ClipResolution::Original)),
            ("Small: 720p, 30 fps".into(), shareable(None, ClipFps::Fps30, ClipResolution::P720)),
            ("Best quality: no limit".into(), shareable(None, ClipFps::Original, ClipResolution::Original)),
        ]
    }

    /// Zoom the timeline by `factor` (0.5 = twice as close), around the playhead.
    fn zoom_by(&mut self, factor: f64) {
        let duration = self.duration().max(1.0);
        let span = self.view_span.unwrap_or(duration);
        let new_span = (span * factor).clamp(20.0_f64.min(duration), duration);
        if new_span >= duration - 0.5 {
            self.view_span = None;
            return;
        }
        let p = self.player.position();
        // Keep the playhead where it is on screen.
        let rel = if self.view_span.is_some() { ((p - self.view_start) / span).clamp(0.0, 1.0) } else { p / duration };
        self.view_start = (p - rel * new_span).clamp(0.0, duration - new_span);
        self.view_span = Some(new_span);
    }

    /// The part of the game the timeline shows; follows the playhead while playing.
    fn timeline_view(&mut self, position: f64, duration: f64, playing: bool) -> (f64, f64) {
        let Some(span) = self.view_span else { return (0.0, duration) };
        let span = span.min(duration);
        if playing && (position > self.view_start + span * 0.9 || position < self.view_start) {
            self.view_start = (position - span * 0.15).clamp(0.0, (duration - span).max(0.0));
        }
        (self.view_start, self.view_start + span)
    }

    /// Visible buttons for everything that also has a key: note and clip.
    fn tool_buttons(&self, position: f64, cx: &mut Context<Self>) -> impl IntoElement {
        let tool = |id: &'static str, name: IconName, label: String, key: &'static str| {
            ui::solid_button(id, theme::raised())
                .flex_none()
                .child(icon(name, 14., theme::text()))
                .child(label)
                .child(ui::kbd(key))
        };
        let clip: AnyElement = match (self.clip_in, self.clip_out) {
            (None, _) => tool("clip-start", IconName::Scissors, "Clip".into(), "I")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.clip_in = Some(this.player.position());
                    this.clip_out = None;
                    cx.notify();
                }))
                .into_any_element(),
            (Some(a), None) => div()
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .child(
                    ui::solid_button("clip-end", theme::victory())
                        .text_color(theme::bg())
                        .child(icon(IconName::Scissors, 14., theme::bg()))
                        .child(format!("End clip here ({})", theme::fmt_clock((position - a).abs())))
                        .child(ui::kbd("O"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.clip_out = Some(this.player.position());
                            this.player.pause();
                            cx.notify();
                        })),
                )
                .child(ui::icon_button("clip-cancel", IconName::X, 12.).on_click(cx.listener(|this, _, _, cx| {
                    this.clip_in = None;
                    cx.notify();
                })))
                .into_any_element(),
            (Some(a), Some(b)) => div()
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .child(
                    ui::solid_button("export-clip", theme::victory())
                        .text_color(theme::bg())
                        .child(icon(IconName::Scissors, 14., theme::bg()))
                        .child(format!("Save clip ({})", theme::fmt_clock((b - a).abs())))
                        .on_click(cx.listener(|this, _, _, cx| this.export_range(this.clip_opts, cx))),
                )
                // Other sizes for just this clip.
                .child({
                    let this = cx.entity().downgrade();
                    let presets = self.clip_presets();
                    Button::new("clip-quality")
                        .ghost()
                        .icon(Icon::new(IconName::ChevronDown))
                        .tooltip("Save at another size")
                        .dropdown_menu_with_anchor(gpui_kit::Anchor::BottomRight, move |mut menu, _, _| {
                            for (label, opts) in presets.clone() {
                                let this = this.clone();
                                menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                                    if let Some(r) = this.upgrade() {
                                        r.update(cx, |r, cx| r.export_range(opts, cx));
                                    }
                                }));
                            }
                            menu
                        })
                })
                .child(ui::icon_button("clip-cancel", IconName::X, 12.).on_click(cx.listener(|this, _, _, cx| {
                    this.clip_in = None;
                    this.clip_out = None;
                    cx.notify();
                })))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                tool("add-note-tb", IconName::StickyNote, "Note".into(), "N")
                    .on_click(cx.listener(|this, _, window, cx| this.start_note(window, cx))),
            )
            .child(
                tool("draw-tb", IconName::Pencil, "Draw".into(), "D")
                    .when(self.sketch.is_some(), |b| b.bg(theme::accent().opacity(0.25)))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_draw(cx))),
            )
            .child(clip)
    }

    fn toggle_keep(&mut self, cx: &mut Context<Self>) {
        let key = self.game_key.clone();
        self.store.update(cx, |s, cx| {
            let g = s.game_mut(&key);
            g.pinned = !g.pinned;
            cx.notify();
        });
    }

    fn duration(&self) -> f64 {
        self.player.duration().unwrap_or(self.meta.duration_secs)
    }

    fn all_moments(&self, cx: &App) -> Vec<Moment> {
        store::moments(&self.meta, self.store.read(cx).games.get(&self.game_key))
    }

    fn seek(&mut self, t: f64) {
        self.player.seek(t);
        self.wake_until = Instant::now() + Duration::from_millis(600);
    }

    /// Go to a moment (list row, previous/next).
    fn jump(&mut self, t: f64, cx: &mut Context<Self>) {
        self.seek((t - LEAD_IN).max(0.0));
        self.player.play();
        if self.play_until.is_some() {
            cx.emit(ReviewEvent::Jumped { t, moment: true });
        }
        cx.notify();
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        self.player.toggle();
        cx.notify();
    }

    fn step_moment(&mut self, forward: bool, cx: &mut Context<Self>) {
        // Compare against the lead-in point so repeated presses advance.
        let now = self.player.position() + LEAD_IN;
        let filter = self.filter;
        let times: Vec<f64> =
            self.all_moments(cx).into_iter().filter(|m| filter.matches(m)).map(|m| m.video_time).collect();
        let target = if forward {
            times.iter().copied().find(|&t| t > now + 0.5)
        } else {
            times.iter().rev().copied().find(|&t| t < now - 1.5)
        };
        if let Some(t) = target {
            self.jump(t, cx);
        }
    }

    fn set_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        self.speed = speed;
        self.player.set_speed(speed);
        cx.notify();
    }

    /// `<` / `>`: one step down or up the speed list.
    fn nudge_speed(&mut self, up: bool, cx: &mut Context<Self>) {
        let i = SPEEDS.iter().position(|&s| s == self.speed).unwrap_or(3);
        let j = if up { (i + 1).min(SPEEDS.len() - 1) } else { i.saturating_sub(1) };
        self.set_speed(SPEEDS[j], cx);
    }

    /// J/K/L: K pauses; L plays, then doubles up to 4×; J does the same
    /// backwards (or skips back 5 s when the file can't play in reverse).
    fn shuttle(&mut self, key: &str) {
        let rate = self.player.rate();
        match key {
            "k" => self.player.pause(),
            "l" if rate > 0.0 => self.player.set_rate((rate * 2.0).min(4.0)),
            "l" => self.player.play(),
            "j" if self.player.can_play_reverse() => {
                self.player.set_rate(if rate < 0.0 { (rate * 2.0).max(-4.0) } else { -1.0 })
            }
            "j" => self.seek(self.player.position() - 5.0),
            _ => {}
        }
    }

    fn step_frame(&mut self, frames: isize) {
        self.player.step(frames);
        self.wake_until = Instant::now() + Duration::from_millis(400);
    }

    fn toggle_draw(&mut self, cx: &mut Context<Self>) {
        if self.sketch.take().is_none() {
            self.player.pause();
            self.sketch = Some(Sketch::default());
        }
        cx.notify();
    }

    /// The saved drawing to show: from a note right where we're paused.
    fn saved_drawing(&self, position: f64, cx: &App) -> Vec<store::Stroke> {
        let Some(g) = self.store.read(cx).games.get(&self.game_key) else { return vec![] };
        g.notes
            .iter()
            .filter(|n| !n.drawing.is_empty() && (n.video_time - position).abs() < 0.75)
            .min_by(|a, b| (a.video_time - position).abs().total_cmp(&(b.video_time - position).abs()))
            .map(|n| n.drawing.clone())
            .unwrap_or_default()
    }

    fn pen(&mut self, at: Point<Pixels>, down: bool, cx: &mut Context<Self>) {
        let pic = draw::picture_rect(self.video_bounds.get(), self.player.frame_size());
        let Some(sk) = &mut self.sketch else { return };
        let p = draw::normalize(at, pic);
        if down {
            sk.pen_down = true;
            sk.strokes.push(store::Stroke { color: sk.color, points: vec![p] });
        } else if sk.pen_down {
            let Some(s) = sk.strokes.last_mut() else { return };
            let last = s.points.last().copied().unwrap_or(p);
            if (p[0] - last[0]).abs() + (p[1] - last[1]).abs() < 0.002 {
                return;
            }
            s.points.push(p);
        } else {
            return;
        }
        cx.notify();
    }

    /// Picture at the timeline hover point. Seeks a separate muted player
    /// (keyframe-fast) when the hover moves, and never shows a stale frame.
    fn hover_thumb(&mut self, window: &mut Window) -> Option<crate::player::Frame> {
        let t = self.hover_t?;
        let preview = self.preview.get_or_insert_with(|| {
            let p = VideoPlayer::open(&self.path);
            p.set_muted(true);
            p
        });
        if !(self.preview_at - t).abs().lt(&0.75) {
            preview.seek_fast(t);
            self.preview_at = t;
        }
        let frame = preview.frame();
        if (preview.frame_time() - t).abs() < 2.0 {
            frame
        } else {
            // Still decoding: check again next frame.
            window.request_animation_frame();
            None
        }
    }

    fn draw_toolbar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sk = self.sketch.as_ref()?;
        let swatches = (0..draw::COLORS as u8).map(|c| {
            div()
                .id(("pen-color", c as usize))
                .size(px(18.))
                .rounded_full()
                .bg(draw::color(c))
                .border_2()
                .border_color(if sk.color == c { theme::text() } else { gpui_kit::transparent_black() })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(sk) = &mut this.sketch {
                        sk.color = c;
                    }
                    cx.notify();
                }))
        });
        let empty = sk.strokes.is_empty();
        Some(
            div()
                .id("draw-toolbar")
                .absolute()
                .top_3()
                .left_3()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1p5()
                .rounded_lg()
                .bg(theme::panel().opacity(0.92))
                .border_1()
                .border_color(theme::line())
                .cursor_default()
                // Clicks here mustn't draw or toggle playback.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(icon(IconName::Pencil, 14., theme::muted()))
                .children(swatches)
                .child(div().w(px(1.)).h(px(18.)).bg(theme::line()))
                .child(ui::icon_button("draw-undo", IconName::Undo2, 14.).tooltip(ui::tip("Undo  ⌘Z")).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(sk) = &mut this.sketch {
                        sk.strokes.pop();
                    }
                    cx.notify();
                })))
                .child(ui::icon_button("draw-clear", IconName::Eraser, 14.).tooltip(ui::tip("Clear")).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(sk) = &mut this.sketch {
                        sk.strokes.clear();
                    }
                    cx.notify();
                })))
                .child(
                    ui::solid_button("draw-note", theme::accent())
                        .text_color(theme::bg())
                        .child(icon(IconName::StickyNote, 13., theme::bg()))
                        .child(if empty { "Note" } else { "Save with note" })
                        .on_click(cx.listener(|this, _, window, cx| this.start_note(window, cx))),
                )
                .child(ui::ghost_button("draw-done").child("Done").tooltip(ui::tip("Stop drawing  D")).on_click(cx.listener(|this, _, _, cx| {
                    this.sketch = None;
                    cx.notify();
                })))
                .when(empty, |d| d.child(div().text_xs().text_color(theme::muted()).child("Drag on the picture to draw")))
                .into_any_element(),
        )
    }

    fn speed_menu(&self, playing: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let rate = self.player.rate();
        // While shuttling (J/L), show the actual rate.
        let label = if playing && rate != self.speed { speed_label(rate) } else { speed_label(self.speed) };
        let this = cx.entity().downgrade();
        let current = self.speed;
        Button::new("speed")
            .ghost()
            .label(label)
            .tooltip("Playback speed  < >")
            .dropdown_menu_with_anchor(gpui_kit::Anchor::BottomLeft, move |mut menu, _, _| {
                for s in SPEEDS {
                    let this = this.clone();
                    menu = menu.item(PopupMenuItem::new(if s == 1.0 { "Normal".to_string() } else { speed_label(s) }).checked(s == current).on_click(
                        move |_, _, cx| {
                            if let Some(r) = this.upgrade() {
                                r.update(cx, |r, cx| r.set_speed(s, cx));
                            }
                        },
                    ));
                }
                menu
            })
    }

    fn start_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.player.pause();
        self.tab = Tab::Moments;
        self.adding_note_at = Some(self.player.position());
        self.note_input.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn finish_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.adding_note_at = None;
        self.note_input.update(cx, |s, cx| s.set_value("", window, cx));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn toggle_saved(&mut self, m: &Moment, cx: &mut Context<Self>) {
        let key = self.game_key.clone();
        self.store.update(cx, |s, cx| {
            s.toggle_saved(&key, m);
            cx.notify();
        });
    }

    fn delete_note(&mut self, id: u64, cx: &mut Context<Self>) {
        let key = self.game_key.clone();
        self.store.update(cx, |s, cx| {
            s.delete_note(&key, id);
            cx.notify();
        });
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Typing in a note field must not drive the player.
        if !self.focus.is_focused(window) {
            return;
        }
        let ks = &ev.keystroke;
        let shift = ks.modifiers.shift;
        if ks.modifiers.platform && ks.key == "z" {
            if let Some(sk) = &mut self.sketch {
                sk.strokes.pop();
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if ks.modifiers.platform || ks.modifiers.control {
            return;
        }
        match ks.key.as_str() {
            "space" => self.player.toggle(),
            "<" => self.nudge_speed(false, cx),
            ">" => self.nudge_speed(true, cx),
            "," if shift => self.nudge_speed(false, cx),
            "." if shift => self.nudge_speed(true, cx),
            "," => self.step_frame(-1),
            "." => self.step_frame(1),
            "j" | "k" | "l" => self.shuttle(ks.key.as_str()),
            "d" => self.toggle_draw(cx),
            "escape" if self.sketch.is_some() => self.sketch = None,
            "left" if ks.modifiers.shift => self.step_moment(false, cx),
            "right" if ks.modifiers.shift => self.step_moment(true, cx),
            "left" => self.user_seek(self.player.position() - 5.0, cx),
            "right" => self.user_seek(self.player.position() + 5.0, cx),
            "n" => self.start_note(window, cx),
            "i" => self.clip_in = Some(self.player.position()),
            "o" => self.clip_out = Some(self.player.position()),
            "escape" => cx.emit(ReviewEvent::Back),
            "=" | "+" => self.zoom_by(0.5),
            "-" => self.zoom_by(2.0),
            "0" => self.view_span = None,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    // -- header ----------------------------------------------------------------

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = &self.meta;
        let champ = m.champion.clone().unwrap_or_else(|| "Manual recording".into());
        let result = match m.result.as_deref() {
            Some("Win") => Some(("Victory", theme::victory())),
            Some("Lose") => Some(("Defeat", theme::defeat())),
            _ => None,
        };
        let portrait = m.player.as_ref().and_then(|p| self.icons.champion(&p.champion_key));
        let opponent = m.opponent.as_ref().map(|o| (o.champion.clone(), self.icons.champion(&o.champion_key)));

        ui::page_header()
            .pl_3()
            .child(
                ui::icon_button("back", IconName::ChevronLeft, 18.)
                    .tooltip(ui::tip("Back"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ReviewEvent::Back))),
            )
            .children(portrait.map(|p| img(p).size(px(28.)).rounded(px(6.))))
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(champ))
            .when(self.store.read(cx).games.get(&self.game_key).is_some_and(|g| g.pinned), |d| {
                d.child(icon(IconName::Pin, 13., theme::kill()))
            })
            .children(opponent.map(|(name, icon)| {
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_color(theme::muted())
                    .child("vs")
                    .children(icon.map(|p| img(p).size(px(22.)).rounded(px(5.))))
                    .child(name)
            }))
            .children(result.map(|(label, color)| div().text_color(color).font_weight(FontWeight::MEDIUM).child(label)))
            .child(div().text_color(theme::muted()).truncate().child(crate::games::queue_label(m)))
            .child(ui::drag_area())
            .children(m.player.as_ref().map(|p| {
                div()
                    .flex()
                    .flex_none()
                    .gap_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(p.kills.to_string())
                    .child(div().text_color(theme::muted()).child("/"))
                    .child(div().text_color(theme::death()).child(p.deaths.to_string()))
                    .child(div().text_color(theme::muted()).child("/"))
                    .child(p.assists.to_string())
                    .child(div().ml_2().font_weight(FontWeight::NORMAL).text_color(theme::muted()).child(format!("{} CS", p.cs)))
            }))
            .when(self.confirm_condense, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .child(div().text_sm().text_color(theme::muted()).child("Delete everything but the moments?"))
                        .child(
                            ui::solid_button("condense-yes", theme::kill())
                                .text_color(theme::bg())
                                .child("Keep moments only")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.player.pause();
                                    cx.emit(ReviewEvent::Condense)
                                })),
                        )
                        .child(ui::ghost_button("condense-no").child("Cancel").on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_condense = false;
                            cx.notify();
                        }))),
                )
            })
            .when(!self.confirm_condense, |d| {
                let open = self.layout.review_panel_open;
                d.child(
                    ui::icon_button("toggle-panel", if open { IconName::PanelRightClose } else { IconName::PanelRightOpen }, 16.)
                        .tooltip(ui::tip(if open { "Hide side panel" } else { "Show moments, clips and notes" }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.layout.review_panel_open = !this.layout.review_panel_open;
                            cx.emit(ReviewEvent::Layout(this.layout));
                            cx.notify();
                        })),
                )
                .child(self.more_menu(cx))
            })
    }

    /// Everything that isn't watching or note-taking lives behind "⋯".
    fn more_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pinned = self.store.read(cx).games.get(&self.game_key).is_some_and(|g| g.pinned);
        let condensed = self.meta.condensed.is_some();
        let this = cx.entity().downgrade();
        let path = self.path.clone();
        Button::new("review-more").ghost().icon(Icon::new(IconName::Ellipsis)).tooltip("More").dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, _| {
            let (t1, t2, t3) = (this.clone(), this.clone(), this.clone());
            let path = path.clone();
            let mut menu = menu.item(
                PopupMenuItem::new(if pinned { "Stop keeping" } else { "Keep forever" })
                    .icon(Icon::new(IconName::Pin))
                    .on_click(move |_, _, cx| {
                        if let Some(r) = t1.upgrade() {
                            r.update(cx, |r, cx| r.toggle_keep(cx));
                        }
                    }),
            );
            if !condensed {
                menu = menu.item(PopupMenuItem::new("Keep moments only…").icon(Icon::new(IconName::Scissors)).on_click(move |_, _, cx| {
                    if let Some(r) = t2.upgrade() {
                        r.update(cx, |r, cx| {
                            r.confirm_condense = true;
                            cx.notify();
                        });
                    }
                }));
            }
            menu.item(PopupMenuItem::new("Show in Finder").icon(Icon::new(IconName::FolderOpen)).on_click(move |_, _, _| {
                let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn();
            }))
            .separator()
            .item(PopupMenuItem::new("Move game to Trash").icon(Icon::new(IconName::Trash)).on_click(move |_, _, cx| {
                if let Some(r) = t3.upgrade() {
                    r.update(cx, |r, cx| {
                        r.player.pause();
                        cx.emit(ReviewEvent::Delete)
                    });
                }
            }))
        })
    }

    // -- side panel --------------------------------------------------------------

    fn tab_button(&self, tab: Tab, label: String, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.tab == tab;
        div()
            .id(SharedString::from(format!("tab-{}", tab as u8)))
            .flex_1()
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .text_color(if active { theme::text() } else { theme::muted() })
            .when(active, |d| d.bg(theme::raised()))
            .when(!active, |d| d.hover(|s| s.text_color(theme::text())))
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
    }

    fn moments_tab(&self, position: f64, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let all = self.all_moments(cx);
        let count = |f: MomentFilter| all.iter().filter(|m| f.matches(m)).count();
        let moments: Vec<&Moment> = all.iter().filter(|m| self.filter.matches(m)).collect();
        // The moment on screen: last one whose lead-in has started, within 8s after it.
        let current = moments
            .iter()
            .rposition(|m| m.video_time - LEAD_IN <= position + 0.25)
            .filter(|&i| position - moments[i].video_time < 8.0);

        let entity = cx.entity().downgrade();
        let chips = MomentFilter::CHIPS.map(|f| {
            let entity = entity.clone();
            ui::icon_chip(("review-filter", f as usize), f, self.filter, count(f), move |picked, _, cx| {
                if let Some(this) = entity.upgrade() {
                    this.update(cx, |this, cx| {
                        this.filter = picked;
                        cx.notify();
                    });
                }
            })
        });

        let add_note = match self.adding_note_at {
            Some(t) => div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded_lg()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::line())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .text_color(theme::muted())
                        .child(icon(IconName::StickyNote, 13., theme::muted()))
                        .child(format!("Note at {}", theme::fmt_clock(t)))
                        .child(div().flex_1())
                        .child(
                            ui::ghost_button("cancel-note")
                                .child("Cancel")
                                .on_click(cx.listener(|this, _, window, cx| this.finish_note(window, cx))),
                        ),
                )
                .child(Input::new(&self.note_input))
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" {
                        this.finish_note(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .into_any_element(),
            // The toolbar under the video has the Note button.
            None => div().into_any_element(),
        };
        let _ = window;

        let drawn: BTreeSet<u64> = self
            .store
            .read(cx)
            .games
            .get(&self.game_key)
            .map(|g| g.notes.iter().filter(|n| !n.drawing.is_empty()).map(|n| n.id).collect())
            .unwrap_or_default();
        let rows = moments
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let has_drawing = m.note_id.is_some_and(|id| drawn.contains(&id));
                let t = m.video_time;
                let star_m = (*m).clone();
                let note_id = m.note_id;
                let is_note = m.kind == MomentKind::Note;
                div()
                    .id(("moment", i))
                    .group("moment-row")
                    .flex()
                    .items_start()
                    .gap_3()
                    .py_2()
                    .pl_3()
                    .pr_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(Some(i) == current, |d| d.bg(theme::raised()))
                    .hover(|s| s.bg(theme::raised()))
                    .child(div().pt(px(3.)).child(icon(ui::moment_icon(m.kind), 14., theme::moment(m.kind))))
                    .child(div().w(px(40.)).flex_none().pt(px(1.)).text_sm().text_color(theme::muted()).child(theme::fmt_clock(m.clock())))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(!is_note, |d| d.truncate().child(m.label.clone()))
                            .when(is_note, |d| d.child(ui::markdown(("moment-md", i), m.label.clone()))),
                    )
                    .when(has_drawing, |d| {
                        d.child(div().pt(px(3.)).flex_none().child(icon(IconName::Pencil, 12., theme::muted())))
                    })
                    // Actions appear on hover; a saved moment keeps its star visible.
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .invisible()
                            .group_hover("moment-row", |s| s.visible())
                            .when(self.clip_for(m.video_time).is_none(), |d| {
                                let clip_m = (*m).clone();
                                d.child(
                                    ui::icon_button(("clip", i), IconName::Scissors, 13.)
                                        .tooltip(ui::tip("Save as clip"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.export_moment(&clip_m, cx)
                                        })),
                                )
                            })
                            .children(note_id.map(|id| {
                                ui::icon_button(("delete", i), IconName::X, 12.).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.delete_note(id, cx)
                                }))
                            })),
                    )
                    .children(self.clip_for(m.video_time).map(|path| {
                        div()
                            .id(("watch-clip", i))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_1()
                            .h(px(24.))
                            .px_1p5()
                            .rounded_md()
                            .bg(theme::victory().opacity(0.15))
                            .text_xs()
                            .text_color(theme::victory())
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::victory().opacity(0.3)))
                            .child(icon(IconName::Play, 10., theme::victory()))
                            .child("Clip")
                            .tooltip(ui::tip("Watch the clip"))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(ReviewEvent::OpenClip(path.clone()))
                            }))
                    }))
                    .child(
                        div()
                            .flex_none()
                            .when(!m.saved, |d| d.invisible().group_hover("moment-row", |s| s.visible()))
                            .child(ui::star_button(("star", i), m.saved).on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_saved(&star_m, cx)
                            }))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if has_drawing {
                            // Stop on the exact frame so the drawing shows.
                            this.player.pause();
                            this.user_seek(t, cx);
                            cx.notify();
                        } else {
                            this.jump(t, cx)
                        }
                    }))
            })
            .collect::<Vec<_>>();

        let empty = moments.is_empty().then(|| {
            div().px_3().py_6().text_sm().text_color(theme::muted()).child(match self.filter {
                MomentFilter::Saved => "Star a moment to save it. Saved moments collect under their matchup so you can study them later.",
                MomentFilter::Notes => "Pause on something worth remembering and press N to write a note at that moment.",
                _ if all.is_empty() => "No moments yet. Moments come from League's live game data; add your own with N.",
                _ => "No moments of this kind.",
            })
        });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(div().flex().gap_1().px_3().pt_3().children(chips))
            .child(div().p_4().pb_2().flex().gap_2().child(div().flex_1().child(add_note)).when(
                self.adding_note_at.is_none() && moments.len() > 1,
                |d| {
                    let times: Vec<f64> = moments.iter().map(|m| m.video_time).collect();
                    let title = match self.filter {
                        MomentFilter::All => "All moments".to_string(),
                        f => f.label().to_string(),
                    };
                    d.child(
                        ui::solid_button("play-all", theme::raised())
                            .flex_none()
                            .child(icon(IconName::ListVideo, 14., theme::text()))
                            .child("Play all")
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ReviewEvent::PlayAll(times.clone(), title.clone())))),
                    )
                },
            ))
            .child(div().id("moments").flex_1().overflow_y_scroll().px_2().pb_2().children(rows).children(empty))
            .into_any_element()
    }

    fn edit_note(&mut self, field: NoteField, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = Some(field);
        let editor = match field {
            NoteField::Game => Some(self.game_notes.clone()),
            NoteField::Matchup => self.matchup_notes.clone(),
        };
        // Edit what's saved now: the same notes can change on other screens.
        let saved = {
            let store = self.store.read(cx);
            match field {
                NoteField::Game => store.games.get(&self.game_key).map(|g| g.note.clone()),
                NoteField::Matchup => self.matchup.as_ref().and_then(|m| store.matchup(&m.key)).map(|m| m.notes.clone()),
            }
            .unwrap_or_default()
        };
        if let Some(e) = editor {
            e.update(cx, |s, cx| {
                if s.value() != saved.as_str() {
                    s.set_value(saved, window, cx);
                }
                s.focus(window, cx)
            });
        }
        cx.notify();
    }

    fn done_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn notes_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let section = |title: String, hint: Option<String>| {
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .children(hint.map(|h| div().text_sm().text_color(theme::muted()).child(h)))
        };
        let (game_text, matchup_text) = {
            let s = self.store.read(cx);
            (
                s.games.get(&self.game_key).map(|g| g.note.clone()).unwrap_or_default(),
                self.matchup.as_ref().and_then(|m| s.matchup(&m.key)).map(|m| m.notes.clone()).unwrap_or_default(),
            )
        };
        let this = cx.entity().downgrade();
        let edit = |field: NoteField| {
            let this = this.clone();
            move |window: &mut Window, cx: &mut App| {
                if let Some(r) = this.upgrade() {
                    r.update(cx, |r, cx| r.edit_note(field, window, cx));
                }
            }
        };
        let done = || {
            let this = this.clone();
            move |window: &mut Window, cx: &mut App| {
                if let Some(r) = this.upgrade() {
                    r.update(cx, |r, cx| r.done_editing(window, cx));
                }
            }
        };

        div()
            .id("notes")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(section("This game".into(), None))
            .child(ui::note_card(
                "game-note",
                &game_text,
                "What went well, what to fix next game…",
                self.editing == Some(NoteField::Game),
                Textarea::new(&self.game_notes).into_any_element(),
                edit(NoteField::Game),
                done(),
            ))
            .children(self.matchup.as_ref().zip(self.matchup_notes.as_ref()).map(|(m, editor)| {
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .pt_4()
                    .child(section(
                        m.label.clone(),
                        Some(match m.games {
                            1 => "Matchup notes. They show up again the next time you get this lane.".into(),
                            n => format!("Matchup notes, shared across your {n} games in this lane."),
                        }),
                    ))
                    .child(ui::note_card(
                        "matchup-note",
                        &matchup_text,
                        "Power spikes, trades to take or avoid, builds that worked…",
                        self.editing == Some(NoteField::Matchup),
                        Textarea::new(editor).into_any_element(),
                        edit(NoteField::Matchup),
                        done(),
                    ))
            }))
            .when(self.matchup.is_none(), |d| {
                d.child(div().pt_2().text_sm().text_color(theme::muted()).child(
                    "Matchup notes appear for games with a lane opponent. Right-click the game in Games and choose My role if it's missing.",
                ))
            })
            .into_any_element()
    }

    fn side_panel(&self, position: f64, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.tab {
            Tab::Moments => self.moments_tab(position, window, cx),
            Tab::Notes => self.notes_tab(cx),
            Tab::Clips => self.clips_tab(cx),
        };
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(self.layout.review_panel_width))
            .h_full()
            .border_l_1()
            .border_color(if self.resizing { theme::victory() } else { theme::line() })
            // Drag the left edge to resize.
            .child(
                div()
                    .id("panel-resize")
                    .absolute()
                    .left(px(-4.))
                    .top_0()
                    .w(px(8.))
                    .h_full()
                    .cursor_col_resize()
                    .hover(|s| s.bg(theme::victory().opacity(0.25)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.resizing = true;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .m_3()
                    .mb_0()
                    .p(px(3.))
                    .rounded_lg()
                    .bg(theme::panel())
                    .child(self.tab_button(Tab::Moments, "Moments".into(), cx))
                    .child(self.tab_button(Tab::Clips, if self.clips.is_empty() { "Clips".into() } else { format!("Clips {}", self.clips.len()) }, cx))
                    .child(self.tab_button(Tab::Notes, "Notes".into(), cx)),
            )
            .child(body)
    }
}

impl Render for Review {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.player.frame();
        let playing = self.player.is_playing();
        if playing || Instant::now() < self.wake_until {
            window.request_animation_frame();
        }
        let position = self.player.position();
        let duration = self.duration();
        let filter = self.filter;
        let moments = self.all_moments(cx);
        if let Some(until) = self.play_until.filter(|_| Instant::now() >= self.item_guard) {
            if position >= until || (!playing && position >= duration - 0.5) {
                self.play_until = None;
                cx.emit(ReviewEvent::ItemEnded);
            }
        }
        let clip_range = match (self.clip_in, self.clip_out) {
            (Some(a), Some(b)) => Some((a.min(b), a.max(b))),
            (Some(a), None) => Some((a, a.max(position))),
            _ => None,
        };

        let drawing: Vec<store::Stroke> = match &self.sketch {
            Some(sk) => sk.strokes.clone(),
            None if !playing => self.saved_drawing(position, cx),
            None => vec![],
        };
        let frame_size = self.player.frame_size();
        let bounds_cell = self.video_bounds.clone();
        let sketching = self.sketch.is_some();
        let video = div()
            .id("video")
            .flex_1()
            .min_h(px(200.))
            .rounded_lg()
            .overflow_hidden()
            .bg(gpui_kit::black())
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .relative()
            .children(frame.map(|f| surface(f).size_full()))
            // Records where the video is, and paints any drawing on top.
            .child(
                canvas(
                    move |b, _, _| bounds_cell.set(b),
                    move |b, _, window, _| draw::paint(&drawing, draw::picture_rect(b, frame_size), window),
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .when(sketching, |d| {
                d.cursor(CursorStyle::Crosshair)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        window.focus(&this.focus, cx);
                        this.pen(ev.position, true, cx)
                    }))
                    .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                        if ev.pressed_button != Some(MouseButton::Left) {
                            if let Some(sk) = &mut this.sketch {
                                sk.pen_down = false;
                            }
                            return;
                        }
                        this.pen(ev.position, false, cx)
                    }))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| {
                        if let Some(sk) = &mut this.sketch {
                            sk.pen_down = false;
                        }
                    }))
            })
            .children(self.draw_toolbar(cx))
            .when(!playing && !sketching, |d| {
                d.child(
                    div()
                        .absolute()
                        .size(px(64.))
                        .rounded_full()
                        .bg(gpui_kit::black().opacity(0.55))
                        .border_1()
                        .border_color(gpui_kit::white().opacity(0.25))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(IconName::Play, 26., gpui_kit::white())),
                )
            })
            .on_click(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus, cx);
                if this.sketch.is_none() {
                    this.toggle(cx)
                }
            }));

        let thumb = self.hover_thumb(window).map(|f| surface(f).size_full().into_any_element());
        let entity = cx.entity().downgrade();
        let entity2 = entity.clone();
        let view = self.timeline_view(position, duration, playing);
        let entity3 = entity.clone();
        let scrub = timeline::scrubber(
            &moments,
            move |m| filter.matches(m),
            duration,
            position,
            clip_range,
            self.hover_t,
            thumb,
            view,
            move |t, _, cx| {
                if let Some(this) = entity.upgrade() {
                    this.update(cx, |this, cx| {
                        this.user_seek(t, cx);
                        cx.notify();
                    });
                }
            },
            move |t, _, cx| {
                if let Some(this) = entity2.upgrade() {
                    this.update(cx, |this, cx| {
                        if this.hover_t != t {
                            this.hover_t = t;
                            cx.notify();
                        }
                    });
                }
            },
            move |start, end, _, cx| {
                if let Some(this) = entity3.upgrade() {
                    this.update(cx, |this, cx| {
                        let duration = this.duration();
                        if end - start >= duration - 0.5 {
                            this.view_span = None;
                        } else {
                            this.view_span = Some(end - start);
                            this.view_start = start;
                        }
                        cx.notify();
                    });
                }
            },
        );
        let zoomed = self.view_span.is_some();

        // Wraps onto two lines when the window (or the video column) is narrow.
        let controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .gap_y_2()
            .child(
                ui::icon_button("prev", IconName::SkipBack, 16.)
                    .tooltip(ui::tip("Previous moment"))
                    .on_click(cx.listener(|this, _, _, cx| this.step_moment(false, cx))),
            )
            .child(
                div()
                    .id("playpause")
                    .size(px(36.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme::text())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.88))
                    .child(icon(if playing { IconName::Pause } else { IconName::Play }, 16., theme::bg()))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle(cx))),
            )
            .child(
                ui::icon_button("next", IconName::SkipForward, 16.)
                    .tooltip(ui::tip("Next moment"))
                    .on_click(cx.listener(|this, _, _, cx| this.step_moment(true, cx))),
            )
            .child(
                div()
                    .ml_3()
                    .flex()
                    .flex_none()
                    .gap_1p5()
                    .text_sm()
                    .child(theme::fmt_clock(position))
                    .child(div().text_color(theme::muted()).child(format!("of {}", theme::fmt_clock(duration)))),
            )
            .child(div().ml_2().flex_none().child(self.speed_menu(playing, cx)))
            // Timeline zoom (also ⌘/Ctrl-scroll on the timeline, and - = 0).
            .child(
                div()
                    .ml_3()
                    .flex()
                    .flex_none()
                    .items_center()
                    .child(
                        ui::icon_button("zoom-out", IconName::ZoomOut, 14.)
                            .tooltip(ui::tip("Zoom out  -"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom_by(2.0);
                                cx.notify();
                            })),
                    )
                    .child(
                        ui::icon_button("zoom-in", IconName::ZoomIn, 14.)
                            .tooltip(ui::tip("Zoom in  =  (or ⌘-scroll on the timeline)"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zoom_by(0.5);
                                cx.notify();
                            })),
                    )
                    .when(zoomed, |d| {
                        d.child(ui::ghost_button("zoom-fit").child("Fit").tooltip(ui::tip("Show the whole game  0")).on_click(cx.listener(|this, _, _, cx| {
                            this.view_span = None;
                            cx.notify();
                        })))
                    }),
            )
            .child(div().flex_1())
            .child(self.tool_buttons(position, cx))
            .child(shortcuts_button());



        div()
            .id("review")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .when(self.resizing, |d| d.cursor_col_resize())
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                if this.resizing {
                    // The panel is flush with the window's right edge.
                    let total = f32::from(window.viewport_size().width);
                    let w = (total - f32::from(ev.position.x)).clamp(300.0, (total * 0.6).max(300.0));
                    if (w - this.layout.review_panel_width).abs() >= 1.0 {
                        this.layout.review_panel_width = w;
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.resizing {
                        this.resizing = false;
                        cx.emit(ReviewEvent::Layout(this.layout));
                        cx.notify();
                    }
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .child(self.header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_3()
                            .p_5()
                            .child(video)
                            .child(scrub)
                            .child(controls),
                    )
                    .when(self.layout.review_panel_open, |d| d.child(self.side_panel(position, window, cx))),
            )
    }
}

/// One quiet keyboard button instead of a row of hints.
fn shortcuts_button() -> impl IntoElement {
    use gpui_kit::component::popover::Popover;
    let rows: [(&[&'static str], &'static str); 11] = [
        (&["Space"], "Play or pause"),
        (&["J", "K", "L"], "Back, pause, faster"),
        (&[",", "."], "One frame back or forward"),
        (&["<", ">"], "Slower or faster"),
        (&["D"], "Draw on the frame"),
        (&["←", "→"], "Skip 5 seconds"),
        (&["⇧←", "⇧→"], "Previous or next moment"),
        (&["N"], "Add a note here"),
        (&["I", "O"], "Mark a clip's start and end"),
        (&["F8"], "Bookmark during a game"),
        (&["Esc"], "Back"),
    ];
    Popover::new("shortcuts")
        .anchor(gpui_kit::Anchor::BottomRight)
        .trigger(Button::new("shortcuts-btn").ghost().icon(Icon::new(IconName::Keyboard)).tooltip("Keyboard shortcuts"))
        .content(move |_, _, _| {
            div().w(px(280.)).flex().flex_col().gap_2().p_3().children(rows.iter().map(|(keys, what)| {
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().text_sm().child(*what))
                    .child(div().flex().gap_1().children(keys.iter().map(|k| ui::kbd(k))))
            }))
        })
}
