//! Small shared building blocks so every screen speaks the same language.

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use timewarp_league::MarkerKind;

use crate::store::{Moment, MomentKind};

use crate::theme;

// Only the icons we use are embedded (a few KB), on top of gpui-kit's defaults.
gpui_kit::assets::icon_assets!(
    pub AppIcons,
    [
        BookOpen, Bug, Castle, Check, ChevronDown, ChevronLeft, ChevronRight, Circle, ClipboardCheck,
        Copy, Download, Ellipsis, Eraser, ExternalLink, Film, Flag, FolderInput, FolderOpen,
        FolderOutput, Gamepad2, Github, Handshake, HardDrive, House, Info, Keyboard, ListVideo,
        LoaderCircle, Mic, Minimize2, Monitor, MousePointer2, NotebookPen, PanelRightClose,
        PanelRightOpen, Pause, Pencil, Pin, Play, Plus, RefreshCw, Rewind, RotateCw, Scissors,
        Settings, ShieldAlert, ShieldCheck, SkipBack, SkipForward, Skull, Sparkles, Square, Star,
        StickyNote, Swords, Trash, TriangleAlert, Undo2, Volume2, X, ZoomIn, ZoomOut
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = AppIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(AppIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

pub fn icon(name: IconName, size: f32, color: Hsla) -> Icon {
    Icon::new(name).size(px(size)).text_color(color)
}

pub fn moment_icon(kind: MomentKind) -> IconName {
    match kind {
        MomentKind::Event(MarkerKind::Kill | MarkerKind::Multikill | MarkerKind::Ace) => IconName::Swords,
        MomentKind::Event(MarkerKind::Death) => IconName::Skull,
        MomentKind::Event(MarkerKind::Assist) => IconName::Handshake,
        MomentKind::Event(MarkerKind::Bookmark) => IconName::Flag,
        MomentKind::Event(_) => IconName::Castle,
        MomentKind::Note => IconName::StickyNote,
    }
}

/// Quiet text button: muted until hovered.
pub fn ghost_button(id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1p5()
        .h(px(28.))
        .px_2p5()
        .rounded_md()
        .text_sm()
        .text_color(theme::muted())
        .cursor_pointer()
        .hover(|s| s.bg(theme::raised()).text_color(theme::text()))
}

/// Filled button for the one main action on a surface.
pub fn solid_button(id: impl Into<ElementId>, bg: Hsla) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .h(px(32.))
        .px_3()
        .rounded_md()
        .bg(bg)
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|s| s.opacity(0.88))
}

/// Square icon-only button.
pub fn icon_button(id: impl Into<ElementId>, name: IconName, size: f32) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(size + 14.))
        .flex_none()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(theme::raised()))
        .child(icon(name, size, theme::text()))
}

/// A row of mutually exclusive options.
pub fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, &'static str)],
    value: T,
    on_change: impl Fn(T, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_change = std::rc::Rc::new(on_change);
    div()
        .flex()
        .p(px(3.))
        .gap(px(2.))
        .rounded_lg()
        .bg(theme::bg())
        .border_1()
        .border_color(theme::line())
        .children(options.iter().enumerate().map(|(i, &(v, label))| {
            let on_change = on_change.clone();
            let active = v == value;
            div()
                .id((id, i))
                .px_3()
                .h(px(26.))
                .flex()
                .items_center()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if active { theme::text() } else { theme::muted() })
                .when(active, |d| d.bg(theme::raised()))
                .when(!active, |d| d.hover(|s| s.text_color(theme::text())))
                .child(label)
                .on_click(move |_, window, cx| on_change(v, window, cx))
        }))
}

/// Which moments a list or timeline shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MomentFilter {
    All,
    Saved,
    Kills,
    Deaths,
    Assists,
    Objectives,
    Notes,
}

impl MomentFilter {
    pub const CHIPS: [MomentFilter; 6] = [
        MomentFilter::Saved,
        MomentFilter::Kills,
        MomentFilter::Deaths,
        MomentFilter::Assists,
        MomentFilter::Objectives,
        MomentFilter::Notes,
    ];

