//! Home: the one screen you need after a game. Your last match with one
//! click to its deaths or kills, recent clips, recent games — or, on first
//! launch, how to get going.

use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use timewarp_engine::Phase;

use crate::store;
use crate::ui::{self, MomentFilter, icon};
use crate::{Page, TimewarpApp, theme, timeline};

impl TimewarpApp {
    fn last_game_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let g = self.games.first()?;
        let m = &g.meta;
        let moments = store::moments(m, self.store.read(cx).game(g));
        let of = |f: MomentFilter| -> Vec<(usize, f64)> {
            moments.iter().filter(|mo| f.matches(mo)).map(|mo| (0, mo.video_time)).collect()
        };
        let (deaths, kills, saved) = (of(MomentFilter::Deaths), of(MomentFilter::Kills), of(MomentFilter::Saved));
        let champ = m.champion.clone().unwrap_or_else(|| "Manual recording".into());
        let (result, rcolor) = match m.result.as_deref() {
            Some("Win") => ("Victory", theme::victory()),
            Some("Lose") => ("Defeat", theme::defeat()),
            _ => ("", theme::muted()),
        };
        let entity = cx.entity().downgrade();

        let action = |id: &'static str, icon_name: IconName, label: String, color: Hsla, items: Vec<(usize, f64)>, title: &'static str| {
            ui::solid_button(id, theme::raised())
                .h(px(36.))
                .px_4()
                .child(icon(icon_name, 14., color))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.start_playlist(title.into(), items.clone(), window, cx)))
        };

        Some(
            div()
                .flex()
                .flex_col()
                .gap_4()
                .p_5()
                .rounded_xl()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::line())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_4()
                        .child(self.portrait(m.player.as_ref().map(|p| p.champion_key.as_str()), &champ, 56.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(div().text_sm().text_color(theme::muted()).child(format!(
                                    "Last game, {}",
                                    m.started_at.format("%A %H:%M")
                                )))
                                .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(champ.clone()))
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .text_sm()
                                        .when(!result.is_empty(), |d| d.child(div().text_color(rcolor).child(result)))
                                        .child(div().text_color(theme::muted()).child(crate::games::queue_label(m)))
                                        .children(m.opponent.as_ref().map(|o| div().text_color(theme::muted()).child(format!("vs {}", o.champion)))),
                                ),
                        )
                        .child(div().flex_1())
                        .children(m.player.as_ref().map(|p| {
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .child(
                                    div()
                                        .flex()
                                        .gap_1p5()
                                        .text_2xl()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(p.kills.to_string())
                                        .child(div().text_color(theme::muted()).child("/"))
                                        .child(div().text_color(theme::death()).child(p.deaths.to_string()))
                                        .child(div().text_color(theme::muted()).child("/"))
                                        .child(p.assists.to_string()),
                                )
                                .child(div().text_sm().text_color(theme::muted()).child(format!("{} CS", p.cs)))
                        })),
                )
                .child(timeline::mini_strip("home-strip", &moments, m.duration_secs, {
                    let entity = entity.clone();
                    move |t, window, cx| {
                        if let Some(app) = entity.upgrade() {
                            app.update(cx, |app, cx| app.open_review(0, Some(t), window, cx));
                        }
                    }
                }))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .when(!deaths.is_empty(), |d| {
                            d.child(action("home-deaths", IconName::Skull, format!("Watch deaths ({})", deaths.len()), theme::death(), deaths, "Deaths"))
                        })
                        .when(!kills.is_empty(), |d| {
                            d.child(action("home-kills", IconName::Swords, format!("Watch kills ({})", kills.len()), theme::kill(), kills, "Kills"))
                        })
                        .when(!saved.is_empty(), |d| {
                            d.child(action("home-saved", IconName::Star, format!("Saved ({})", saved.len()), theme::kill(), saved, "Saved"))
                        })
                        .child(
                            ui::solid_button("home-review", theme::victory())
                                .h(px(36.))
                                .px_4()
                                .text_color(theme::bg())
                                .child(icon(IconName::Play, 14., theme::bg()))
                                .child("Review game")
                                .on_click(cx.listener(|this, _, window, cx| this.open_review(0, None, window, cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    fn live_banner(&self) -> Option<AnyElement> {
        let s = &self.status;
        let (title, detail) = match s.phase {
            Phase::Recording => (
                format!("Recording {}", theme::fmt_clock(s.elapsed.as_secs_f64())),
                match self.config.bookmark_key.as_deref() {
                    Some(k) => format!("Press {k} in game to bookmark a moment. It shows up here when the game ends."),
                    None => "It shows up here when the game ends.".into(),
                },
            ),
            Phase::WaitingForWindow => ("Match starting".into(), "Recording begins when the game window opens.".into()),
            Phase::Armed => ("Waiting for League".into(), "Recording starts when your game opens. Only League is recorded.".into()),
            Phase::Finalizing => ("Saving your game".into(), "Almost done.".into()),
            _ => return None,
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_5()
                .py_4()
                .rounded_xl()
                .bg(theme::live().opacity(0.1))
                .border_1()
                .border_color(theme::live().opacity(0.4))
                .child(div().size(px(10.)).rounded_full().bg(theme::live()))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_sm().text_color(theme::muted()).child(detail))
                .into_any_element(),
        )
    }

    /// One plain sentence about recent form, from games with known results.
    fn form_line(&self) -> Option<String> {
        let recent: Vec<_> = self.games.iter().take(10).filter(|g| g.meta.result.is_some()).collect();
        if recent.len() < 2 {
            return None;
        }
        let wins = recent.iter().filter(|g| g.meta.result.as_deref() == Some("Win")).count();
        let with_stats: Vec<_> = recent.iter().filter_map(|g| g.meta.player.as_ref()).collect();
        let deaths = if with_stats.is_empty() {
            0.0
        } else {
            with_stats.iter().map(|p| p.deaths as f64).sum::<f64>() / with_stats.len() as f64
        };
        Some(format!(
            "Last {} games: {}W {}L, {:.1} deaths per game.",
            recent.len(),
            wins,
            recent.len() - wins,
            deaths
        ))
    }

    fn section_title(&self, title: &str, link: Option<(&'static str, Page)>, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .pt_8()
            .pb_2()
            .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
            .children(link.map(|(label, page)| {
                ui::ghost_button(SharedString::from(format!("see-{label}")))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.go(page, cx)))
            }))
    }

    fn onboarding(&self) -> impl IntoElement {
        let key = self.config.bookmark_key.clone().unwrap_or_else(|| "your bookmark key".into());
        let steps = [
            ("Play a match", "Timewarp starts recording when the game window opens and saves when the match ends. Nothing is drawn over the game."),
            ("Bookmark what matters", "Press the key below in game to flag a moment. Kills, deaths, assists and objectives are tagged for you."),
            ("Review it here", "Your last game shows up on this page with one click to watch your deaths or kills."),
        ];
        div()
            .max_w(px(620.))
            .flex()
            .flex_col()
            .gap_5()
            .pt_10()
            .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("Welcome to Timewarp"))
            .child(div().text_color(theme::muted()).child(
                "Record your League games, find the moments that matter, and keep only those. Three things to know:",
            ))
            .children(steps.iter().enumerate().map(|(i, (title, body))| {
                div()
                    .flex()
                    .gap_4()
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .rounded_full()
                            .bg(theme::raised())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child((i + 1).to_string()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(div().font_weight(FontWeight::MEDIUM).child(title.to_string()))
                            .child(div().text_sm().text_color(theme::muted()).child(body.to_string())),
                    )
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(theme::muted())
                    .child("Bookmark key:")
                    .child(ui::kbd(Box::leak(key.into_boxed_str())))
                    .child("Change it, and what gets recorded, in Settings."),
            )
    }

    pub fn home_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let header = ui::page_header().child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Home")).child(ui::drag_area());

        let mut body = div().id("home").flex_1().overflow_y_scroll().px_6().pb_10().flex().flex_col();
        if let Some(b) = self.live_banner() {
            body = body.child(div().pt_5().child(b));
        }
        if self.games.is_empty() {
            body = body.child(self.onboarding());
        } else {
            body = body.children(self.last_game_card(cx).map(|c| div().pt_5().child(c)));
            if let Some(line) = self.form_line() {
                body = body.child(div().pt_3().px_1().text_sm().text_color(theme::muted()).child(line));
            }
            #[cfg(target_os = "macos")]
            if !self.clips.is_empty() {
                body = body.child(self.section_title("Recent clips", Some(("All clips", Page::Clips)), cx)).child(
                    div()
                        .id("home-clips")
                        .flex()
                        .gap_2()
                        .ml(px(-8.))
                        .overflow_x_scroll()
                        .children(self.clips.iter().take(4).enumerate().map(|(i, c)| self.clip_card(i, c, cx)).collect::<Vec<_>>()),
                );
            }
            if self.games.len() > 1 {
                body = body
                    .child(self.section_title("Earlier games", Some(("All games", Page::Games)), cx))
                    .children((1..self.games.len().min(6)).map(|i| self.game_row(i, cx)).collect::<Vec<_>>());
            }
        }
        div().size_full().flex().flex_col().child(header).child(body)
    }
}
