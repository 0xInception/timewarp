//! Games page: every recording, newest first, grouped by day.

use std::path::PathBuf;

use chrono::{Datelike, Local};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use gpui_kit::assets::IconName;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};

use crate::{TimewarpApp, store, theme, timeline, ui};

impl TimewarpApp {
    /// Right-click menu for a game row.
    fn game_menu(
        &self,
        idx: usize,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let app = cx.entity().downgrade();
        let g = &self.games[idx];
        let path = g.path.clone();
        let pinned = self.store.read(cx).game(g).is_some_and(|n| n.pinned);
        let condensed = g.meta.condensed.is_some();
        let role = g.meta.player.as_ref().map(|p| p.position.clone()).unwrap_or_default();
        let has_override = g.meta.role_override.is_some();
        let deaths: Vec<(usize, f64)> = store::moments(&g.meta, self.store.read(cx).game(g))
            .iter()
            .filter(|m| crate::ui::MomentFilter::Deaths.matches(m))
            .map(|m| (idx, m.video_time))
            .collect();
        move |menu, _, _| {
            let with = |f: Box<dyn Fn(&mut TimewarpApp, &mut Window, &mut Context<TimewarpApp>)>| {
                let app = app.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |a, cx| f(a, window, cx));
                    }
                }
            };
            let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path.clone());
            let deaths = deaths.clone();
            let mut menu = menu.item(
                PopupMenuItem::new("Review").icon(Icon::new(IconName::Play)).on_click(with(Box::new(move |a, w, cx| a.open_review(idx, None, w, cx)))),
            );
            if !deaths.is_empty() {
                let n = deaths.len();
                menu = menu.item(PopupMenuItem::new(format!("Watch deaths ({n})")).icon(Icon::new(IconName::Skull)).on_click(with(Box::new(
                    move |a, w, cx| a.start_playlist("Deaths".into(), deaths.clone(), w, cx),
                ))));
            }
            // Your role drives matchups; fix it here when League didn't say.
            menu = menu.separator().label("My role");
            for r in crate::filters::ROLES {
                let p = path.clone();
                menu = menu.item(
                    PopupMenuItem::new(crate::filters::role_label(r))
                        .checked(role == r)
                        .on_click(with(Box::new(move |a, _, cx| a.set_role(&p, Some(r.to_string()), cx)))),
                );
            }
            if has_override {
                let p = path.clone();
                menu = menu.item(PopupMenuItem::new("Detect automatically").on_click(with(Box::new(move |a, _, cx| a.set_role(&p, None, cx)))));
            }
            menu = menu.separator().item(
                PopupMenuItem::new(if pinned { "Stop keeping" } else { "Keep forever" })
                    .icon(Icon::new(IconName::Pin))
                    .on_click(with(Box::new(move |a, _, cx| a.toggle_keep_game(&p1, cx)))),
            );
            if !condensed && !pinned {
                menu = menu.item(PopupMenuItem::new("Keep moments only…").icon(Icon::new(IconName::Scissors)).on_click(with(Box::new(
                    move |a, _, cx| {
                        a.page = crate::Page::Games;
                        a.confirm_condense = Some(vec![p2.clone()]);
                        cx.notify();
                    },
                ))));
            }
            menu.item(PopupMenuItem::new("Show in Finder").icon(Icon::new(IconName::FolderOpen)).on_click(move |_, _, _| {
                let _ = std::process::Command::new("open").arg("-R").arg(&p3).spawn();
            }))
            .separator()
            .item(
                PopupMenuItem::new("Move to Trash")
                    .icon(Icon::new(IconName::Trash))
                    .on_click(with(Box::new(move |a, _, cx| a.delete_game(p4.clone(), cx)))),
            )
        }
    }

    pub fn visible_games(&self) -> Vec<usize> {
        (0..self.games.len()).filter(|&i| self.filter.matches(&self.games[i].meta)).collect()
    }

    pub fn portrait(&self, key: Option<&str>, fallback: &str, size: f32) -> AnyElement {
        match key.and_then(|k| self.icons.champion(k)) {
            Some(path) => img(path).flex_none().size(px(size)).rounded(px(size * 0.22)).into_any_element(),
            None => div()
                .size(px(size))
                .flex_none()
                .rounded(px(size * 0.22))
                .bg(theme::raised())
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::muted())
                .child(fallback.chars().next().unwrap_or('?').to_string())
                .into_any_element(),
        }
    }

    pub fn game_row(&self, idx: usize, cx: &mut Context<Self>) -> AnyElement {
        let g = &self.games[idx];
        let m = &g.meta;
        let notes = self.store.read(cx).game(g).cloned();
        let moments = store::moments(m, notes.as_ref());
        let has_notes = notes.as_ref().is_some_and(|n| !n.note.trim().is_empty() || !n.notes.is_empty());
        let champ = m.champion.clone().unwrap_or_else(|| "Manual recording".into());
        let queue = queue_label(m);
        let duration = m.duration_secs;
        let game_length = m.condensed.as_ref().map(|c| c.original_duration_secs).unwrap_or(duration);
        let condensed = m.condensed.is_some();
        let entity = cx.entity().downgrade();
        let result = match m.result.as_deref() {
            Some("Win") => Some(("Victory", theme::victory())),
            Some("Lose") => Some(("Defeat", theme::defeat())),
            _ => None,
        };

        let kda = div().flex().flex_col().w(px(108.)).flex_none().map(|d| match &m.player {
            Some(p) => d
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(p.kills.to_string())
                        .child(div().text_color(theme::muted()).child("/"))
                        .child(div().text_color(theme::death()).child(p.deaths.to_string()))
                        .child(div().text_color(theme::muted()).child("/"))
                        .child(p.assists.to_string()),
                )
                .child(div().text_sm().text_color(theme::muted()).child(format!("{} CS", p.cs))),
            None => d.child(div().text_color(theme::muted()).child("No stats")),
        });

        let opponent = div().w(px(64.)).flex_none().flex().items_center().gap_1p5().children(m.opponent.as_ref().map(|o| {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_sm()
                .text_color(theme::muted())
                .child("vs")
                .child(self.portrait(Some(&o.champion_key), &o.champion, 24.))
        }));

        let selected = self.selected.contains(&g.path);
        let selecting = !self.selected.is_empty();
        let focused = self.list_cursor.as_ref() == Some(&g.path);
        div()
            .id(("game", idx))
            .group("game-row")
            .relative()
            .child(self.bounds_probe(g.path.clone()))
            .flex()
            .items_center()
            .gap_4()
            .h(px(64.))
            .pl_2()
            .pr_2()
            .rounded_lg()
            .cursor_pointer()
            .border_1()
            .border_color(if focused {
                theme::accent()
            } else if selected {
                theme::victory().opacity(0.5)
            } else {
                gpui_kit::transparent_black()
            })
            .when(selected, |d| d.bg(theme::raised()))
            .when(!selected, |d| d.hover(|s| s.bg(theme::panel())))
            // Selection checkbox: on hover, or always while selecting.
            .child({
                let path = g.path.clone();
                div()
                    .id(("select", idx))
                    .flex_none()
                    .size(px(18.))
                    .rounded(px(4.))
                    .border_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_color(if selected { theme::victory() } else { theme::muted() })
                    .when(selected, |d| d.bg(theme::victory()))
                    .when(!selecting, |d| d.invisible().group_hover("game-row", |s| s.visible()))
                    .when(selected, |d| d.child(ui::icon(gpui_kit::assets::IconName::Check, 12., theme::bg())))
                    .tooltip(ui::tip("Select (⌘-click, ⇧-click for a range)"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_selected(&path, idx);
                        cx.notify();
                    }))
            })
            .child(
                // result edge: blue victory, red defeat, none otherwise
                div().w(px(3.)).h(px(36.)).rounded_full().bg(theme::result(m.result.as_deref())),
            )
            .child(self.portrait(m.player.as_ref().map(|p| p.champion_key.as_str()), &champ, 40.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(210.))
                    .flex_none()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(div().font_weight(FontWeight::SEMIBOLD).truncate().child(champ))
                            .when(has_notes, |d| d.child(ui::icon(gpui_kit::assets::IconName::StickyNote, 12., theme::muted()))),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .text_sm()
                            .children(result.map(|(l, c)| div().text_color(c).child(l)))
                            .child(div().text_color(theme::muted()).truncate().child(queue)),
                    )
                    .when(condensed, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(theme::muted())
                                .child(ui::icon(gpui_kit::assets::IconName::Scissors, 11., theme::muted()))
                                .child(format!("Moments only, {} kept", theme::fmt_clock(duration))),
                        )
                    }),
            )
            .child(kda)
            .child(opponent)
            .child(div().flex_1().min_w(px(48.)).px_2().child(timeline::mini_strip(
                ("strip", idx),
                &moments,
                duration,
                move |t, window, cx| {
                    if let Some(app) = entity.upgrade() {
                        app.update(cx, |app, cx| {
                            if !std::mem::take(&mut app.suppress_click) {
                                app.open_review(idx, Some(t), window, cx)
                            }
                        });
                    }
                },
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .w(px(72.))
                    .flex_none()
                    .child(theme::fmt_clock(game_length))
                    .child(div().text_sm().text_color(theme::muted()).child(m.started_at.format("%H:%M").to_string())),
            )
            // Row actions, visible on hover (same as right-click).
            .child(
                div().flex_none().invisible().group_hover("game-row", |s| s.visible()).child(
                    Button::new(("game-more", idx))
                        .ghost()
                        .icon(Icon::new(IconName::Ellipsis))
                        .tooltip("Actions")
                        .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, self.game_menu(idx, cx)),
                ),
            )
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                if std::mem::take(&mut this.suppress_click) {
                    return;
                }
                let m = ev.modifiers();
                let path = this.games[idx].path.clone();
                this.list_cursor = Some(path.clone());
                if m.shift {
                    this.select_range_to(idx);
                } else if m.platform || !this.selected.is_empty() {
                    // ⌘-click, or any click while selecting, toggles.
                    this.toggle_selected(&path, idx);
                } else {
                    this.open_review(idx, None, window, cx);
                }
                cx.notify();
            }))
            .context_menu(self.game_menu(idx, cx))
            .into_any_element()
    }

    fn toggle_selected(&mut self, path: &PathBuf, idx: usize) {
        if !self.selected.remove(path) {
            self.selected.insert(path.clone());
        }
        self.select_anchor = Some(idx);
    }

    /// ⇧-click: select everything between the last clicked row and this one,
    /// in the order the list shows them.
    fn select_range_to(&mut self, idx: usize) {
        let order = self.visible_games();
        let anchor = self.select_anchor.unwrap_or(idx);
        let (Some(a), Some(b)) = (order.iter().position(|&i| i == anchor), order.iter().position(|&i| i == idx)) else {
            return;
        };
        let (lo, hi) = (a.min(b), a.max(b));
        for &i in &order[lo..=hi] {
            self.selected.insert(self.games[i].path.clone());
        }
        self.select_anchor = Some(idx);
    }

    /// Replaces the page header while games are selected.
    fn selection_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let paths: Vec<PathBuf> = self.selected.iter().cloned().collect();
        let n = paths.len();
        let bytes: u64 = self.games.iter().filter(|g| self.selected.contains(&g.path)).map(|g| g.bytes).sum();
        let condensable: Vec<PathBuf> = self
            .games
            .iter()
            .filter(|g| self.selected.contains(&g.path) && g.meta.condensed.is_none())
            .filter(|g| !self.store.read(cx).game(g).is_some_and(|n| n.pinned))
            .map(|g| g.path.clone())
            .collect();
        let all_kept = self
            .games
            .iter()
            .filter(|g| self.selected.contains(&g.path))
            .all(|g| self.store.read(cx).game(g).is_some_and(|n| n.pinned));
        let visible_n = self.visible_games().len();

        ui::page_header()
            .bg(theme::raised().opacity(0.5))
            .child(
                ui::icon_button("clear-selection", IconName::X, 14.)
                    .tooltip(ui::tip("Clear selection (Esc)"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected.clear();
                        cx.notify();
                    })),
            )
            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{n} selected")))
            .child(div().text_sm().text_color(theme::muted()).child(ui::human_bytes(bytes)))
            .when(n < visible_n, |d| {
                d.child(ui::ghost_button("select-all").child(format!("Select all {visible_n}")).on_click(cx.listener(|this, _, _, cx| {
                    let all: Vec<PathBuf> = this.visible_games().into_iter().map(|i| this.games[i].path.clone()).collect();
                    this.selected.extend(all);
                    cx.notify();
                })))
            })
            .child(ui::drag_area())
            .when(!condensable.is_empty(), |d| {
                let c = condensable.clone();
                d.child(
                    ui::ghost_button("sel-condense")
                        .text_color(theme::text())
                        .child(ui::icon(IconName::Scissors, 13., theme::kill()))
                        .child(format!("Keep moments only ({})", c.len()))
                        .tooltip(ui::tip("Cut to the highlights and remove the full-length video"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.confirm_condense = Some(c.clone());
                            cx.notify();
                        })),
                )
            })
            .child({
                let p = paths.clone();
                ui::ghost_button("sel-keep")
                    .text_color(theme::text())
                    .child(ui::icon(IconName::Pin, 13., theme::text()))
                    .child(if all_kept { "Stop keeping" } else { "Keep forever" })
                    .on_click(cx.listener(move |this, _, _, cx| this.set_kept(&p, !all_kept, cx)))
            })
            .child({
                let p = paths.clone();
                ui::ghost_button("sel-export")
                    .text_color(theme::text())
                    .child(ui::icon(IconName::FolderOutput, 13., theme::text()))
                    .child("Export…")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let parts = crate::transfer::Parts {
                            notes: true,
                            matchups: false,
                            clips: false,
                            recordings: true,
                            settings: false,
                        };
                        this.export_with(parts, p.clone(), vec![], cx);
                    }))
            })
            .child({
                let p = paths;
                ui::solid_button("sel-trash", theme::death())
                    .text_color(theme::text())
                    .child(ui::icon(IconName::Trash, 13., theme::text()))
                    .child("Move to Trash")
                    .tooltip(ui::tip("Delete or Backspace. You can undo."))
                    .on_click(cx.listener(move |this, _, _, cx| this.trash_games(p.clone(), cx)))
            })
    }

    pub fn games_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let visible = self.visible_games();
        let shown: Vec<_> = visible.iter().map(|&i| &self.games[i].meta).collect();
        let wins = shown.iter().filter(|m| m.result.as_deref() == Some("Win")).count();
        let losses = shown.iter().filter(|m| m.result.as_deref() == Some("Lose")).count();

        let header = if !self.selected.is_empty() {
            self.selection_bar(cx).into_any_element()
        } else {
            ui::page_header()
            .child(self.games_view_switch(cx))
            .child(ui::drag_area())
            .when(wins + losses > 0, |d| {
                let rate = wins as f64 / (wins + losses) as f64 * 100.;
                d.child(div().text_sm().text_color(theme::muted()).child(format!("{wins}W {losses}L, {rate:.0}% win rate")))
            })
            .child(
                ui::ghost_button("folder")
                    .child(ui::icon(gpui_kit::assets::IconName::FolderOpen, 14., theme::muted()))
                    .child("Show in Finder")
                    .on_click({
                        let dir = self.config.output_dir.clone();
                        move |_, _, _| {
                            let _ = std::fs::create_dir_all(&dir);
                            let _ = std::process::Command::new("open").arg(&dir).spawn();
                        }
                    }),
            )
            .into_any_element()
        };
        // Bulk "moments only" for exactly what the filter shows.
        let bulk: Vec<PathBuf> = visible
            .iter()
            .map(|&i| &self.games[i])
            .filter(|g| g.meta.condensed.is_none() && !self.store.read(cx).game(g).is_some_and(|n| n.pinned))
            .map(|g| g.path.clone())
            .collect();
        let busy = self.maintenance.active.is_some();
        let filters = div()
            .flex()
            .items_start()
            .gap_3()
            .px_6()
            .py_3()
            .child(div().flex_1().child(self.filter_bar(true, cx)))
            .when(!bulk.is_empty() && self.confirm_condense.is_none() && !busy && self.selected.is_empty(), |d| {
                d.child(
                    ui::ghost_button("bulk-condense")
                        .flex_none()
                        .child(ui::icon(gpui_kit::assets::IconName::Scissors, 13., theme::muted()))
                        .child(format!("Keep moments only ({})", bulk.len()))
                        .on_click(cx.listener({
                            let bulk = bulk.clone();
                            move |this, _, _, cx| {
                                this.confirm_condense = Some(bulk.clone());
                                cx.notify();
                            }
                        })),
                )
            });
        let confirm = self.confirm_condense.clone().filter(|b| !b.is_empty()).map(|bulk| {
            let keep = self.config.storage.keep;
            let kinds: Vec<&str> = [
                (keep.kills, "kills"),
                (keep.deaths, "deaths"),
                (keep.assists, "assists"),
                (keep.objectives, "objectives"),
            ]
            .into_iter()
            .filter_map(|(on, l)| on.then_some(l))
            .collect();
            let saves = self.condense_savings(&bulk, cx);
            let n = bulk.len();
            div()
                .flex()
                .items_center()
                .gap_3()
                .mx_6()
                .mb_2()
                .px_4()
                .py_3()
                .rounded_lg()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::kill().opacity(0.5))
                .child(ui::icon(gpui_kit::assets::IconName::Scissors, 15., theme::kill()))
                .child(div().flex_1().text_sm().child(format!(
                    "Cut {n} game{} down to their {}, notes, bookmarks and saved moments? The full videos go to the Trash. Saves about {} once you empty it.",
                    if n == 1 { "" } else { "s" },
                    if kinds.is_empty() { "nothing but".to_string() } else { kinds.join(", ") },
                    ui::human_bytes(saves)
                )))
                .child(
                    ui::solid_button("bulk-condense-go", theme::kill())
                        .flex_none()
                        .text_color(theme::bg())
                        .child("Keep moments only")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.confirm_condense = None;
                            this.selected.clear();
                            this.queue_condense_manual(bulk.clone(), cx);
                        })),
                )
                .child(ui::ghost_button("bulk-condense-cancel").flex_none().child("Cancel").on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_condense = None;
                    cx.notify();
                })))
        });

        let body = if visible.is_empty() {
            let (title, body) = if self.games.is_empty() {
                (
                    "No games yet",
                    "Start a League match and Timewarp records it on its own. Your kills, deaths and assists show up here as moments you can jump straight to.",
                )
            } else {
                ("Nothing here", "No recorded games match this filter.")
            };
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_lg().font_weight(FontWeight::MEDIUM).child(title))
                .child(div().max_w(px(420.)).text_center().text_color(theme::muted()).child(body))
                .into_any_element()
        } else {
            let mut children: Vec<AnyElement> = vec![];
            let mut last_day = None;
            for i in visible {
                let day = self.games[i].meta.started_at.date_naive();
                if last_day != Some(day) {
                    last_day = Some(day);
                    children.push(
                        div()
                            .pt_5()
                            .pb_1()
                            .pl_3()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::muted())
                            .child(day_label(day))
                            .into_any_element(),
                    );
                }
                children.push(self.game_row(i, cx));
            }
            self.item_bounds.borrow_mut().clear();
            let list = div()
                .id("games")
                .track_scroll(&self.games_scroll)
                .size_full()
                .overflow_y_scroll()
                .px_3()
                .pb_6()
                .flex()
                .flex_col()
                .children(children)
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
                        this.marquee_down(crate::marquee::SelectTarget::Games, ev.position, ev.modifiers)
                    }),
                )
                .child(list)
                .into_any_element()
        };

        div().size_full().flex().flex_col().child(header).child(filters).children(confirm).child(body)
    }
}