    pub fn matches(self, m: &Moment) -> bool {
        use MarkerKind::*;
        match (self, m.kind) {
            (MomentFilter::All, _) => true,
            (MomentFilter::Saved, _) => m.saved,
            (MomentFilter::Kills, MomentKind::Event(Kill | Multikill | Ace)) => true,
            (MomentFilter::Deaths, MomentKind::Event(Death)) => true,
            (MomentFilter::Assists, MomentKind::Event(Assist)) => true,
            (MomentFilter::Objectives, MomentKind::Event(Objective | Structure)) => true,
            (MomentFilter::Notes, MomentKind::Note | MomentKind::Event(Bookmark)) => true,
            _ => false,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MomentFilter::All => "All",
            MomentFilter::Saved => "Saved",
            MomentFilter::Kills => "Kills",
            MomentFilter::Deaths => "Deaths",
            MomentFilter::Assists => "Assists",
            MomentFilter::Objectives => "Objectives",
            MomentFilter::Notes => "Notes",
        }
    }

    pub fn color(self) -> Hsla {
        match self {
            MomentFilter::All => theme::muted(),
            MomentFilter::Saved => theme::kill(),
            MomentFilter::Kills => theme::kill(),
            MomentFilter::Deaths => theme::death(),
            MomentFilter::Assists => theme::assist(),
            MomentFilter::Objectives => theme::objective(),
            MomentFilter::Notes => theme::text(),
        }
    }
}

/// Toggle chip for a moment kind; clicking the active chip clears the filter.
pub fn filter_chip(
    id: impl Into<ElementId>,
    f: MomentFilter,
    current: MomentFilter,
    count: usize,
    on_pick: impl Fn(MomentFilter, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let active = f == current;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_1p5()
        .h(px(24.))
        .px_2()
        .rounded_full()
        .border_1()
        .text_sm()
        .border_color(if active { f.color() } else { theme::line() })
        .bg(if active { theme::raised() } else { theme::bg() })
        .text_color(if active { theme::text() } else { theme::muted() })
        .cursor_pointer()
        .hover(|s| s.text_color(theme::text()))
        .map(|d| match f {
            MomentFilter::Saved => d.child(icon(IconName::Star, 11., f.color())),
            _ => d.child(div().size(px(7.)).rounded_full().bg(f.color())),
        })
        .child(f.label())
        .child(div().text_color(theme::muted()).child(count.to_string()))
        .on_click(move |_, window, cx| on_pick(if active { MomentFilter::All } else { f }, window, cx))
}

/// The 52px strip at the top of the content area; also drags the window,
/// since the title bar is transparent.
pub fn page_header() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_3()
        .h(px(52.))
        .px_6()
        .border_b_1()
        .border_color(theme::line())
}

/// Empty region that drags the window (double-click zooms, like a title bar).
pub fn drag_area() -> Div {
    div().flex_1().h_full().on_mouse_down(MouseButton::Left, |ev, window, _| {
        if ev.click_count == 2 {
            window.titlebar_double_click();
        } else {
            window.start_window_move();
        }
    })
}

pub fn human_bytes(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else {
        format!("{:.0} MB", b / 1e6)
    }
}

