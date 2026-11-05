//! Moments page: every tagged play and note across the games that match the
//! shared filter, newest game first. Star the ones worth studying.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::games::queue_label;
use crate::store::{self, Moment};
use crate::ui::{self, MomentFilter};
use crate::{TimewarpApp, theme};

impl TimewarpApp {
    fn toggle_saved(&mut self, game_idx: usize, m: &Moment, cx: &mut Context<Self>) {
        let Some(g) = self.games.get(game_idx) else { return };
        let key = store::game_key(g);
        self.store.update(cx, |s, cx| {
            s.toggle_saved(&key, m);
            cx.notify();
        });
    }

    /// One moment row; clicking plays it from a few seconds before.
    pub fn moment_row(&self, gi: usize, mi: usize, m: &Moment, cx: &mut Context<Self>) -> AnyElement {
        let t = m.video_time;
        let star_m = m.clone();
        let is_note = m.kind == store::MomentKind::Note;
        div()
            .id(("moment", gi * 10_000 + mi))
            .flex()
            .items_center()
            .gap_3()
            .min_h(px(40.))
            .py_1()
            .pl_3()
            .pr_1()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(theme::panel()))
            .child(ui::icon(ui::moment_icon(m.kind), 15., theme::moment(m.kind)))
            .child(div().w(px(52.)).flex_none().text_sm().text_color(theme::muted()).child(theme::fmt_clock(m.clock())))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(!is_note, |d| d.truncate().child(m.label.clone()))
                    .when(is_note, |d| d.child(ui::markdown(("moments-md", gi * 10_000 + mi), m.label.clone()))),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .text_sm()
                    .text_color(theme::muted())
                    .child(ui::icon(gpui_kit::assets::IconName::Play, 12., theme::muted()))
                    .child("Watch"),
            )
            .child(ui::star_button(("moment-star", gi * 10_000 + mi), m.saved).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_saved(gi, &star_m, cx);
            })))
            .on_click(cx.listener(move |this, _, window, cx| this.open_review(gi, Some(t), window, cx)))
            .into_any_element()
    }

    pub fn moments_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.moment_filter;
        let visible = self.visible_games();
        let per_game: Vec<(usize, Vec<Moment>)> = {
            let store = self.store.read(cx);
            visible.iter().map(|&i| (i, store::moments(&self.games[i].meta, store.game(&self.games[i])))).collect()
        };
        let count = |f: MomentFilter| -> usize { per_game.iter().map(|(_, ms)| ms.iter().filter(|m| f.matches(m)).count()).sum() };

        let entity = cx.entity().downgrade();
        let chips = MomentFilter::CHIPS.map(|f| {
            let entity = entity.clone();
            ui::filter_chip(("moment-filter", f as usize), f, kind, count(f), move |picked, _, cx| {
                if let Some(app) = entity.upgrade() {
                    app.update(cx, |app, cx| {
                        app.moment_filter = picked;
                        cx.notify();
                    });
                }
            })
        });

        // Everything shown, in list order, for "Play all".
        let queue: Vec<(usize, f64)> = per_game
            .iter()
            .flat_map(|(gi, ms)| ms.iter().filter(|m| kind.matches(m)).map(move |m| (*gi, m.video_time)))
            .collect();
        let title = match kind {
            MomentFilter::All => "All moments".to_string(),
            f => f.label().to_string(),
        };
        let header = ui::page_header()
            .child(self.games_view_switch(cx))
            .child(ui::drag_area())
            .when(queue.len() > 1, |d| {
                let n = queue.len();
                d.child(
                    ui::solid_button("moments-play-all", theme::raised())
                        .child(ui::icon(gpui_kit::assets::IconName::ListVideo, 14., theme::text()))
                        .child(format!("Play all {n}"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_playlist(title.clone(), queue.clone(), window, cx)
                        })),
                )
            });
        let filters = div()
            .flex()
            .flex_col()
            .gap_3()
            .px_6()
            .py_3()
            .child(self.filter_bar(true, cx))
            .child(div().flex().flex_wrap().gap_1p5().children(chips));

        let mut rows: Vec<AnyElement> = vec![];
        for (gi, moments) in &per_game {
            let g = &self.games[*gi];
            let moments: Vec<&Moment> = moments.iter().filter(|m| kind.matches(m)).collect();
            if moments.is_empty() {
                continue;
            }
            let champ = g.meta.champion.clone().unwrap_or_else(|| "Manual recording".into());
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .pt_5()
                    .pb_1p5()
                    .pl_3()
                    .child(self.portrait(g.meta.player.as_ref().map(|p| p.champion_key.as_str()), &champ, 22.))
                    .child(div().font_weight(FontWeight::MEDIUM).child(champ))
                    .children(store::matchup_label(&g.meta).and_then(|l| l.split_once(" vs ").map(|(_, vs)| {
                        div().text_sm().text_color(theme::muted()).child(format!("vs {vs}"))
                    })))
                    .child(div().text_sm().text_color(theme::muted()).child(queue_label(&g.meta)))
                    .child(div().text_sm().text_color(theme::muted()).child(g.meta.started_at.format("%b %-d, %H:%M").to_string()))
                    .into_any_element(),
            );
            for (mi, m) in moments.into_iter().enumerate() {
                rows.push(self.moment_row(*gi, mi, m, cx));
            }
        }

        let body = if rows.is_empty() {
            let (title, text) = match kind {
                MomentFilter::Saved => (
                    "No saved moments",
                    "Star a kill, a death or one of your notes to keep it here. Saved moments also show up under their matchup.",
                ),
                _ if self.filter.is_narrowed() => ("Nothing matches", "No moments in games that match these filters."),
                _ => (
                    "No moments yet",
                    "Kills, deaths, assists and objectives from your recorded matches collect here, along with any notes you write while reviewing.",
                ),
            };
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_lg().font_weight(FontWeight::MEDIUM).child(title))
                .child(div().max_w(px(420.)).text_center().text_color(theme::muted()).child(text))
                .into_any_element()
        } else {
            div().id("moments").flex_1().overflow_y_scroll().px_3().pb_6().flex().flex_col().children(rows).into_any_element()
        };

        div().size_full().flex().flex_col().child(header).child(filters).child(body)
    }
}
