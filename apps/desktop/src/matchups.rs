//! Matchups: your champion vs the enemy in your role (jungle vs jungle, mid
//! vs mid…). Each matchup collects your record, your notes for it, every
//! moment you saved in those games, and the games themselves. The shared
//! filter bar narrows further, e.g. only games where the enemy top was Darius.

use std::collections::BTreeMap;

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::filters::{ROLES, role_label};
use crate::store::{self, Moment};
use crate::{TimewarpApp, theme, ui};

struct Summary {
    key: String,
    role: String,
    me: (String, String),
    vs: (String, String),
    games: Vec<usize>,
    wins: usize,
    losses: usize,
}

impl TimewarpApp {
    fn summaries(&self) -> Vec<Summary> {
        let mut by_key: BTreeMap<String, Summary> = BTreeMap::new();
        for i in self.visible_games() {
            let m = &self.games[i].meta;
            let (Some(key), Some(me), Some(vs)) = (store::matchup_key(m), &m.player, &m.opponent) else { continue };
            let s = by_key.entry(key.clone()).or_insert_with(|| Summary {
                key,
                role: me.position.clone(),
                me: (me.champion.clone(), me.champion_key.clone()),
                vs: (vs.champion.clone(), vs.champion_key.clone()),
                games: vec![],
                wins: 0,
                losses: 0,
            });
            s.games.push(i);
            match m.result.as_deref() {
                Some("Win") => s.wins += 1,
                Some("Lose") => s.losses += 1,
                _ => {}
            }
        }
        let mut out: Vec<Summary> = by_key.into_values().collect();
        let role_rank = |r: &str| ROLES.iter().position(|x| *x == r).unwrap_or(ROLES.len());
        out.sort_by(|a, b| role_rank(&a.role).cmp(&role_rank(&b.role)).then(b.games.len().cmp(&a.games.len())).then(a.vs.0.cmp(&b.vs.0)));
        out
    }

    fn select_matchup(&mut self, key: String, cx: &mut Context<Self>) {
        self.matchup_note_editing = false;
        self.selected_matchup = Some(key);
        cx.notify();
    }

