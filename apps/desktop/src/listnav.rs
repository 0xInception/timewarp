//! Keyboard control of the Games list and the Clips grid: a focus row/card
//! you move with the arrows (⇧ extends the selection), ↵ to open, Space to
//! select a game or preview a clip. The focus is separate from the
//! multi-selection, like Gmail's cursor.

use std::path::PathBuf;

use gpui_kit::*;

use crate::TimewarpApp;

impl TimewarpApp {
    /// Child index of game `idx` in the list (day headers are children too).
    fn game_child_index(&self, idx: usize) -> Option<usize> {
        let mut child = 0;
        let mut last_day = None;
        for i in self.visible_games() {
            let day = self.games[i].meta.started_at.date_naive();
            if last_day != Some(day) {
                last_day = Some(day);
                child += 1;
            }
            if i == idx {
                return Some(child);
            }
            child += 1;
        }
        None
    }

    /// Returns true when the key was handled.
    pub fn game_list_key(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let order = self.visible_games();
        if order.is_empty() {
            return false;
        }
        let pos = self
            .list_cursor
            .as_ref()
            .and_then(|p| order.iter().position(|&i| &self.games[i].path == p));
        let move_to = |pos: Option<usize>, delta: isize| -> usize {
            match pos {
                None => 0,
                Some(p) => (p as isize + delta).clamp(0, order.len() as isize - 1) as usize,
            }
        };
        match k.key.as_str() {
            "down" | "up" if !k.modifiers.platform => {
                let next = move_to(pos, if k.key == "down" { 1 } else { -1 });
                let path = self.games[order[next]].path.clone();
                if k.modifiers.shift {
                    if let Some(p) = pos {
                        self.selected.insert(self.games[order[p]].path.clone());
                    }
                    self.selected.insert(path.clone());
                }
                self.list_cursor = Some(path);
                self.cursor_ring = true;
                self.select_anchor = Some(order[next]);
                if let Some(child) = self.game_child_index(order[next]) {
                    self.games_scroll.scroll_to_item(child);
                }
                true
            }
            "enter" => {
                let Some(p) = pos else { return false };
                self.open_review(order[p], None, window, cx);
                true
            }
            "space" => {
                let Some(p) = pos else { return false };
                let path = self.games[order[p]].path.clone();
                if !self.selected.remove(&path) {
                    self.selected.insert(path);
                }
                true
            }
            _ => false,
        }
    }

    /// Clips are a grid: ←→ move one card, ↑↓ one row.
    pub fn clip_list_key(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        #[cfg(target_os = "macos")]
        {
            let n = self.clips.len();
            if n == 0 {
                return false;
            }
            // Cards are 272px wide with an 8px gap inside 16px padding.
            let width = f32::from(self.list_area.get().size.width);
            let cols = (((width - 32.0 + 8.0) / 280.0).floor() as usize).max(1);
            let pos = self.list_cursor.as_ref().and_then(|p| self.clips.iter().position(|c| &c.path == p));
            let delta: isize = match k.key.as_str() {
                "right" => 1,
                "left" => -1,
                "down" => cols as isize,
                "up" => -(cols as isize),
                "enter" | "space" => {
                    let Some(p) = pos else { return false };
                    let path: PathBuf = self.clips[p].path.clone();
                    self.open_clip_path(&path, window, cx);
                    return true;
                }
                _ => return false,
            };
            if k.modifiers.platform {
                return false;
            }
            let next = match pos {
                None => 0,
                Some(p) => (p as isize + delta).clamp(0, n as isize - 1) as usize,
            };
            let path = self.clips[next].path.clone();
            if k.modifiers.shift {
                if let Some(p) = pos {
                    self.selected_clips.insert(self.clips[p].path.clone());
                }
                self.selected_clips.insert(path.clone());
            }
            self.list_cursor = Some(path);
            self.cursor_ring = true;
            self.clip_anchor = Some(next);
            self.clips_scroll.scroll_to_item(next);
            true
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (k, window, cx);
            false
        }
    }
}
