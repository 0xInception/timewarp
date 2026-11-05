//! One filter shared by Games, Moments and Matchups: result, your role, your
//! champion, and any number of "ally/enemy <role>: <champion>" conditions —
//! e.g. as a jungler, only games where the enemy top laner was Darius.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui_kit::component::popover::Popover;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use timewarp_engine::SessionMeta;

use crate::{Game, ResultFilter, TimewarpApp, theme, ui};

pub const ROLES: [&str; 5] = ["TOP", "JUNGLE", "MIDDLE", "BOTTOM", "UTILITY"];

pub fn role_label(role: &str) -> &'static str {
    match role {
        "TOP" => "Top",
        "JUNGLE" => "Jungle",
        "MIDDLE" => "Mid",
        "BOTTOM" => "Bot",
        "UTILITY" => "Support",
        _ => "No role",
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    Enemy,
    Ally,
}

impl Side {
    pub fn label(self) -> &'static str {
        match self {
            Side::Enemy => "Enemy",
            Side::Ally => "Ally",
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Condition {
    pub side: Side,
    pub role: String,
    pub champion_key: String,
    pub champion: String,
}

impl Condition {
    pub fn label(&self) -> String {
        format!("{} {}: {}", self.side.label(), role_label(&self.role), self.champion)
    }
}

#[derive(Clone)]
pub struct GameFilter {
    pub result: ResultFilter,
    /// Your role (TOP/JUNGLE/...).
    pub role: Option<String>,
    /// Your champion (Data Dragon key).
    pub champion: Option<String>,
    pub conditions: Vec<Condition>,
}

impl Default for GameFilter {
    fn default() -> Self {
        Self { result: ResultFilter::All, role: None, champion: None, conditions: vec![] }
    }
}

impl GameFilter {
    pub fn is_narrowed(&self) -> bool {
        self.role.is_some() || self.champion.is_some() || !self.conditions.is_empty()
    }

    pub fn matches(&self, m: &SessionMeta) -> bool {
        let r = m.result.as_deref();
        let result_ok = match self.result {
            ResultFilter::All => true,
            ResultFilter::Wins => r == Some("Win"),
            ResultFilter::Losses => r == Some("Lose"),
        };
        result_ok
            && self.role.as_ref().is_none_or(|role| my_role(m) == Some(role.as_str()))
            && self.champion.as_ref().is_none_or(|c| m.player.as_ref().is_some_and(|p| &p.champion_key == c))
            && self.conditions.iter().all(|c| {
                m.roster.iter().any(|p| {
                    !p.me && p.ally == (c.side == Side::Ally) && p.position == c.role && p.champion_key == c.champion_key
                })
            })
    }
}

/// Dev aid for screenshots: TIMEWARP_OPEN=<picker id> renders it open.
fn dev_open(id: &str) -> bool {
    std::env::var("TIMEWARP_OPEN").is_ok_and(|v| v == id)
}

pub fn my_role(m: &SessionMeta) -> Option<&str> {
    m.player.as_ref().map(|p| p.position.as_str()).filter(|p| ROLES.contains(p))
}

/// Champions seen in a roster slot across games, with games/wins, most played first.
pub fn slot_champions(games: &[&Game], side: Side, role: &str) -> Vec<(String, String, usize, usize)> {
    let mut seen: BTreeMap<String, (String, usize, usize)> = BTreeMap::new();
    for g in games {
        let win = g.meta.result.as_deref() == Some("Win");
        for p in g.meta.roster.iter().filter(|p| !p.me && p.ally == (side == Side::Ally) && p.position == role) {
            let e = seen.entry(p.champion_key.clone()).or_insert((p.champion.clone(), 0, 0));
            e.1 += 1;
            e.2 += win as usize;
        }
    }
    let mut out: Vec<_> = seen.into_iter().map(|(k, (name, n, w))| (k, name, n, w)).collect();
    out.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
    out
}

// ---------------------------------------------------------------------------
// Filter bar
// ---------------------------------------------------------------------------

type Pick = Rc<dyn Fn(&mut TimewarpApp, Option<(String, String)>, &mut Context<TimewarpApp>)>;

impl TimewarpApp {
    fn set_filter(&mut self, f: impl FnOnce(&mut GameFilter), cx: &mut Context<Self>) {
        f(&mut self.filter);
        cx.notify();
    }

    /// Dropdown listing `options` (value, label, games). `None` value = "Any".
    fn picker(
        &self,
        id: &'static str,
        title: String,
        active: bool,
        options: Vec<(Option<String>, String, Option<usize>)>,
        on_pick: Pick,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let app = cx.entity().downgrade();
        let trigger = ui::PickerTrigger::new(id, title, active);
        Popover::new(SharedString::from(format!("{id}-popover")))
            .default_open(dev_open(id))
            .trigger(trigger)
            .content(move |_, _, cx| {
                let app = app.clone();
                div()
                    .id("picker-list")
                    .w(px(220.))
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .p_1()
                    .children(options.iter().enumerate().map(|(i, (value, label, n))| {
                        let (app, value, on_pick, label_s) = (app.clone(), value.clone(), on_pick.clone(), label.clone());
                        let popover = cx.entity().downgrade();
                        div()
                            .id(("opt", i))
                            .flex()
                            .items_center()
                            .h(px(30.))
                            .px_2()
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .hover(|s| s.bg(theme::raised()))
                            .child(div().flex_1().child(label.clone()))
                            .children(n.map(|n| div().text_color(theme::muted()).child(n.to_string())))
                            .on_click(move |_, window, cx| {
                                if let Some(app) = app.upgrade() {
                                    let pick = value.clone().map(|v| (v, label_s.clone()));
                                    app.update(cx, |a, cx| on_pick(a, pick, cx));
                                }
                                if let Some(p) = popover.upgrade() {
                                    p.update(cx, |p, cx| p.dismiss(window, cx));
                                }
                            })
                    }))
            })
    }

    /// Picker for a new "ally/enemy <role>: <champion>" condition.
    fn add_condition_picker(&self, games: &[&Game], cx: &mut Context<Self>) -> impl IntoElement {
        let app = cx.entity().downgrade();
        // Pre-compute every slot's champions so the popover can switch instantly.
        let mut slots: BTreeMap<(Side, &'static str), Vec<(String, String, usize, usize)>> = BTreeMap::new();
        for side in [Side::Enemy, Side::Ally] {
            for role in ROLES {
                slots.insert((side, role), slot_champions(games, side, role));
            }
        }
        let draft = self.condition_draft;
        let trigger = ui::PickerTrigger::new("add-condition", "Ally or enemy champion", false);
        Popover::new("add-condition-popover").default_open(dev_open("add-condition")).trigger(trigger).content(move |_, _, cx| {
            let popover = cx.entity().downgrade();
            let (side, role) = draft;
            let champs = slots.get(&(side, role)).cloned().unwrap_or_default();
            let set_draft = |side: Side, role: &'static str| {
                let app = app.clone();
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    if let Some(app) = app.upgrade() {
                        app.update(cx, |a, cx| {
                            a.condition_draft = (side, role);
                            cx.notify();
                        });
                    }
                }
            };
            let tab = |id: SharedString, label: &'static str, on: bool| {
                div()
                    .id(id)
                    .px_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .text_color(if on { theme::text() } else { theme::muted() })
                    .when(on, |d| d.bg(theme::raised()))
                    .child(label)
            };
            div()
                .w(px(300.))
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .children([Side::Enemy, Side::Ally].map(|s| {
                            tab(format!("side-{}", s.label()).into(), s.label(), s == side).on_click(set_draft(s, role))
                        })),
                )
                .child(div().flex().flex_wrap().gap_1().children(ROLES.map(|r| {
                    tab(format!("role-{r}").into(), role_label(r), r == role).on_click(set_draft(side, r))
                })))
                .child(div().h(px(1.)).bg(theme::line()))
                .child(
                    div()
                        .id("slot-champs")
                        .max_h(px(300.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .when(champs.is_empty(), |d| {
                            d.child(div().p_2().text_sm().text_color(theme::muted()).child(format!(
                                "No recorded games with a known {} {} yet.",
                                side.label().to_lowercase(),
                                role_label(role).to_lowercase()
                            )))
                        })
                        .children(champs.into_iter().enumerate().map(|(i, (key, name, n, wins))| {
                            let (app, popover) = (app.clone(), popover.clone());
                            div()
                                .id(("slot-champ", i))
                                .flex()
                                .items_center()
                                .gap_2()
                                .h(px(30.))
                                .px_2()
                                .rounded_md()
                                .cursor_pointer()
                                .text_sm()
                                .hover(|s| s.bg(theme::raised()))
                                .child(div().flex_1().child(name.clone()))
                                .child(div().text_color(theme::muted()).child(format!("{wins}W {}L", n - wins)))
                                .on_click(move |_, window, cx| {
                                    let cond = Condition {
                                        side,
                                        role: role.to_string(),
                                        champion_key: key.clone(),
                                        champion: name.clone(),
                                    };
                                    if let Some(app) = app.upgrade() {
                                        app.update(cx, |a, cx| {
                                            if !a.filter.conditions.contains(&cond) {
                                                a.filter.conditions.push(cond);
                                            }
                                            cx.notify();
                                        });
                                    }
                                    if let Some(p) = popover.upgrade() {
                                        p.update(cx, |p, cx| p.dismiss(window, cx));
                                    }
                                })
                        })),
                )
        })
    }

    /// Role / champion / conditions row. `show_result` adds All/Wins/Losses.
    pub fn filter_bar(&self, show_result: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let f = &self.filter;
        let all: Vec<&Game> = self.games.iter().collect();

        // Your roles and champions, with game counts.
        let mut roles: BTreeMap<&str, usize> = BTreeMap::new();
        let mut champs: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for g in &all {
            if let Some(r) = my_role(&g.meta) {
                *roles.entry(r).or_default() += 1;
            }
            if let Some(p) = &g.meta.player {
                champs.entry(p.champion_key.clone()).or_insert((p.champion.clone(), 0)).1 += 1;
            }
        }
        let mut role_opts = vec![(None, "Any role".to_string(), None)];
        role_opts.extend(ROLES.iter().filter_map(|r| roles.get(r).map(|n| (Some(r.to_string()), role_label(r).to_string(), Some(*n)))));
        let mut champ_list: Vec<_> = champs.into_iter().collect();
        champ_list.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(a.1.0.cmp(&b.1.0)));
        let mut champ_opts = vec![(None, "Any champion".to_string(), None)];
        champ_opts.extend(champ_list.into_iter().map(|(k, (name, n))| (Some(k), name, Some(n))));

        let role_title = f.role.as_deref().map(|r| format!("As {}", role_label(r))).unwrap_or_else(|| "Any role".into());
        let champ_title = f
            .champion
            .as_ref()
            .map(|k| {
                self.games
                    .iter()
                    .find_map(|g| g.meta.player.as_ref().filter(|p| &p.champion_key == k).map(|p| format!("Playing {}", p.champion)))
                    .unwrap_or_else(|| k.clone())
            })
            .unwrap_or_else(|| "Any champion".into());

        let pick_role: Pick = Rc::new(|a, v, cx| a.set_filter(|f| f.role = v.map(|(v, _)| v), cx));
        let pick_champ: Pick = Rc::new(|a, v, cx| a.set_filter(|f| f.champion = v.map(|(v, _)| v), cx));

        let result_tab = |r: ResultFilter, label: &'static str| {
            let active = f.result == r;
            div()
                .id(label)
                .h(px(24.))
                .px_2p5()
                .flex()
                .items_center()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if active { theme::text() } else { theme::muted() })
                .when(active, |d| d.bg(theme::raised()))
                .when(!active, |d| d.hover(|s| s.text_color(theme::text())))
                .child(label)
                .on_click(cx.listener(move |a, _, _, cx| a.set_filter(|f| f.result = r, cx)))
        };

        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .when(show_result, |d| {
                d.child(
                    div()
                        .flex()
                        .p(px(2.))
                        .rounded_lg()
                        .border_1()
                        .border_color(theme::line())
                        .child(result_tab(ResultFilter::All, "All"))
                        .child(result_tab(ResultFilter::Wins, "Wins"))
                        .child(result_tab(ResultFilter::Losses, "Losses")),
                )
            })
            .child(self.picker("role", role_title, f.role.is_some(), role_opts, pick_role, cx))
            .child(self.picker("champ", champ_title, f.champion.is_some(), champ_opts, pick_champ, cx))
            .children(f.conditions.iter().enumerate().map(|(i, c)| {
                div()
                    .id(("cond", i))
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(24.))
                    .pl_2p5()
                    .pr_1()
                    .rounded_md()
                    .bg(theme::raised())
                    .border_1()
                    .border_color(if c.side == Side::Enemy { theme::death().opacity(0.5) } else { theme::victory().opacity(0.5) })
                    .text_sm()
                    .child(c.label())
                    .child(
                        div()
                            .id(("cond-x", i))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::line()))
                            .child(ui::icon(gpui_kit::assets::IconName::X, 11., theme::muted()))
                            .on_click(cx.listener(move |a, _, _, cx| a.set_filter(|f| { f.conditions.remove(i); }, cx))),
                    )
            }))
            .child(self.add_condition_picker(&all, cx))
            .when(f.is_narrowed(), |d| {
                d.child(
                    ui::ghost_button("clear-filters")
                        .child("Clear")
                        .on_click(cx.listener(|a, _, _, cx| {
                            a.set_filter(|f| *f = GameFilter { result: f.result, ..Default::default() }, cx)
                        })),
                )
            })
    }
}
