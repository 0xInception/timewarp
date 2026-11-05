//! The match strip: the one visual idea the whole app is built around.
//! A game is a line; kills, deaths, assists and objectives are ticks on it.
//! Small in the library (click a tick to jump there), large in review
//! (a scrubber with a playhead).

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder;
use crate::store::{Moment, MomentKind};
use timewarp_league::MarkerKind;

use crate::theme;

/// Compact strip for a library row. `on_jump(video_time)` fires when a tick
/// is clicked.
pub fn mini_strip(
    id: impl Into<ElementId>,
    moments: &[Moment],
    duration: f64,
    on_jump: impl Fn(f64, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_jump = Rc::new(on_jump);
    let duration = duration.max(1.0);
    div()
        .id(id)
        .relative()
        .h(px(22.))
        .w_full()
        .child(div().absolute().left_0().right_0().top(px(10.)).h(px(2.)).rounded_full().bg(theme::line()))
        .children(moments.iter().enumerate().map(
            |(i, m)| {
                let frac = (m.video_time / duration).clamp(0.0, 1.0) as f32;
                let color = theme::moment(m.kind);
                let t = m.video_time;
                let on_jump = on_jump.clone();
                let tall = m.saved
                    || matches!(m.kind, MomentKind::Event(MarkerKind::Multikill | MarkerKind::Ace));
                // 10px hit area around a 3px tick.
                div()
                    .id(("tick", i))
                    .absolute()
                    .left(relative(frac))
                    .ml(px(-5.))
                    .top_0()
                    .w(px(10.))
                    .h_full()
                    .flex()
                    .justify_center()
                    .items_center()
                    .cursor_pointer()
                    .child(div().w(px(3.)).h(px(if tall { 20. } else { 12. })).rounded_full().bg(color))
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        on_jump(t, window, cx)
                    })
            },
        ))
}

/// Which lane a moment sits in on the review timeline.
fn lane(kind: MomentKind) -> usize {
    match kind {
        MomentKind::Event(MarkerKind::Kill | MarkerKind::Multikill | MarkerKind::Ace) => 0,
        MomentKind::Event(MarkerKind::Assist) => 1,
        MomentKind::Event(MarkerKind::Death) => 2,
        MomentKind::Event(MarkerKind::Objective | MarkerKind::Structure | MarkerKind::GameEnd) => 3,
        MomentKind::Note | MomentKind::Event(MarkerKind::Bookmark) => 4,
    }
}

thread_local! {
    /// Last drawn width of the review timeline, for spacing ruler labels and
    /// grouping bursts before this frame's layout is known.
    static LAST_WIDTH: Cell<f32> = const { Cell::new(1000.0) };
}

const LANES: usize = 5;
const LANE_H: f32 = 8.;
const LANE_GAP: f32 = 4.;
const RULER_H: f32 = 16.;
const TRACK_H: f32 = 3.;
/// Ticks closer than this (px) in one lane draw as a single burst pill.
const BURST_PX: f32 = 7.;

/// Seconds between ruler labels so they sit at least ~80px apart.
fn ruler_step(span: f64, width_px: f32) -> f64 {
    let want = span * 80.0 / f64::from(width_px.max(200.0));
    [5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0]
        .into_iter()
        .find(|s| *s >= want)
        .unwrap_or(3600.0)
}

