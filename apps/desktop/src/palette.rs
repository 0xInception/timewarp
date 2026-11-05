//! ⌘K: one box to go anywhere and do anything. Type a champion, a matchup, a
//! clip title or a command; ↑↓ to move, ↵ to run. Every result shows its
//! keyboard shortcut, so the palette also teaches them. `?` opens the full
//! shortcut sheet.

use std::path::PathBuf;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use timewarp_engine::{Command, Phase};

use crate::ui::{self, icon};
use crate::{Page, TimewarpApp, store, theme};

#[derive(Clone)]
pub enum PaletteAction {
    Page(Page),
    Game(PathBuf),
    Clip(PathBuf),
    Matchup(String),
    CheckUpdates,
    StartStop,
    ToggleAutoRecord,
    Export,
    Import,
    OpenFolder,
    Shortcuts,
}

#[derive(Clone)]
pub struct PaletteItem {
    group: &'static str,
    title: String,
    detail: String,
    shortcut: Option<&'static str>,
    icon: IconName,
    action: PaletteAction,
}

pub struct Palette {
    input: Entity<InputState>,
    selected: usize,
    _sub: Subscription,
}

/// Case-insensitive match: substring beats in-order letters; None = no match.
fn score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let (q, t) = (query.to_lowercase(), text.to_lowercase());
    if let Some(pos) = t.find(&q) {
        // Earlier and word-start matches rank higher.
        let word_start = pos == 0 || !t.as_bytes()[pos - 1].is_ascii_alphanumeric();
        return Some(1000 - pos as i32 + if word_start { 200 } else { 0 });
    }
    let mut it = t.chars();
    q.chars().all(|c| it.any(|x| x == c)).then_some(100)
}