/// A keyboard key cap, for shortcut hints.
pub fn kbd(label: &'static str) -> impl IntoElement {
    div()
        .px_1p5()
        .h(px(20.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(theme::line())
        .bg(theme::panel())
        .text_xs()
        .text_color(theme::muted())
        .child(label)
}


/// Star toggle for saving a moment to study later.
pub fn star_button(id: impl Into<ElementId>, saved: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(26.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .cursor_pointer()
        .hover(|s| s.bg(theme::line()))
        .child(icon(IconName::Star, 14., if saved { theme::kill() } else { theme::muted().opacity(0.6) }))
}


/// Move a file to the Trash and return where it went, so it can be put back.
#[cfg(target_os = "macos")]
pub fn trash_to(path: &std::path::Path) -> Option<std::path::PathBuf> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    if !path.exists() {
        return None;
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let mut out = None;
    NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, Some(&mut out)).ok()?;
    out.and_then(|u| u.path()).map(|p| std::path::PathBuf::from(p.to_string()))
}

/// Trash several files, remembering (original, in-trash) pairs for Undo.
#[cfg(target_os = "macos")]
pub fn trash_all(paths: &[std::path::PathBuf]) -> Vec<(std::path::PathBuf, std::path::PathBuf)> {
    paths.iter().filter_map(|p| trash_to(p).map(|t| (p.clone(), t))).collect()
}

/// What a toast's button does.
#[derive(Clone)]
pub enum ToastAction {
    /// Play a clip inside the app.
    OpenClip(std::path::PathBuf),
    Reveal(std::path::PathBuf),
    /// Put trashed files back: (original, in-trash) pairs.
    Undo(Vec<(std::path::PathBuf, std::path::PathBuf)>),
    /// Button that opens a page ("View" an available update).
    Go(crate::Page, &'static str),
}

/// Short-lived confirmation in the corner ("Clip saved", "Bookmark added").
#[derive(Clone)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub icon: IconName,
    pub action: Option<ToastAction>,
    pub until: std::time::Instant,
}

pub fn toast_view(
    t: &Toast,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
    on_open_clip: impl Fn(std::path::PathBuf, &mut Window, &mut App) + 'static,
    on_undo: impl Fn(Vec<(std::path::PathBuf, std::path::PathBuf)>, &mut Window, &mut App) + 'static,
    on_go: impl Fn(crate::Page, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let action = t.action.clone();
    div()
        .flex()
        .items_center()
        .gap_3()
        .min_w(px(280.))
        .max_w(px(440.))
        .pl_3p5()
        .pr_1p5()
        .py_2()
        .rounded_lg()
        .bg(theme::raised())
        .border_1()
        .border_color(theme::line())
        .shadow_lg()
        .child(icon(t.icon.clone(), 15., theme::text()))
        .child(div().flex_1().text_sm().child(t.text.clone()))
        .children(action.map(|a| match a {
            ToastAction::OpenClip(p) => solid_button(("toast-open", t.id as usize), theme::victory())
                .h(px(26.))
                .px_2p5()
                .text_color(theme::bg())
                .child(icon(IconName::Play, 11., theme::bg()))
                .child("Watch")
                .on_click(move |_, window, cx| on_open_clip(p.clone(), window, cx))
                .into_any_element(),
            // Undo now, or see it later under Recently deleted.
            ToastAction::Undo(files) => div()
                .flex()
                .items_center()
                .child(
                    ghost_button(("toast-undo", t.id as usize))
                        .text_color(theme::text())
                        .child(icon(IconName::Undo2, 12., theme::text()))
                        .child("Undo")
                        .on_click(move |_, window, cx| on_undo(files.clone(), window, cx)),
                )
                .child(ghost_button(("toast-view", t.id as usize)).child("Show").on_click(move |_, window, cx| on_go(crate::Page::Deleted, window, cx)))
                .into_any_element(),
            ToastAction::Go(page, label) => ghost_button(("toast-go", t.id as usize))
                .text_color(theme::text())
                .child(label)
                .on_click(move |_, window, cx| on_go(page, window, cx))
                .into_any_element(),
            ToastAction::Reveal(p) => ghost_button(("toast-show", t.id as usize))
                .child("Show")
                .on_click(move |_, _, _| {
                    let _ = std::process::Command::new("open").arg("-R").arg(&p).spawn();
                })
                .into_any_element(),
        }))
        .child(icon_button(("toast-x", t.id as usize), IconName::X, 11.).on_click(move |_, window, cx| on_close(window, cx)))
}

/// Hover tooltip builder for `.tooltip(...)`.
pub fn tip(text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(text).build(window, cx)
}

/// Compact filter chip: icon + count, label in the tooltip. Fits six in a row.
pub fn icon_chip(
    id: impl Into<ElementId>,
    f: MomentFilter,
    current: MomentFilter,
    count: usize,
    on_pick: impl Fn(MomentFilter, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let active = f == current;
    let glyph = match f {
        MomentFilter::Saved => IconName::Star,
        MomentFilter::Kills => IconName::Swords,
        MomentFilter::Deaths => IconName::Skull,
        MomentFilter::Assists => IconName::Handshake,
        MomentFilter::Objectives => IconName::Castle,
        MomentFilter::Notes => IconName::StickyNote,
        MomentFilter::All => IconName::Sparkles,
    };
    div()
        .id(id)
        .flex()
        .flex_1()
        .justify_center()
        .items_center()
        .gap_1p5()
        .h(px(28.))
        .rounded_md()
        .border_1()
        .text_sm()
        .border_color(if active { f.color() } else { theme::line() })
        .bg(if active { theme::raised() } else { theme::bg() })
        .text_color(if active || count > 0 { theme::text() } else { theme::muted().opacity(0.6) })
        .cursor_pointer()
        .hover(|s| s.bg(theme::raised()))
        .child(icon(glyph, 13., if count > 0 { f.color() } else { theme::muted().opacity(0.6) }))
        .child(count.to_string())
        .tooltip(tip(f.label()))
        .on_click(move |_, window, cx| on_pick(if active { MomentFilter::All } else { f }, window, cx))
}


/// Dropdown trigger in our own style (label + chevron), usable with
/// gpui-kit's Popover, which needs a `Selectable` trigger.
#[derive(IntoElement)]
pub struct PickerTrigger {
    id: ElementId,
    label: SharedString,
    active: bool,
    open: bool,
}

impl PickerTrigger {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, active: bool) -> Self {
        Self { id: id.into(), label: label.into(), active, open: false }
    }
}

impl gpui_kit::component::Selectable for PickerTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.open = selected;
        self
    }
    fn is_selected(&self) -> bool {
        self.open
    }
}

impl RenderOnce for PickerTrigger {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap_1()
            .h(px(28.))
            .pl_2p5()
            .pr_1p5()
            .rounded_md()
            .border_1()
            .text_sm()
            .cursor_pointer()
            .border_color(if self.active { theme::victory().opacity(0.6) } else { theme::line() })
            .bg(if self.active || self.open { theme::raised() } else { theme::bg() })
            .text_color(if self.active { theme::text() } else { theme::muted() })
            .hover(|s| s.text_color(theme::text()).bg(theme::raised()))
            .child(self.label)
            .child(icon(IconName::ChevronDown, 12., theme::muted()))
    }
}

