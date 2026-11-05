//! Clips: everything you exported, as a thumbnail grid, plus a small player
//! with copy / show / open-in-game / rename / delete. Clips are independent
//! files and storage cleanup never touches them.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::clip::{self, Clip};
use crate::icons::Icons;
use crate::player::VideoPlayer;
use crate::ui::{self, icon};
use crate::{TimewarpApp, theme, timeline};

pub enum ClipEvent {
    Back,
    /// File changed on disk (renamed or deleted): reload the list.
    Changed,
    /// Open the source game at this source-video time.
    OpenSource(String, f64),
    Trash(PathBuf),
    Toast(String),
}

pub struct ClipViewer {
    clip: Clip,
    player: VideoPlayer,
    focus: FocusHandle,
    icons: Arc<Icons>,
    wake_until: Instant,
    source_available: bool,
    renaming: bool,
    name_input: Entity<InputState>,
    confirm_delete: bool,
    status: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ClipEvent> for ClipViewer {}

impl ClipViewer {
    pub fn new(clip: Clip, source_available: bool, icons: Arc<Icons>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let player = VideoPlayer::open(&clip.path);
        player.play();
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let title = clip.title();
        let name_input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        let subs = vec![cx.subscribe_in(&name_input, window, |this, input, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                let name = input.read(cx).value().to_string();
                this.finish_rename(Some(name), window, cx);
            }
        })];
        Self {
            clip,
            player,
            focus,
            icons,
            wake_until: Instant::now() + Duration::from_millis(600),
            source_available,
            renaming: false,
            name_input,
            confirm_delete: false,
            status: None,
            _subs: subs,
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.clip.path
    }

    fn seek(&mut self, t: f64) {
        self.player.seek(t);
        self.wake_until = Instant::now() + Duration::from_millis(600);
    }

    fn finish_rename(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = false;
        if let Some(name) = name.filter(|n| n.trim() != self.clip.title()) {
            match clip::rename(&self.clip, &name) {
                Ok(path) => {
                    // The player keeps its open file handle across the rename.
                    self.clip.path = path;
                    self.clip.meta.title = name.trim().to_string();
                    cx.emit(ClipEvent::Changed);
                }
                Err(e) => self.status = Some(e),
            }
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        match ev.keystroke.key.as_str() {
            "space" => self.player.toggle(),
            "left" => self.seek(self.player.position() - 2.0),
            "right" => self.seek(self.player.position() + 2.0),
            "escape" => cx.emit(ClipEvent::Back),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let m = &self.clip.meta;
        let portrait = m.champion_key.as_deref().and_then(|k| self.icons.champion(k));
        let title: AnyElement = if self.renaming {
            div()
                .w(px(320.))
                .child(Input::new(&self.name_input))
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key == "escape" {
                        this.finish_rename(None, window, cx);
                        cx.stop_propagation();
                    }
                }))
                .into_any_element()
        } else {
            div().text_lg().font_weight(FontWeight::SEMIBOLD).truncate().child(self.clip.title()).into_any_element()
        };

        ui::page_header()
            .pl_3()
            .child(ui::icon_button("clip-back", IconName::ChevronLeft, 18.).on_click(cx.listener(|_, _, _, cx| cx.emit(ClipEvent::Back))))
            .children(portrait.map(|p| img(p).flex_none().size(px(28.)).rounded(px(6.))))
            .child(title)
            .children(m.opponent.as_ref().map(|o| div().flex_none().text_color(theme::muted()).child(format!("vs {o}"))))
            .child(ui::drag_area())
            .when(self.confirm_delete, |d| {
                d.child(div().text_sm().text_color(theme::muted()).child("Move this clip to the Trash?"))
                    .child(
                        ui::solid_button("clip-delete-yes", theme::death())
                            .text_color(theme::text())
                            .child("Move to Trash")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.player.pause();
                                cx.emit(ClipEvent::Trash(this.clip.path.clone()));
                            })),
                    )
                    .child(ui::ghost_button("clip-delete-no").child("Cancel").on_click(cx.listener(|this, _, _, cx| {
                        this.confirm_delete = false;
                        cx.notify();
                    })))
            })
            .when(!self.confirm_delete, |d| {
                d.child(
                    ui::ghost_button("clip-copy")
                        .flex_none()
                        .child(icon(IconName::ClipboardCheck, 14., theme::muted()))
                        .child("Copy")
                        .on_click(cx.listener(|this, _, _, cx| {
                            clip::copy_file_to_clipboard(&this.clip.path);
                            cx.emit(ClipEvent::Toast("Copied. Paste it into Discord.".into()));
                        })),
                )
                .when(self.source_available, |d| {
                    d.child(
                        ui::ghost_button("clip-source")
                            .flex_none()
                            .child(icon(IconName::Gamepad2, 14., theme::muted()))
                            .child("Open in game")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.player.pause();
                                let m = &this.clip.meta;
                                cx.emit(ClipEvent::OpenSource(m.source.clone(), m.start));
                            })),
                    )
                })
                .child(
                    ui::ghost_button("clip-rename")
                        .flex_none()
                        .child("Rename")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.renaming = true;
                            this.name_input.update(cx, |s, cx| s.focus(window, cx));
                            cx.notify();
                        })),
                )
                .child(
                    ui::ghost_button("clip-reveal")
                        .flex_none()
                        .child(icon(IconName::FolderOpen, 14., theme::muted()))
                        .child("Show in Finder")
                        .on_click(cx.listener(|this, _, _, _| {
                            let _ = std::process::Command::new("open").arg("-R").arg(&this.clip.path).spawn();
                        })),
                )
                .child(ui::icon_button("clip-delete", IconName::X, 14.).on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_delete = true;
                    cx.notify();
                })))
            })
    }
}