impl TimewarpApp {
    pub fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            return self.close_palette(window, cx);
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search games, champions, clips, or type a command…"));
        input.update(cx, |s, cx| s.focus(window, cx));
        let sub = cx.subscribe(&input, |this, _, ev: &InputEvent, cx| {
            if let InputEvent::Change = ev {
                if let Some(p) = &mut this.palette {
                    p.selected = 0;
                }
                cx.notify();
            }
        });
        self.palette = Some(Palette { input, selected: 0, _sub: sub });
        cx.notify();
    }

    pub fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn palette_items(&self, query: &str, cx: &App) -> Vec<PaletteItem> {
        let recording = matches!(self.status.phase, Phase::Recording | Phase::Finalizing);
        let mut all: Vec<PaletteItem> = vec![];
        let act = |title: &str, detail: &str, shortcut, icon, action| PaletteItem {
            group: "Actions",
            title: title.into(),
            detail: detail.into(),
            shortcut,
            icon,
            action,
        };
        all.push(act(
            if recording { "Stop recording" } else { "Start recording now" },
            "",
            None,
            if recording { IconName::Square } else { IconName::Circle },
            PaletteAction::StartStop,
        ));
        all.push(act(
            if self.config.auto_record { "Turn off auto-record" } else { "Turn on auto-record" },
            "Record matches automatically",
            None,
            IconName::Settings,
            PaletteAction::ToggleAutoRecord,
        ));
        all.push(act("Check for updates", "See if there's a newer Timewarp (nothing installs without you)", None, IconName::RefreshCw, PaletteAction::CheckUpdates));
        all.push(act("Export…", "Back up notes, clips, recordings and settings", None, IconName::FolderOutput, PaletteAction::Export));
        all.push(act("Import…", "Bring in a Timewarp export", None, IconName::FolderInput, PaletteAction::Import));
        all.push(act("Show recordings in Finder", "", None, IconName::FolderOpen, PaletteAction::OpenFolder));
        all.push(act("Keyboard shortcuts", "Every shortcut in Timewarp", Some("?"), IconName::Keyboard, PaletteAction::Shortcuts));

        for (page, title, sc, ic) in [
            (Page::Home, "Home", "⌘1", IconName::House),
            (Page::Games, "Games", "⌘2", IconName::Gamepad2),
            (Page::Moments, "Moments", "", IconName::Sparkles),
            (Page::Matchups, "Matchups", "⌘3", IconName::Swords),
            (Page::Clips, "Clips", "⌘4", IconName::Scissors),
            (Page::Deleted, "Recently deleted", "", IconName::Trash),
            (Page::About, "About Timewarp", "", IconName::Info),
            (Page::Settings, "Settings", "⌘,", IconName::Settings),
            (Page::Setup, "Permissions and setup", "", IconName::ShieldCheck),
        ] {
            all.push(PaletteItem {
                group: "Go to",
                title: title.into(),
                detail: String::new(),
                shortcut: (!sc.is_empty()).then_some(sc),
                icon: ic,
                action: PaletteAction::Page(page),
            });
        }

        for g in &self.games {
            let m = &g.meta;
            let champ = m.champion.clone().unwrap_or_else(|| "Manual recording".into());
            let vs = m.opponent.as_ref().map(|o| format!(" vs {}", o.champion)).unwrap_or_default();
            let result = match m.result.as_deref() {
                Some("Win") => "Victory, ",
                Some("Lose") => "Defeat, ",
                _ => "",
            };
            all.push(PaletteItem {
                group: "Games",
                title: format!("{champ}{vs}"),
                detail: format!("{result}{} {}", crate::games::queue_label(m), m.started_at.format("%b %-d, %H:%M")),
                shortcut: None,
                icon: IconName::Gamepad2,
                action: PaletteAction::Game(g.path.clone()),
            });
        }

        let mut seen = std::collections::BTreeSet::new();
        for g in &self.games {
            if let (Some(key), Some(label)) = (store::matchup_key(&g.meta), store::matchup_label(&g.meta)) {
                if seen.insert(key.clone()) {
                    let role = g.meta.player.as_ref().map(|p| crate::filters::role_label(&p.position)).unwrap_or("");
                    all.push(PaletteItem {
                        group: "Matchups",
                        title: label,
                        detail: format!("{role} matchup"),
                        shortcut: None,
                        icon: IconName::Swords,
                        action: PaletteAction::Matchup(key),
                    });
                }
            }
        }

        #[cfg(target_os = "macos")]
        for c in &self.clips {
            all.push(PaletteItem {
                group: "Clips",
                title: c.title(),
                detail: c.meta.opponent.as_ref().map(|o| format!("vs {o}")).unwrap_or_default(),
                shortcut: None,
                icon: IconName::Scissors,
                action: PaletteAction::Clip(c.path.clone()),
            });
        }
        let _ = cx;

        if query.trim().is_empty() {
            // No query: actions, places, and the latest few games.
            let mut out: Vec<PaletteItem> = all.iter().filter(|i| i.group == "Actions" || i.group == "Go to").cloned().collect();
            out.extend(all.iter().filter(|i| i.group == "Games").take(5).cloned());
            return out;
        }
        let mut scored: Vec<(i32, usize, PaletteItem)> = all
            .into_iter()
            .enumerate()
            .filter_map(|(n, i)| {
                let s = score(query, &i.title).or_else(|| score(query, &i.detail).map(|s| s / 2))?;
                Some((s, n, i))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().take(40).map(|(_, _, i)| i).collect()
    }

    fn run_palette(&mut self, action: PaletteAction, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(window, cx);
        match action {
            PaletteAction::Page(p) => self.go(p, cx),
            PaletteAction::Game(path) => {
                if let Some(i) = self.games.iter().position(|g| g.path == path) {
                    self.open_review(i, None, window, cx);
                }
            }
            #[cfg(target_os = "macos")]
            PaletteAction::Clip(path) => self.open_clip_path(&path, window, cx),
            #[cfg(not(target_os = "macos"))]
            PaletteAction::Clip(_) => {}
            PaletteAction::Matchup(key) => {
                self.go(Page::Matchups, cx);
                self.selected_matchup = Some(key);
            }
            PaletteAction::StartStop => {
                let recording = matches!(self.status.phase, Phase::Recording | Phase::Finalizing);
                self.engine.send(if recording { Command::Stop } else { Command::StartManual });
            }
            PaletteAction::ToggleAutoRecord => {
                let on = !self.config.auto_record;
                self.update_config(|c| c.auto_record = on, cx);
                self.toast(IconName::Settings, if on { "Auto-record is on." } else { "Auto-record is off." }, None, cx);
            }
            PaletteAction::CheckUpdates => {
                self.about_tab = crate::about::AboutTab::Overview;
                self.go(Page::About, cx);
                self.check_for_updates(false, cx);
            }
            PaletteAction::Export => {
                self.go(Page::Settings, cx);
                self.start_export(cx);
            }
            PaletteAction::Import => {
                self.go(Page::Settings, cx);
                self.choose_import(cx);
            }
            PaletteAction::OpenFolder => {
                let _ = std::process::Command::new("open").arg(&self.config.output_dir).spawn();
            }
            PaletteAction::Shortcuts => self.show_shortcuts = true,
        }
        cx.notify();
    }

    fn palette_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.palette else { return };
        let query = p.input.read(cx).value().to_string();
        let n = self.palette_items(&query, cx).len();
        let Some(p) = &mut self.palette else { return };
        match ev.keystroke.key.as_str() {
            "down" => p.selected = (p.selected + 1).min(n.saturating_sub(1)),
            "up" => p.selected = p.selected.saturating_sub(1),
            "escape" => return self.close_palette(window, cx),
            "enter" => {
                let items = self.palette_items(&query, cx);
                let sel = self.palette.as_ref().map(|p| p.selected).unwrap_or(0);
                if let Some(item) = items.get(sel) {
                    let action = item.action.clone();
                    cx.stop_propagation();
                    return self.run_palette(action, window, cx);
                }
                return;
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub fn palette_overlay(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = self.palette.as_ref()?;
        let query = p.input.read(cx).value().to_string();
        let items = self.palette_items(&query, cx);
        let selected = p.selected.min(items.len().saturating_sub(1));

        let mut rows: Vec<AnyElement> = vec![];
        let mut last_group = "";
        for (i, item) in items.iter().enumerate() {
            if item.group != last_group {
                last_group = item.group;
                rows.push(
                    div().px_3().pt_3().pb_1().text_xs().font_weight(FontWeight::MEDIUM).text_color(theme::muted()).child(item.group).into_any_element(),
                );
            }
            let action = item.action.clone();
            let on = i == selected;
            rows.push(
                div()
                    .id(("palette-item", i))
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(36.))
                    .px_3()
                    .mx_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(on, |d| d.bg(theme::accent().opacity(0.18)))
                    .when(!on, |d| d.hover(|s| s.bg(theme::raised())))
                    .child(icon(item.icon.clone(), 14., if on { theme::accent() } else { theme::muted() }))
                    .child(div().flex_none().max_w(px(320.)).truncate().child(item.title.clone()))
                    .child(div().flex_1().min_w_0().truncate().text_sm().text_color(theme::muted()).child(item.detail.clone()))
                    .children(item.shortcut.map(ui::kbd))
                    .on_click(cx.listener(move |this, _, window, cx| this.run_palette(action.clone(), window, cx)))
                    .into_any_element(),
            );
        }
        if items.is_empty() {
            rows.push(div().p_6().text_sm().text_color(theme::muted()).child(format!("Nothing matches “{query}”.")).into_any_element());
        }

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(96.))
                .bg(gpui_kit::black().opacity(0.45))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.close_palette(window, cx)))
                .child(
                    div()
                        .id("palette")
                        .w(px(640.))
                        .max_h(px(480.))
                        .flex()
                        .flex_col()
                        .rounded_xl()
                        .bg(theme::panel())
                        .border_1()
                        .border_color(theme::line())
                        .shadow_lg()
                        .overflow_hidden()
                        // Clicks inside don't close it.
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.palette_key(ev, window, cx)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_2()
                                .border_b_1()
                                .border_color(theme::line())
                                .child(icon(IconName::Sparkles, 15., theme::muted()))
                                .child(div().flex_1().child(Input::new(&p.input).appearance(false))),
                        )
                        .child(div().id("palette-results").flex_1().overflow_y_scroll().pb_2().children(rows))
                        .child(
                            div()
                                .flex()
                                .gap_4()
                                .px_3()
                                .py_2()
                                .border_t_1()
                                .border_color(theme::line())
                                .text_xs()
                                .text_color(theme::muted())
                                .child(div().flex().gap_1().items_center().child(ui::kbd("↑")).child(ui::kbd("↓")).child("move"))
                                .child(div().flex().gap_1().items_center().child(ui::kbd("↵")).child("open"))
                                .child(div().flex().gap_1().items_center().child(ui::kbd("esc")).child("close")),
                        ),
                ),
        )
    }

    /// `?`: every shortcut in one place.
    pub fn shortcuts_overlay(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.show_shortcuts {
            return None;
        }
        let groups: [(&str, &[(&[&'static str], &str)]); 3] = [
            (
                "Everywhere",
                &[
                    (&["⌘", "K"], "Search and commands"),
                    (&["?"], "This list"),
                    (&["⌘", "1"], "Home"),
                    (&["⌘", "2"], "Games"),
                    (&["⌘", "3"], "Matchups"),
                    (&["⌘", "4"], "Clips"),
                    (&["⌘", "["], "Back (or mouse back button)"),
                    (&["⌘", "]"], "Forward"),
                    (&["⌘", ","], "Settings"),
                ],
            ),
            (
                "Games and clips",
                &[
                    (&["↑", "↓"], "Move"),
                    (&["⇧", "↑↓"], "Extend selection"),
                    (&["↵"], "Open"),
                    (&["Space"], "Select (games) or preview (clips)"),
                    (&["⌘", "A"], "Select all"),
                    (&["⌫"], "Move selected to Trash"),
                    (&["Esc"], "Clear selection"),
                ],
            ),
            (
                "Reviewing a game",
                &[
                    (&["Space"], "Play or pause"),
                    (&["J", "K", "L"], "Back, pause, faster"),
                    (&["←", "→"], "5 seconds"),
                    (&[",", "."], "One frame"),
                    (&["<", ">"], "Slower, faster"),
                    (&["⇧", "←→"], "Previous, next moment"),
                    (&["N"], "Note"),
                    (&["D"], "Draw on the frame"),
                    (&["I", "O"], "Clip start, end"),
                    (&["-", "="], "Zoom timeline (0 fits)"),
                ],
            ),
        ];
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .bg(gpui_kit::black().opacity(0.45))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.show_shortcuts = false;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .gap_8()
                        .p_6()
                        .rounded_xl()
                        .bg(theme::panel())
                        .border_1()
                        .border_color(theme::line())
                        .shadow_lg()
                        .children(groups.iter().map(|(title, rows)| {
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .w(px(250.))
                                .child(div().pb_1().font_weight(FontWeight::SEMIBOLD).child(*title))
                                .children(rows.iter().map(|(keys, what)| {
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(div().flex_1().text_sm().text_color(theme::muted()).child(*what))
                                        .child(div().flex().gap_1().children(keys.iter().map(|k| ui::kbd(k))))
                                }))
                        })),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn ranks_substrings_and_word_starts_first() {
        assert!(score("gra", "Lee Sin vs Graves").unwrap() > score("lsg", "Lee Sin vs Graves").unwrap());
        assert!(score("lsg", "Lee Sin vs Graves").is_some());
        assert!(score("zed", "Lee Sin vs Graves").is_none());
        assert!(score("vs", "Lee Sin vs Graves").unwrap() > score("es", "Lee Sin vs Graves").unwrap());
    }
}