    /// The notes editor is created lazily (it needs a window) and rebuilt when
    /// the selection changes.
    fn ensure_matchup_editor(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.matchup_editor.as_ref().is_some_and(|(k, _, _)| k == key) {
            return;
        }
        let text = self.store.read(cx).matchup(key).map(|m| m.notes.clone()).unwrap_or_default();
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Power spikes, trades to take or avoid, wave and jungle tracking tips, builds that worked…")
                .default_value(text)
                .auto_grow(5, 18)
        });
        let k = key.to_string();
        let sub = cx.subscribe(&editor, move |this, state, ev: &InputEvent, cx| {
            if let InputEvent::Blur = ev {
                this.matchup_note_editing = false;
                cx.notify();
            }
            if let InputEvent::Change = ev {
                let text = state.read(cx).value().to_string();
                let k = k.clone();
                this.store.update(cx, |s, cx| {
                    s.set_matchup_notes(&k, text);
                    cx.notify();
                });
            }
        });
        self.matchup_editor = Some((key.to_string(), editor, sub));
    }

    fn matchup_item(&self, s: &Summary, selected: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let key = s.key.clone();
        let has_notes = self.store.read(cx).matchup(&s.key).is_some_and(|m| !m.notes.trim().is_empty());
        div()
            .id(SharedString::from(format!("mu-{}", s.key)))
            .flex()
            .items_center()
            .gap_2p5()
            .py_2()
            .px_3()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme::raised()))
            .when(!selected, |d| d.hover(|st| st.bg(theme::panel())))
            .child(self.portrait(Some(&s.me.1), &s.me.0, 28.))
            .child(self.portrait(Some(&s.vs.1), &s.vs.0, 28.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(format!("{} vs {}", s.me.0, s.vs.0)))
                    .child(div().text_sm().text_color(theme::muted()).truncate().child(format!(
                        "{}, {}W {}L",
                        match s.games.len() {
                            1 => "1 game".to_string(),
                            n => format!("{n} games"),
                        },
                        s.wins,
                        s.losses
                    ))),
            )
            .when(has_notes, |d| d.child(ui::icon(gpui_kit::assets::IconName::StickyNote, 13., theme::muted())))
            .on_click(cx.listener(move |this, _, _, cx| this.select_matchup(key.clone(), cx)))
    }

    fn matchup_detail(&self, s: &Summary, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.matchup_editor.as_ref().filter(|(k, _, _)| *k == s.key).map(|(_, e, _)| e.clone());
        let saved: Vec<(usize, Moment)> = {
            let store = self.store.read(cx);
            s.games
                .iter()
                .flat_map(|&gi| {
                    let g = &self.games[gi];
                    store::moments(&g.meta, store.game(g)).into_iter().filter(|m| m.saved).map(move |m| (gi, m))
                })
                .collect()
        };
        let total = s.wins + s.losses;

        let section = |title: &str, hint: Option<String>| {
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .pt_6()
                .pb_2()
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
                .children(hint.map(|h| div().text_sm().text_color(theme::muted()).child(h)))
        };

        div()
            .id("matchup-detail")
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .px_6()
            .pb_8()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .pt_5()
                    .child(self.portrait(Some(&s.me.1), &s.me.0, 44.))
                    .child(div().text_color(theme::muted()).child("vs"))
                    .child(self.portrait(Some(&s.vs.1), &s.vs.0, 44.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .ml_1()
                            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(format!("{} vs {}", s.me.0, s.vs.0)))
                            .child(div().text_color(theme::muted()).child(format!("{} matchup", role_label(&s.role)))),
                    )
                    .child(div().flex_1())
                    .when(total > 0, |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .child(
                                    div()
                                        .text_xl()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if s.wins * 2 >= total { theme::victory() } else { theme::defeat() })
                                        .child(format!("{:.0}%", s.wins as f64 / total as f64 * 100.)),
                                )
                                .child(div().text_sm().text_color(theme::muted()).child(format!("{}W {}L", s.wins, s.losses))),
                        )
                    }),
            )
            .child(section("Notes", Some("Shown again in review whenever you get this lane.".into())))
            .children(editor.map(|e| {
                let text = self.store.read(cx).matchup(&s.key).map(|m| m.notes.clone()).unwrap_or_default();
                let (app1, app2) = (cx.entity().downgrade(), cx.entity().downgrade());
                let e2 = e.clone();
                ui::note_card(
                    "matchup-page-note",
                    &text,
                    "Power spikes, trades to take or avoid, wave and jungle tracking tips, builds that worked…",
                    self.matchup_note_editing,
                    Textarea::new(&e).into_any_element(),
                    move |window, cx| {
                        if let Some(app) = app1.upgrade() {
                            app.update(cx, |app, cx| {
                                app.matchup_note_editing = true;
                                e2.update(cx, |s, cx| s.focus(window, cx));
                                cx.notify();
                            });
                        }
                    },
                    move |_, cx| {
                        if let Some(app) = app2.upgrade() {
                            app.update(cx, |app, cx| {
                                app.matchup_note_editing = false;
                                cx.notify();
                            });
                        }
                    },
                )
            }))
            .when(saved.len() > 1, |d| {
                let queue: Vec<(usize, f64)> = saved.iter().map(|(gi, m)| (*gi, m.video_time)).collect();
                let title = format!("Saved moments: {} vs {}", s.me.0, s.vs.0);
                d.child(
                    div().pt_6().child(
                        ui::solid_button("matchup-play-saved", theme::raised())
                            .child(ui::icon(gpui_kit::assets::IconName::ListVideo, 14., theme::text()))
                            .child(format!("Play {} saved moments", queue.len()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_playlist(title.clone(), queue.clone(), window, cx)
                            })),
                    ),
                )
            })
            .child(section(
                "Saved moments",
                Some(match saved.len() {
                    0 => "Star moments while reviewing these games to collect them here.".into(),
                    n => format!("{n} from {} games", s.games.len()),
                }),
            ))
            .children(saved.iter().enumerate().map(|(i, (gi, m))| self.moment_row(*gi, 50_000 + i, m, cx)))
            .child(section("Games", None))
            .children(s.games.iter().map(|&gi| self.game_row(gi, cx)).collect::<Vec<_>>())
    }

    pub fn matchups_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summaries = self.summaries();
        // Keep the selection valid under the current filters.
        let selected = self
            .selected_matchup
            .clone()
            .filter(|k| summaries.iter().any(|s| &s.key == k))
            .or_else(|| summaries.first().map(|s| s.key.clone()));
        if let Some(k) = &selected {
            self.ensure_matchup_editor(k, window, cx);
        }

        let header = ui::page_header()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Matchups"))
            .child(ui::drag_area());
        let filters = div().px_6().py_3().border_b_1().border_color(theme::line()).child(self.filter_bar(true, cx));

        let body = if summaries.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_lg().font_weight(FontWeight::MEDIUM).child(if self.filter.is_narrowed() {
                    "No matchups match these filters"
                } else {
                    "No matchups yet"
                }))
                .child(div().max_w(px(480.)).text_center().text_color(theme::muted()).child(if self.games.is_empty() {
                    "A matchup is your champion against the enemy in your role: jungle against jungle, mid against mid. Play a game and it shows up here.".to_string()
                } else {
                    format!(
                        "A matchup is your champion against the enemy in your role. None of your {} games has a known role yet, which happens in Practice Tool and ARAM. Right-click a game and choose My role to add it.",
                        self.games.len()
                    )
                }))
                .into_any_element()
        } else {
            let mut list: Vec<AnyElement> = vec![];
            let mut last_role: Option<&str> = None;
            for s in &summaries {
                if last_role != Some(s.role.as_str()) {
                    last_role = Some(s.role.as_str());
                    list.push(
                        div()
                            .pt_4()
                            .pb_1()
                            .px_3()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::muted())
                            .child(role_label(&s.role))
                            .into_any_element(),
                    );
                }
                let is_sel = selected.as_deref() == Some(s.key.as_str());
                list.push(self.matchup_item(s, is_sel, cx).into_any_element());
            }
            let detail = selected
                .as_ref()
                .and_then(|k| summaries.iter().find(|s| &s.key == k))
                .map(|s| self.matchup_detail(s, cx).into_any_element());
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("matchup-list")
                        .w(px(280.))
                        .flex_none()
                        .h_full()
                        .overflow_y_scroll()
                        .px_2()
                        .pb_4()
                        .border_r_1()
                        .border_color(theme::line())
                        .children(list),
                )
                .children(detail)
                .into_any_element()
        };

        div().size_full().flex().flex_col().child(header).child(filters).child(body)
    }
}