impl Render for ClipViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.player.frame();
        let playing = self.player.is_playing();
        if playing || Instant::now() < self.wake_until {
            window.request_animation_frame();
        }
        let position = self.player.position();
        let duration = self.player.duration().unwrap_or(self.clip.duration());

        let entity = cx.entity().downgrade();
        let scrub = timeline::scrubber(
            &[],
            |_| false,
            duration,
            position,
            None,
            None,
            None,
            (0.0, duration),
            move |t, _, cx| {
                if let Some(this) = entity.upgrade() {
                    this.update(cx, |this, cx| {
                        this.seek(t);
                        cx.notify();
                    });
                }
            },
            |_, _, _| {},
            |_, _, _, _| {},
        );

        let m = &self.clip.meta;
        let details = [
            m.result.as_deref().map(|r| if r == "Win" { "Victory".to_string() } else { "Defeat".to_string() }),
            m.queue.clone(),
            m.game_time.map(|t| format!("At {} in game", theme::fmt_clock(t))),
            m.created_at.map(|d| format!("Clipped {}", d.format("%b %-d, %H:%M"))),
            Some(ui::human_bytes(self.clip.bytes)),
        ];

        div()
            .id("clip-viewer")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .size_full()
            .flex()
            .flex_col()
            .child(self.header(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .p_5()
                    .child(
                        div()
                            .id("clip-video")
                            .flex_1()
                            .min_h(px(200.))
                            .rounded_lg()
                            .overflow_hidden()
                            .bg(gpui_kit::black())
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .children(frame.map(|f| surface(f).size_full()))
                            .on_click(cx.listener(|this, _, window, cx| {
                                window.focus(&this.focus, cx);
                                this.player.toggle();
                                cx.notify();
                            })),
                    )
                    .child(scrub)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .id("clip-play")
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
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.player.toggle();
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .gap_1p5()
                                    .text_sm()
                                    .child(theme::fmt_clock(position))
                                    .child(div().text_color(theme::muted()).child(format!("of {}", theme::fmt_clock(duration)))),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(theme::muted())
                                    .child(details.into_iter().flatten().collect::<Vec<_>>().join("   ")),
                            )
                            .children(self.status.clone().map(|s| div().flex_none().text_sm().text_color(theme::muted()).child(s))),
                    ),
            )
    }
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

impl TimewarpApp {
    pub fn reload_clips(&mut self) {
        self.clips = clip::list(&self.config.output_dir);
        // Thumbnails for clips that don't have one yet (older exports).
        let missing: Vec<PathBuf> = self
            .clips
            .iter()
            .filter(|c| c.thumb.is_none() && !self.thumbs_requested.contains(&c.path))
            .map(|c| c.path.clone())
            .collect();
        if !missing.is_empty() {
            self.thumbs_requested.extend(missing.iter().cloned());
            std::thread::spawn(move || missing.iter().for_each(|p| clip::make_thumbnail(p)));
        }
    }

    /// Pick up thumbnails that finished rendering in the background.
    pub fn refresh_thumbs(&mut self) -> bool {
        let mut changed = false;
        for c in self.clips.iter_mut().filter(|c| c.thumb.is_none()) {
            let t = clip::thumb_path(&c.path);
            if t.exists() {
                c.thumb = Some(t);
                changed = true;
            }
        }
        changed
    }

