//! Rubber-band selection, the Finder/Explorer way: press in a list, drag, and
//! everything the rectangle touches is selected. Standard modifiers:
//! ⇧-drag adds to the selection, ⌘-drag (Ctrl on Windows) flips what it
//! touches. A click on empty space clears. The rectangle stays inside the
//! list it started in. Works on the Games list and the Clips grid.
//!
//! Each row/card drops an invisible probe that records its window bounds on
//! every frame; the drag tests the rectangle against those.

use std::collections::BTreeSet;
use std::path::PathBuf;

use gpui_kit::*;

use crate::{TimewarpApp, theme};

/// Mouse travel before a press becomes a drag (so clicks stay clicks).
const DRAG_THRESHOLD: f32 = 5.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SelectTarget {
    Games,
    Clips,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DragMode {
    /// Plain drag: the rectangle is the selection.
    Replace,
    /// ⇧-drag: add what the rectangle touches.
    Add,
    /// ⌘-drag: flip what the rectangle touches.
    Toggle,
}

pub struct Marquee {
    target: SelectTarget,
    start: Point<Pixels>,
    now: Point<Pixels>,
    active: bool,
    mode: DragMode,
    /// Selection when the drag started.
    base: BTreeSet<PathBuf>,
    /// The list's visible area; the rectangle never leaves it.
    area: Bounds<Pixels>,
}

impl Marquee {
    fn rect(&self) -> Bounds<Pixels> {
        let min = point(self.start.x.min(self.now.x), self.start.y.min(self.now.y));
        let max = point(self.start.x.max(self.now.x), self.start.y.max(self.now.y));
        Bounds::from_corners(min, max)
    }
}

impl TimewarpApp {
    /// Invisible layer recording the list's visible area (for clamping).
    pub fn area_probe(&self) -> impl IntoElement {
        let cell = self.list_area.clone();
        canvas(move |b, _, _| cell.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full()
    }

    /// Invisible child that records where `path`'s row/card is on screen.
    pub fn bounds_probe(&self, path: PathBuf) -> impl IntoElement {
        let map = self.item_bounds.clone();
        canvas(move |b, _, _| {
            map.borrow_mut().insert(path, b);
        }, |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    fn selection_mut(&mut self, target: SelectTarget) -> &mut BTreeSet<PathBuf> {
        match target {
            SelectTarget::Games => &mut self.selected,
            SelectTarget::Clips => &mut self.selected_clips,
        }
    }

    pub fn marquee_down(&mut self, target: SelectTarget, at: Point<Pixels>, modifiers: Modifiers) {
        self.suppress_click = false;
        let mode = if modifiers.platform || modifiers.control {
            DragMode::Toggle
        } else if modifiers.shift {
            DragMode::Add
        } else {
            DragMode::Replace
        };
        let base = self.selection_mut(target).clone();
        let area = self.list_area.get();
        self.marquee = Some(Marquee { target, start: at, now: at, active: false, mode, base, area });
    }

    pub fn marquee_move(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        // Keep the rectangle inside the list (its current visible area).
        let current = self.list_area.get();
        let Some(m) = &mut self.marquee else { return };
        if current.size.width > px(0.) {
            m.area = current;
        }
        let a = m.area;
        m.now = if a.size.width > px(0.) {
            point(at.x.clamp(a.origin.x, a.origin.x + a.size.width), at.y.clamp(a.origin.y, a.origin.y + a.size.height))
        } else {
            at
        };
        if !m.active {
            let (dx, dy) = (f32::from(m.now.x - m.start.x), f32::from(m.now.y - m.start.y));
            if (dx * dx + dy * dy).sqrt() < DRAG_THRESHOLD {
                return;
            }
            m.active = true;
            // The press turned into a drag: the release must not open anything.
            self.suppress_click = true;
        }
        let (target, rect, base, mode) = (m.target, m.rect(), m.base.clone(), m.mode);
        if std::env::var_os("TIMEWARP_DEBUG_MARQUEE").is_some() {
            log::info!("marquee start={:?} now={:?} rect={:?}", m.start, m.now, rect);
            for (p, b) in self.item_bounds.borrow().iter() {
                log::info!("  item {} {:?}", p.file_name().unwrap_or_default().to_string_lossy(), b);
            }
        }
        let in_list: BTreeSet<PathBuf> = match target {
            SelectTarget::Games => self.games.iter().map(|g| g.path.clone()).collect(),
            SelectTarget::Clips => self.clips.iter().map(|c| c.path.clone()).collect(),
        };
        let hits: Vec<PathBuf> = self
            .item_bounds
            .borrow()
            .iter()
            .filter(|(p, b)| in_list.contains(*p) && b.intersects(&rect))
            .map(|(p, _)| p.clone())
            .collect();
        let sel = self.selection_mut(target);
        *sel = match mode {
            DragMode::Replace => hits.into_iter().collect(),
            DragMode::Add => base.into_iter().chain(hits).collect(),
            DragMode::Toggle => base.symmetric_difference(&hits.into_iter().collect()).cloned().collect(),
        };
        cx.notify();
    }

    pub fn marquee_up(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.marquee.take() else { return };
        if m.active {
            cx.notify();
            return;
        }
        // A plain click on empty space (not on any row/card) clears the selection.
        let on_item = self.item_bounds.borrow().values().any(|b| b.contains(&m.start));
        if !on_item && m.mode == DragMode::Replace {
            self.selection_mut(m.target).clear();
            cx.notify();
        }
    }

    /// The translucent rectangle, drawn over the whole window.
    pub fn marquee_overlay(&self) -> Option<impl IntoElement> {
        let m = self.marquee.as_ref().filter(|m| m.active)?;
        let r = m.rect();
        Some(
            div()
                .absolute()
                .left(r.origin.x)
                .top(r.origin.y)
                .w(r.size.width)
                .h(r.size.height)
                .rounded(px(2.))
                .border_1()
                .border_color(theme::victory())
                .bg(theme::victory().opacity(0.12)),
        )
    }
}