/// Review timeline: moments in lanes (kills, assists, deaths, objectives,
/// notes), a minute ruler, a playhead, zoom (⌘/Ctrl-scroll) and pan
/// (scroll). `view` is the part of the game shown; when zoomed in, a thin
/// overview of the whole game sits on top with the visible window marked.
#[allow(clippy::too_many_arguments)]
pub fn scrubber(
    moments: &[Moment],
    visible: impl Fn(&Moment) -> bool,
    duration: f64,
    position: f64,
    clip: Option<(f64, f64)>,
    hover: Option<f64>,
    // Picture at the hover point (platform player renders it).
    thumb: Option<AnyElement>,
    view: (f64, f64),
    on_seek: impl Fn(f64, &mut Window, &mut App) + 'static,
    on_hover: impl Fn(Option<f64>, &mut Window, &mut App) + 'static,
    on_view: impl Fn(f64, f64, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let duration = duration.max(1.0);
    let (vs, ve) = (view.0.max(0.0), view.1.min(duration).max(view.0 + 1.0));
    let span = ve - vs;
    let zoomed = span < duration - 0.5;
    let frac = move |t: f64| ((t - vs) / span) as f32;

    let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
    let on_seek = Rc::new(on_seek);
    let on_hover = Rc::new(on_hover);
    let mut thumb = thumb;
    let on_view = Rc::new(on_view);
    let time_at = {
        let bounds = bounds.clone();
        move |x: Pixels| -> Option<f64> {
            let b = bounds.get();
            (b.size.width > px(0.)).then(|| vs + ((x - b.origin.x) / b.size.width).clamp(0.0, 1.0) as f64 * span)
        }
    };
    let width_px = LAST_WIDTH.with(|w| w.get());
    let lanes_top = RULER_H;
    let lanes_h = LANES as f32 * LANE_H + (LANES as f32 - 1.) * LANE_GAP;
    let track_top = lanes_top + lanes_h + 6.;
    let height = track_top + TRACK_H + 6.;

    // Ruler labels.
    let step = ruler_step(span, width_px);
    let first = (vs / step).ceil() * step;
    let mut ruler: Vec<AnyElement> = vec![];
    let mut t = first;
    while t <= ve {
        let f = frac(t);
        ruler.push(div().absolute().left(relative(f)).top(px(lanes_top)).w(px(1.)).h(px(lanes_h)).bg(theme::line().opacity(0.5)).into_any_element());
        // Labels near the right edge would be cut off; the gridline is enough there.
        if f < 0.94 {
            ruler.push(
                div()
                    .absolute()
                    .left(relative(f))
                    .ml(px(3.))
                    .top_0()
                    .text_xs()
                    .whitespace_nowrap()
                    .text_color(theme::muted())
                    .child(crate::theme::fmt_clock(t))
                    .into_any_element(),
            );
        }
        t += step;
    }

    // Lanes: faint track per lane, then ticks/bursts.
    let mut lane_els: Vec<AnyElement> = vec![];
    for l in 0..LANES {
        let top = lanes_top + l as f32 * (LANE_H + LANE_GAP);
        lane_els.push(
            div().absolute().left_0().right_0().top(px(top)).h(px(LANE_H)).rounded_sm().bg(theme::line().opacity(0.25)).into_any_element(),
        );
        let mut in_lane: Vec<&Moment> = moments
            .iter()
            .filter(|m| lane(m.kind) == l && visible(m) && m.video_time >= vs - 1.0 && m.video_time <= ve + 1.0)
            .collect();
        in_lane.sort_by(|a, b| a.video_time.total_cmp(&b.video_time));
        // Group into bursts using the last known width in px.
        let w = f64::from(width_px.max(200.0));
        let mut groups: Vec<(f64, f64, usize, Hsla, bool)> = vec![];
        for m in in_lane {
            let x = (m.video_time - vs) / span * w;
            let big = m.saved || m.big;
            match groups.last_mut() {
                Some(g) if x - g.1 < f64::from(BURST_PX) => {
                    g.1 = x;
                    g.2 += 1;
                    g.4 |= big;
                }
                _ => groups.push((x, x, 1, theme::moment(m.kind), big)),
            }
        }
        for (x0, x1, n, color, big) in groups {
            let (f0, f1) = ((x0 / w) as f32, (x1 / w) as f32);
            let h = if big { LANE_H + 6. } else { LANE_H };
            let y = top - (h - LANE_H) / 2.;
            let el = if n == 1 {
                div().absolute().left(relative(f0)).ml(px(-1.5)).top(px(y)).w(px(3.)).h(px(h)).rounded_full().bg(color)
            } else {
                // A burst (teamfight, multikill): one pill from first to last.
                div()
                    .absolute()
                    .left(relative(f0))
                    .ml(px(-2.))
                    .w(relative((f1 - f0).max(0.0)))
                    .min_w(px(4.))
                    .pr(px(4.))
                    .top(px(y))
                    .h(px(h))
                    .rounded_full()
                    .bg(color.opacity(0.85))
            };
            lane_els.push(el.into_any_element());
        }
    }

    let played = frac(position).clamp(0.0, 1.0);
    let playhead_visible = position >= vs && position <= ve;

    let main = div()
        .id("scrubber")
        .relative()
        .w_full()
        .h(px(height))
        .cursor_pointer()
        .child({
            let bounds = bounds.clone();
            canvas(
                move |b, _, _| {
                    LAST_WIDTH.with(|w| w.set(f32::from(b.size.width)));
                    bounds.set(b)
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full()
        })
        .children(ruler)
        // clip range (I/O)
        .children(clip.map(|(a, b)| {
            let (fa, fb) = (frac(a).clamp(0.0, 1.0), frac(b).clamp(0.0, 1.0));
            div()
                .absolute()
                .left(relative(fa))
                .w(relative((fb - fa).max(0.002)))
                .top(px(lanes_top - 2.))
                .h(px(lanes_h + 4.))
                .rounded_sm()
                .bg(theme::victory().opacity(0.18))
                .border_1()
                .border_color(theme::victory().opacity(0.7))
        }))
        .children(lane_els)
        // progress track
        .child(div().absolute().left_0().right_0().top(px(track_top)).h(px(TRACK_H)).rounded_full().bg(theme::line()))
        .child(div().absolute().left_0().w(relative(played)).top(px(track_top)).h(px(TRACK_H)).rounded_full().bg(theme::accent()))
        // playhead
        .when(playhead_visible, |d| {
            d.child(div().absolute().left(relative(played)).ml(px(-1.)).top(px(lanes_top - 3.)).w(px(2.)).h(px(track_top - lanes_top + 6.)).bg(theme::text()))
                .child(
                    div()
                        .absolute()
                        .left(relative(played))
                        .ml(px(-6.))
                        .top(px(track_top - 4.5))
                        .size(px(12.))
                        .rounded_full()
                        .bg(theme::text())
                        .border_2()
                        .border_color(theme::bg()),
                )
        })
        .on_mouse_down(MouseButton::Left, {
            let (time_at, on_seek) = (time_at.clone(), on_seek.clone());
            move |ev, window, cx| {
                if let Some(t) = time_at(ev.position.x) {
                    on_seek(t, window, cx)
                }
            }
        })
        .on_mouse_move({
            let (time_at, on_seek, on_hover) = (time_at.clone(), on_seek.clone(), on_hover.clone());
            move |ev, window, cx| {
                let Some(t) = time_at(ev.position.x) else { return };
                if ev.pressed_button == Some(MouseButton::Left) {
                    on_seek(t, window, cx)
                } else {
                    on_hover(Some(t), window, cx)
                }
            }
        })
        .on_hover({
            let on_hover = on_hover.clone();
            move |hovered, window, cx| {
                if !*hovered {
                    on_hover(None, window, cx)
                }
            }
        })
        // ⌘/Ctrl-scroll zooms around the cursor; scroll pans when zoomed in.
        .on_scroll({
            let (bounds, on_view) = (bounds.clone(), on_view.clone());
            move |ev, window, cx| {
                let b = bounds.get();
                if b.size.width <= px(0.) {
                    return;
                }
                let d = ev.delta.pixel_delta(px(16.));
                let (dx, dy) = (f64::from(f32::from(d.x)), f64::from(f32::from(d.y)));
                let w = f64::from(f32::from(b.size.width));
                if ev.modifiers.platform || ev.modifiers.control {
                    let anchor = vs + f64::from((ev.position.x - b.origin.x) / b.size.width).clamp(0.0, 1.0) * span;
                    let new_span = (span * (dy * 0.01).exp()).clamp(20.0_f64.min(duration), duration);
                    let start = (anchor - (anchor - vs) * new_span / span).clamp(0.0, duration - new_span);
                    on_view(start, start + new_span, window, cx);
                } else if zoomed {
                    let delta = if dx.abs() > dy.abs() { dx } else { dy };
                    let start = (vs - delta / w * span).clamp(0.0, duration - span);
                    on_view(start, start + span, window, cx);
                }
            }
        })
        // hover preview
        .children(hover.filter(|t| *t >= vs && *t <= ve).map(|t| {
            let f = frac(t);
            let near = moments
                .iter()
                .filter(|m| visible(m))
                .min_by(|a, b| (a.video_time - t).abs().total_cmp(&(b.video_time - t).abs()))
                .filter(|m| (m.video_time - t).abs() < span * 0.01 + 2.0);
            let label = match near {
                Some(m) => format!("{}  {}", crate::theme::fmt_clock(m.clock()), m.label),
                None => crate::theme::fmt_clock(t),
            };
            let anchor = f.clamp(0.08, 0.92);
            div()
                .absolute()
                .left_0()
                .top_0()
                .size_full()
                .child(div().absolute().left(relative(f)).top(px(lanes_top - 3.)).w(px(1.)).h(px(track_top - lanes_top + 6.)).bg(theme::text().opacity(0.45)))
                .child(
                    div()
                        .absolute()
                        .left(relative(anchor))
                        .ml(px(-130.))
                        .w(px(260.))
                        .bottom(px(height + 2.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .children(thumb.take().map(|t| {
                            div()
                                .w(px(192.))
                                .h(px(108.))
                                .rounded_md()
                                .overflow_hidden()
                                .bg(gpui_kit::black())
                                .border_1()
                                .border_color(theme::line())
                                .child(t)
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .px_2()
                                .py_0p5()
                                .rounded_md()
                                .bg(theme::raised())
                                .border_1()
                                .border_color(theme::line())
                                .text_xs()
                                .whitespace_nowrap()
                                .children(near.map(|m| div().size(px(6.)).rounded_full().bg(theme::moment(m.kind))))
                                .child(label),
                        ),
                )
        }));

    // Overview of the whole game when zoomed in; click to move the view.
    let overview = zoomed.then(|| {
        let ob: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
        let on_view = on_view.clone();
        let (fa, fb) = ((vs / duration) as f32, (ve / duration) as f32);
        div()
            .id("timeline-overview")
            .relative()
            .w_full()
            .h(px(10.))
            .mb_1()
            .cursor_pointer()
            .child({
                let ob = ob.clone();
                canvas(move |b, _, _| ob.set(b), |_, _, _, _| {}).absolute().size_full()
            })
            .child(div().absolute().left_0().right_0().top(px(4.)).h(px(2.)).bg(theme::line()))
            .children(moments.iter().filter(|m| visible(m)).map(|m| {
                div()
                    .absolute()
                    .left(relative((m.video_time / duration) as f32))
                    .top(px(3.))
                    .w(px(1.))
                    .h(px(4.))
                    .bg(theme::moment(m.kind).opacity(0.8))
            }))
            .child(
                div()
                    .absolute()
                    .left(relative(fa))
                    .w(relative((fb - fa).max(0.01)))
                    .top_0()
                    .h_full()
                    .rounded_sm()
                    .border_1()
                    .border_color(theme::accent())
                    .bg(theme::accent().opacity(0.15)),
            )
            .child(div().absolute().left(relative((position / duration) as f32)).top_0().w(px(1.)).h_full().bg(theme::text()))
            .on_mouse_down(MouseButton::Left, move |ev, window, cx| {
                let b = ob.get();
                if b.size.width > px(0.) {
                    let t = f64::from((ev.position.x - b.origin.x) / b.size.width).clamp(0.0, 1.0) * duration;
                    let start = (t - span / 2.0).clamp(0.0, duration - span);
                    on_view(start, start + span, window, cx);
                }
            })
    });

    div().flex().flex_col().w_full().children(overview).child(main)
}