    pub fn trash_clip(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let title = self.clips.iter().find(|c| c.path == path).map(|c| c.title());
        let moved = crate::deleted::trash(crate::deleted::Kind::Clip, title, "", &[path.to_path_buf(), path.with_extension("json")]);
        self.reload_clips();
        if moved.is_empty() {
            self.toast(IconName::X, "Couldn't move that clip to the Trash.", None, cx);
        } else {
            self.toast(IconName::Trash, "Clip moved to the Trash.", Some(ui::ToastAction::Undo(moved)), cx);
        }
    }

    /// Open a clip by file, e.g. straight after saving it. Whatever was on
    /// screen (a review, say) stays underneath and comes back on Back.
    pub fn open_clip_path(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        if !self.clips.iter().any(|c| c.path == path) {
            self.reload_clips();
        }
        if let Some(i) = self.clips.iter().position(|c| c.path == path) {
            if let Some(r) = &self.review {
                r.update(cx, |r, _| r.pause());
            }
            self.open_clip(i, window, cx);
        }
    }

    pub fn open_clip(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_clip_at(idx, true, window, cx);
    }

    pub fn open_clip_at(&mut self, idx: usize, record: bool, window: &mut Window, cx: &mut Context<Self>) {
        if record {
            self.record_history(cx);
        }
        let Some(c) = self.clips.get(idx) else { return };
        let clip = Clip { path: c.path.clone(), meta: c.meta.clone(), bytes: c.bytes, thumb: c.thumb.clone() };
        let source_available = self.games.iter().any(|g| crate::store::game_key(g) == clip.meta.source);
        let icons = self.icons.clone();
        let viewer = cx.new(|cx| ClipViewer::new(clip, source_available, icons, window, cx));
        cx.subscribe_in(&viewer, window, |this, _, ev: &ClipEvent, window, cx| match ev {
            ClipEvent::Back => {
                if this.history.is_empty() {
                    this.clip_view = None;
                    cx.notify();
                } else {
                    this.go_back(window, cx);
                }
            }
            ClipEvent::Changed => {
                this.reload_clips();
                cx.notify();
            }
            ClipEvent::Trash(path) => {
                this.clip_view = None;
                this.trash_clip(path, cx);
            }
            ClipEvent::Toast(text) => this.toast(IconName::ClipboardCheck, text.clone(), None, cx),
            ClipEvent::OpenSource(source, t) => {
                if let Some(i) = this.games.iter().position(|g| &crate::store::game_key(g) == source) {
                    let t = *t;
                    this.clip_view = None;
                    this.open_review(i, None, window, cx);
                    if let Some(r) = &this.review {
                        r.update(cx, |r, cx| r.seek_to(t, cx));
                    }
                }
            }
        })
        .detach();
        self.clip_view = Some(viewer);
        cx.notify();
    }