/// Rendered Markdown (notes). Not selectable, so clicks reach the parent.
pub fn markdown(id: impl Into<ElementId>, text: impl Into<SharedString>) -> impl IntoElement {
    gpui_kit::component::text::TextView::markdown(id, text).selectable(false).text_sm()
}

/// One-line syntax reminder shown under a note editor.
pub fn markdown_hint() -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .text_xs()
        .text_color(theme::muted())
        .child("Markdown")
        .child("**bold**")
        .child("*italic*")
        .child("- list")
        .child("# heading")
        .child("[link](url)")
}

/// A note shown rendered; click to edit. `editing` swaps in the editor.
pub fn note_card(
    id: &'static str,
    text: &str,
    placeholder: &'static str,
    editing: bool,
    editor: AnyElement,
    on_edit: impl Fn(&mut Window, &mut App) + 'static,
    on_done: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    if editing {
        return div()
            .flex()
            .flex_col()
            .gap_2()
            .child(editor)
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(div().flex_1().child(markdown_hint()))
                    .child(
                        solid_button(SharedString::from(format!("{id}-done")), theme::raised())
                            .h(px(26.))
                            .child(icon(IconName::Check, 12., theme::text()))
                            .child("Done")
                            .on_click(move |_, window, cx| on_done(window, cx)),
                    ),
            )
            .into_any_element();
    }
    let empty = text.trim().is_empty();
    div()
        .id(SharedString::from(format!("{id}-view")))
        .group("note-card")
        .relative()
        .min_h(px(if empty { 72. } else { 40. }))
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme::line())
        .bg(if empty { theme::bg() } else { theme::panel() })
        .cursor_text()
        .hover(|s| s.border_color(theme::muted().opacity(0.6)))
        .map(|d| {
            if empty {
                d.flex()
                    .items_start()
                    .gap_2()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(div().pt(px(2.)).child(icon(IconName::NotebookPen, 14., theme::muted())))
                    .child(div().flex_1().min_w_0().child(placeholder))
            } else {
                d.child(markdown(SharedString::from(format!("{id}-md")), text.to_string())).child(
                    div()
                        .absolute()
                        .top_2()
                        .right_2()
                        .invisible()
                        .group_hover("note-card", |s| s.visible())
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1p5()
                        .rounded_md()
                        .bg(theme::raised())
                        .text_xs()
                        .text_color(theme::muted())
                        .child(icon(IconName::NotebookPen, 11., theme::muted()))
                        .child("Edit"),
                )
            }
        })
        .on_click(move |_, window, cx| on_edit(window, cx))
        .into_any_element()
}