pub fn queue_label(m: &timewarp_engine::SessionMeta) -> String {
    m.queue.as_deref().map(short_queue).or_else(|| m.game_mode.clone().map(pretty_mode)).unwrap_or_default()
}

/// Riot's queue descriptions are long ("Multiplayer Practice Tool Custom");
/// show the name players actually use.
fn short_queue(q: &str) -> String {
    let known = [
        ("Practice Tool", "Practice Tool"),
        ("Ranked Solo", "Ranked Solo/Duo"),
        ("Ranked Flex", "Ranked Flex"),
        ("Draft Pick", "Normal Draft"),
        ("Blind Pick", "Normal Blind"),
        ("Quickplay", "Quickplay"),
        ("Swiftplay", "Swiftplay"),
        ("ARAM", "ARAM"),
        ("Arena", "Arena"),
        ("Co-op vs", "Co-op vs AI"),
        ("Intro", "Co-op vs AI"),
        ("Beginner", "Co-op vs AI"),
        ("Intermediate", "Co-op vs AI"),
    ];
    known.iter().find(|(k, _)| q.contains(k)).map(|(_, v)| v.to_string()).unwrap_or_else(|| q.trim_end_matches(" Custom").to_string())
}

fn pretty_mode(mode: String) -> String {
    match mode.as_str() {
        "CLASSIC" => "Summoner's Rift".into(),
        "PRACTICETOOL" => "Practice Tool".into(),
        "CHERRY" => "Arena".into(),
        _ => mode,
    }
}

fn day_label(day: chrono::NaiveDate) -> String {
    let today = Local::now().date_naive();
    match (today - day).num_days() {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..7 => day.format("%A").to_string(),
        _ if day.year() == today.year() => day.format("%B %-d").to_string(),
        _ => day.format("%B %-d, %Y").to_string(),
    }
}