    pub fn clip_card(&self, idx: usize, c: &Clip, cx: &mut Context<Self>) -> AnyElement {
        let color = match (c.meta.kind, c.meta.note) {
            (Some(k), _) => theme::marker(k),
            (None, true) => theme::text(),
            _ => theme::victory(),
        };
        let sub = match (&c.meta.champion, &c.meta.opponent) {
            (Some(me), Some(vs)) => format!("{me} vs {vs}"),
            (Some(me), None) => me.clone(),
            _ => String::new(),
        };
        let date = c.meta.created_at.map(|d| d.format("%b %-d").to_string()).unwrap_or_default();
        let selected = self.selected_clips.contains(&c.path);
        let selecting = !self.selected_clips.is_empty();
        let focused = self.list_cursor.as_ref() == Some(&c.path);
        let path = c.path.clone();
        div()
            .id(("clip-card", idx))
            .group("clip-card")
            .relative()
            .child(self.bounds_probe(c.path.clone()))
            .w(px(272.))
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded_lg()
            .cursor_pointer()
            .border_1()
            .border_color(if focused {
                theme::accent()
            } else if selected {
                theme::victory().opacity(0.6)
            } else {
                gpui_kit::transparent_black()
            })
            .when(selected, |d| d.bg(theme::raised()))
            .when(!selected, |d| d.hover(|s| s.bg(theme::panel())))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(144.))
                    .rounded_md()
                    .overflow_hidden()
                    .bg(theme::panel())
                    .border_1()
                    .border_color(theme::line())
                    .child(match &c.thumb {
                        Some(t) => img(t.clone()).size_full().object_fit(ObjectFit::Cover).into_any_element(),
                        None => div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(IconName::Scissors, 20., theme::muted()))
                            .into_any_element(),
                    })
                    .child(div().absolute().top_2().left_2().size(px(9.)).rounded_full().bg(color).border_1().border_color(theme::bg()))
                    // Selection checkbox: on hover, or always while selecting.
                    .child({
                        let path = path.clone();
                        div()
                            .id(("clip-select", idx))
                            .absolute()
                            .top_1p5()
                            .right_1p5()
                            .size(px(20.))
                            .rounded(px(5.))
                            .border_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_color(if selected { theme::victory() } else { gpui_kit::white().opacity(0.8) })
                            .bg(if selected { theme::victory() } else { gpui_kit::black().opacity(0.45) })
                            .when(!selecting, |d| d.invisible().group_hover("clip-card", |s| s.visible()))
                            .when(selected, |d| d.child(icon(IconName::Check, 13., theme::bg())))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_clip(&path, idx);
                                cx.notify();
                            }))
                    })
                    .child(
                        div()
                            .absolute()
                            .bottom_2()
                            .right_2()
                            .px_1p5()
                            .rounded(px(4.))
                            .bg(gpui_kit::black().opacity(0.7))
                            .text_xs()
                            .child(theme::fmt_clock(c.duration())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .px_0p5()
                    .child(div().font_weight(FontWeight::MEDIUM).truncate().child(c.title()))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .text_sm()
                            .text_color(theme::muted())
                            .child(div().flex_1().truncate().child(sub))
                            .child(date),
                    ),
            )
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                if std::mem::take(&mut this.suppress_click) {
                    return;
                }
                let m = ev.modifiers();
                let path = this.clips[idx].path.clone();
                this.list_cursor = Some(path.clone());
                if m.shift {
                    let anchor = this.clip_anchor.unwrap_or(idx);
                    let (lo, hi) = (anchor.min(idx), anchor.max(idx));
                    let range: Vec<PathBuf> = this.clips[lo..=hi].iter().map(|c| c.path.clone()).collect();
                    this.selected_clips.extend(range);
                    this.clip_anchor = Some(idx);
                } else if m.platform || !this.selected_clips.is_empty() {
                    this.toggle_clip(&path, idx);
                } else {
                    this.open_clip(idx, window, cx);
                }
                cx.notify();
            }))
            .context_menu({
                let app = cx.entity().downgrade();
                let path = c.path.clone();
                move |menu, _, _| {
                    let (p1, p2, p3, app1, app2) = (path.clone(), path.clone(), path.clone(), app.clone(), app.clone());
                    menu.item(PopupMenuItem::new("Copy").icon(Icon::new(IconName::ClipboardCheck)).on_click(move |_, _, cx| {
                        clip::copy_file_to_clipboard(&p1);
                        if let Some(a) = app1.upgrade() {
                            a.update(cx, |a, cx| a.toast(IconName::ClipboardCheck, "Copied. Paste it into Discord.", None, cx));
                        }
                    }))
                    .item(PopupMenuItem::new("Show in Finder").icon(Icon::new(IconName::FolderOpen)).on_click(move |_, _, _| {
                        let _ = std::process::Command::new("open").arg("-R").arg(&p2).spawn();
                    }))
                    .separator()
                    .item(PopupMenuItem::new("Move to Trash").icon(Icon::new(IconName::Trash)).on_click(move |_, _, cx| {
                        if let Some(a) = app2.upgrade() {
                            a.update(cx, |a, cx| a.trash_clip(&p3, cx));
                        }
                    }))
                }
            })
            .into_any_element()
    }

    fn toggle_clip(&mut self, path: &PathBuf, idx: usize) {
        if !self.selected_clips.remove(path) {
            self.selected_clips.insert(path.clone());
        }
        self.clip_anchor = Some(idx);
    }

    /// Several clips to the Trash with one Undo.
    pub fn trash_clips(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let moved: Vec<(PathBuf, PathBuf)> = paths
            .iter()
            .flat_map(|p| {
                let title = self.clips.iter().find(|c| &c.path == p).map(|c| c.title());
                crate::deleted::trash(crate::deleted::Kind::Clip, title, "", &[p.clone(), p.with_extension("json")])
            })
            .collect();
        let n = moved.iter().filter(|(o, _)| o.extension().is_some_and(|e| e == "mp4")).count();
        for p in &paths {
            self.selected_clips.remove(p);
        }
        self.reload_clips();
        self.toast(
            IconName::Trash,
            if n == 1 { "Clip moved to the Trash.".to_string() } else { format!("{n} clips moved to the Trash.") },
            Some(ui::ToastAction::Undo(moved)),
            cx,
        );
    }

    fn clips_selection_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let paths: Vec<PathBuf> = self.selected_clips.iter().cloned().collect();
        let n = paths.len();
        let bytes: u64 = self.clips.iter().filter(|c| self.selected_clips.contains(&c.path)).map(|c| c.bytes).sum();
        ui::page_header()
            .bg(theme::raised().opacity(0.5))
            .child(
                ui::icon_button("clear-clip-selection", IconName::X, 14.)
                    .tooltip(ui::tip("Clear selection (Esc)"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected_clips.clear();
                        cx.notify();
                    })),
            )
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{n} selected")))
            .child(div().text_sm().text_color(theme::muted()).child(ui::human_bytes(bytes)))
            .when(n < self.clips.len(), |d| {
                d.child(ui::ghost_button("select-all-clips").child(format!("Select all {}", self.clips.len())).on_click(cx.listener(|this, _, _, cx| {
                    let all: Vec<PathBuf> = this.clips.iter().map(|c| c.path.clone()).collect();
                    this.selected_clips.extend(all);
                    cx.notify();
                })))
            })
            .child(ui::drag_area())
            .child({
                let p = paths.clone();
                ui::ghost_button("clips-export")
                    .text_color(theme::text())
                    .child(icon(IconName::FolderOutput, 13., theme::text()))
                    .child("Export…")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let parts = crate::transfer::Parts {
                            notes: false,
                            matchups: false,
                            clips: true,
                            recordings: false,
                            settings: false,
                        };
                        this.export_with(parts, vec![], p.clone(), cx);
                    }))
            })
            .child({
                let p = paths;
                ui::solid_button("clips-trash", theme::death())
                    .text_color(theme::text())
                    .child(icon(IconName::Trash, 13., theme::text()))
                    .child("Move to Trash")
                    .tooltip(ui::tip("Delete or Backspace. You can undo."))
                    .on_click(cx.listener(move |this, _, _, cx| this.trash_clips(p.clone(), cx)))
            })
    }

    pub fn clips_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let total: u64 = self.clips.iter().map(|c| c.bytes).sum();
        if !self.selected_clips.is_empty() {
            let header = self.clips_selection_bar(cx).into_any_element();
            return self.clips_body(header, cx).into_any_element();
        }
        let header = ui::page_header()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Clips"))
            .child(ui::drag_area())
            .when(!self.clips.is_empty(), |d| {
                d.child(div().text_sm().text_color(theme::muted()).child(format!("{} clips, {}", self.clips.len(), ui::human_bytes(total))))
            })
            .child(
                ui::ghost_button("clips-folder")
                    .child(icon(IconName::FolderOpen, 14., theme::muted()))
                    .child("Show in Finder")
                    .on_click({
                        let dir = clip::clips_dir(&self.config.output_dir);
                        move |_, _, _| {
                            let _ = std::fs::create_dir_all(&dir);
                            let _ = std::process::Command::new("open").arg(&dir).spawn();
                        }
                    }),
            )
            .into_any_element();
        self.clips_body(header, cx).into_any_element()
    }

    fn clips_body(&self, header: AnyElement, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.clips.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_lg().font_weight(FontWeight::MEDIUM).child("No clips yet"))
                .child(div().max_w(px(440.)).text_center().text_color(theme::muted()).child(
                    "While reviewing a game, press the scissors on any moment, or mark a range with I and O. Clips land here and on your clipboard, and they stay even after the full game is cleaned up.",
                ))
                .into_any_element()
        } else {
            self.item_bounds.borrow_mut().clear();
            let list = div()
                .id("clips-grid")
                .track_scroll(&self.clips_scroll)
                .size_full()
                .overflow_y_scroll()
                .p_4()
                .flex()
                .flex_wrap()
                .content_start()
                .gap_2()
                .children(self.clips.iter().enumerate().map(|(i, c)| self.clip_card(i, c, cx)).collect::<Vec<_>>())
                .into_any_element();
            // Relative box: the area probe sees the visible list, and a press
            // anywhere in it starts a rubber band (or clears on empty space).
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(self.area_probe())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        // Page shortcuts (⌘A/Ctrl+A, Esc, Delete) need focus here.
                        window.focus(&this.focus, cx);
                        this.marquee_down(crate::marquee::SelectTarget::Clips, ev.position, ev.modifiers)
                    }),
                )
                .child(list)
                .into_any_element()
        };

        div().size_full().flex().flex_col().child(header).child(body)
    }
}
