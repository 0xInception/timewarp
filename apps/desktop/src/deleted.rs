//! Recently deleted: everything Timewarp moves to the Trash, with Restore
//! and Delete forever, so you never need Finder to undo a mistake.
//!
//! Files really go to the system Trash (space comes back when it's emptied,
//! Finder's Put Back works, and on Windows this maps to the Recycle Bin).
//! This module only keeps a small record of what went where. Items drop off
//! the list on their own once they leave the Trash, by any route.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Local};
use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::ui::{self, icon};
use crate::{TimewarpApp, theme};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Kind {
    Game,
    Clip,
    /// A full video replaced by a new file at the same place (keep moments
    /// only, joining split recordings, repairs). Not restorable in place.
    Replaced,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    pub kind: Kind,
    pub title: String,
    /// Why it's here, for Replaced items ("Full video before keeping moments only").
    #[serde(default)]
    pub note: String,
    pub deleted_at: DateTime<Local>,
    pub bytes: u64,
    /// (original place, place in the Trash). The video comes first.
    pub files: Vec<(PathBuf, PathBuf)>,
}

impl Item {
    fn video(&self) -> Option<&(PathBuf, PathBuf)> {
        self.files.first()
    }

    /// Still in the Trash (the video file, which is what matters).
    pub fn in_trash(&self) -> bool {
        self.video().is_some_and(|(_, t)| t.exists())
    }

    pub fn restorable(&self) -> bool {
        self.kind != Kind::Replaced
    }

    /// Library key of a deleted game (its file name), for its notes.
    pub fn game_key(&self) -> Option<String> {
        (self.kind == Kind::Game)
            .then(|| self.video().and_then(|(o, _)| o.file_name()).map(|n| n.to_string_lossy().into_owned()))
            .flatten()
    }
}

static LOG: Mutex<Option<Vec<Item>>> = Mutex::new(None);

fn path() -> PathBuf {
    if let Some(dir) = std::env::var_os("TIMEWARP_DIR") {
        return PathBuf::from(dir).join("deleted.json");
    }
    timewarp_engine::Config::path().with_file_name("deleted.json")
}

fn with<R>(f: impl FnOnce(&mut Vec<Item>) -> R) -> R {
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    let items = guard.get_or_insert_with(|| {
        std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    });
    f(items)
}

fn save(items: &[Item]) {
    let p = path();
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let tmp = p.with_extension("json.tmp");
    if let Ok(bytes) = serde_json::to_vec_pretty(items) {
        if let Err(e) = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &p)) {
            log::warn!("saving deleted list: {e}");
        }
    }
}

/// "2026-11-05_16-11-15_Rammus.mp4" → "Rammus, Nov 5 at 16:11".
pub fn title_from_file(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let Some(stamp) = stem.get(..19) else { return stem };
    let Ok(t) = chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d_%H-%M-%S") else { return stem };
    let when = t.format("%b %-d at %H:%M").to_string();
    match stem.get(20..).filter(|s| !s.is_empty()) {
        Some(champ) => format!("{}, {when}", champ.replace('_', " ")),
        None => format!("Game, {when}"),
    }
}

/// Move `paths` (video first, then its companions) to the Trash as one item.
/// Returns (original, in-trash) pairs for an immediate Undo.
pub fn trash(kind: Kind, title: Option<String>, note: &str, paths: &[PathBuf]) -> Vec<(PathBuf, PathBuf)> {
    let Some(video) = paths.first() else { return vec![] };
    let bytes = paths.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    let moved = ui::trash_all(paths);
    log::info!("moved to Trash ({kind:?}): {}", video.display());
    if moved.first().is_some_and(|(o, _)| o == video) {
        let item = Item {
            id: Local::now().timestamp_micros() as u64,
            kind,
            title: title.unwrap_or_else(|| title_from_file(video)),
            note: note.to_string(),
            deleted_at: Local::now(),
            bytes,
            files: moved.clone(),
        };
        with(|items| {
            // Ids must be unique even for several items in the same microsecond.
            let mut item = item;
            while items.iter().any(|i| i.id == item.id) {
                item.id += 1;
            }
            items.push(item);
            save(items);
        });
    }
    moved
}

/// Newest first.
pub fn items() -> Vec<Item> {
    let mut v = with(|items| items.clone());
    v.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
    v
}

pub fn count() -> usize {
    with(|items| items.len())
}

fn take(id: u64) -> Option<Item> {
    with(|items| {
        let i = items.iter().position(|it| it.id == id)?;
        let item = items.remove(i);
        save(items);
        Some(item)
    })
}

/// Files were put back some other way (the toast's Undo): forget them.
pub fn forget_files(files: &[(PathBuf, PathBuf)]) {
    with(|items| {
        let before = items.len();
        items.retain(|it| !it.files.iter().any(|f| files.contains(f)));
        if items.len() != before {
            save(items);
        }
    });
}

/// Drop items that left the Trash (emptied, Put Back in Finder, deleted).
pub fn prune() -> Vec<Item> {
    with(|items| {
        let (gone, keep): (Vec<Item>, Vec<Item>) = std::mem::take(items).into_iter().partition(|it| !it.in_trash());
        *items = keep;
        if !gone.is_empty() {
            save(items);
        }
        gone
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    One(u64),
    All,
}

impl TimewarpApp {
    /// Periodic: notice items that left the Trash. A game that's gone for
    /// good takes its notes with it; one put back reappears in Games.
    pub fn prune_deleted(&mut self, cx: &mut Context<Self>) {
        let gone = prune();
        if gone.is_empty() {
            return;
        }
        for it in &gone {
            log::info!("left the Trash outside Timewarp ({:?}): {}", it.kind, it.title);
        }
        let mut reload = false;
        for it in &gone {
            let put_back = it.video().is_some_and(|(o, _)| o.exists());
            reload |= put_back;
            if !put_back {
                self.forget_notes(it, cx);
            }
        }
        if reload {
            self.reload_games();
            self.reload_clips();
        }
        cx.notify();
    }

    fn forget_notes(&mut self, it: &Item, cx: &mut Context<Self>) {
        if let Some(key) = it.game_key() {
            self.store.update(cx, |s, cx| {
                s.forget_game(&key);
                cx.notify();
            });
        }
    }

    pub fn restore_deleted(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(item) = take(id) else { return };
        if let Some((orig, _)) = item.files.iter().find(|(o, _)| o.exists()) {
            self.toast(
                IconName::X,
                format!("Couldn't restore: {} is already there.", orig.file_name().unwrap_or_default().to_string_lossy()),
                None,
                cx,
            );
            with(|items| {
                items.push(item);
                save(items);
            });
            return;
        }
        let ok = item.files.iter().all(|(orig, in_trash)| {
            !in_trash.exists() || {
                let _ = std::fs::create_dir_all(orig.parent().unwrap_or(Path::new("/")));
                std::fs::rename(in_trash, orig).is_ok()
            }
        });
        self.reload_games();
        self.reload_clips();
        if ok {
            let what = if item.kind == Kind::Clip { "Clips" } else { "Games" };
            self.toast(IconName::Undo2, format!("{} restored to {what}.", item.title), None, cx);
        } else {
            self.toast(IconName::X, "Couldn't restore everything. Check the Trash in Finder.", None, cx);
        }
        cx.notify();
    }

    fn delete_forever(&mut self, ids: &[u64], cx: &mut Context<Self>) {
        let mut freed = 0;
        let mut failed = 0;
        for &id in ids {
            let Some(item) = take(id) else { continue };
            log::info!("deleted forever ({:?}): {}", item.kind, item.title);
            for (_, in_trash) in &item.files {
                if in_trash.exists() && std::fs::remove_file(in_trash).is_err() {
                    failed += 1;
                }
            }
            freed += item.bytes;
            self.forget_notes(&item, cx);
        }
        self.deleted_confirm = None;
        if failed > 0 {
            self.toast(IconName::X, "Some files couldn't be deleted. Empty the Trash in Finder instead.", None, cx);
        } else {
            self.toast(IconName::Trash, format!("Deleted for good. {} freed.", ui::human_bytes(freed)), None, cx);
        }
        cx.notify();
    }

    fn deleted_row(&self, it: &Item, cx: &mut Context<Self>) -> AnyElement {
        let id = it.id;
        let confirming = self.deleted_confirm == Some(Confirm::One(id));
        let glyph = match it.kind {
            Kind::Game => IconName::Gamepad2,
            Kind::Clip => IconName::Scissors,
            Kind::Replaced => IconName::Film,
        };
        let kind = match it.kind {
            Kind::Game => "Game".to_string(),
            Kind::Clip => "Clip".to_string(),
            Kind::Replaced => it.note.clone(),
        };
        let when = {
            let d = it.deleted_at;
            let today = Local::now().date_naive();
            match (today - d.date_naive()).num_days() {
                0 => format!("today at {}", d.format("%H:%M")),
                1 => format!("yesterday at {}", d.format("%H:%M")),
                _ => d.format("%b %-d").to_string(),
            }
        };
        let in_trash = it.video().map(|(_, t)| t.clone());
        let actions = if confirming {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().text_sm().text_color(theme::muted()).child("Delete for good? This can't be undone."))
                .child(ui::ghost_button(("del-cancel", id as usize)).child("Cancel").on_click(cx.listener(|this, _, _, cx| {
                    this.deleted_confirm = None;
                    cx.notify();
                })))
                .child(
                    ui::solid_button(("del-forever", id as usize), theme::death())
                        .text_color(theme::text())
                        .child("Delete forever")
                        .on_click(cx.listener(move |this, _, _, cx| this.delete_forever(&[id], cx))),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(if it.restorable() {
                    ui::solid_button(("restore", id as usize), theme::raised())
                        .child(icon(IconName::Undo2, 13., theme::text()))
                        .child("Restore")
                        .on_click(cx.listener(move |this, _, _, cx| this.restore_deleted(id, cx)))
                        .into_any_element()
                } else {
                    ui::ghost_button(("reveal", id as usize))
                        .child(icon(IconName::FolderOpen, 13., theme::muted()))
                        .child("Show in Finder")
                        .on_click(move |_, _, _| {
                            if let Some(p) = &in_trash {
                                let _ = std::process::Command::new("open").arg("-R").arg(p).spawn();
                            }
                        })
                        .into_any_element()
                })
                .child(
                    ui::icon_button(("forever", id as usize), IconName::Trash, 13.)
                        .tooltip(ui::tip("Delete forever"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.deleted_confirm = Some(Confirm::One(id));
                            cx.notify();
                        })),
                )
                .into_any_element()
        };
        div()
            .flex()
            .items_center()
            .gap_3()
            .px_3()
            .py_2p5()
            .rounded_lg()
            .when(confirming, |d| d.bg(theme::death().opacity(0.08)))
            .hover(|s| s.bg(theme::panel()))
            .child(
                div()
                    .size(px(32.))
                    .flex_none()
                    .rounded_md()
                    .bg(theme::raised())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(glyph, 15., theme::muted())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(it.title.clone()))
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(theme::muted())
                            .child(format!("{kind} · deleted {when} · {}", ui::human_bytes(it.bytes))),
                    ),
            )
            .child(actions)
            .into_any_element()
    }

    pub fn deleted_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items = items();
        let total: u64 = items.iter().map(|i| i.bytes).sum();
        let ids: Vec<u64> = items.iter().map(|i| i.id).collect();
        let confirm_all = self.deleted_confirm == Some(Confirm::All);
        let header = ui::page_header()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Recently deleted"))
            .child(ui::drag_area())
            .when(!items.is_empty(), |d| {
                d.child(div().text_sm().text_color(theme::muted()).child(format!(
                    "{} item{}, {}",
                    items.len(),
                    if items.len() == 1 { "" } else { "s" },
                    ui::human_bytes(total)
                )))
                .child(if confirm_all {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(ui::ghost_button("empty-cancel").child("Cancel").on_click(cx.listener(|this, _, _, cx| {
                            this.deleted_confirm = None;
                            cx.notify();
                        })))
                        .child(
                            ui::solid_button("empty-confirm", theme::death())
                                .text_color(theme::text())
                                .child(format!("Delete all forever, free {}", ui::human_bytes(total)))
                                .on_click(cx.listener(move |this, _, _, cx| this.delete_forever(&ids, cx))),
                        )
                        .into_any_element()
                } else {
                    ui::ghost_button("empty-deleted")
                        .child(icon(IconName::Trash, 14., theme::muted()))
                        .child("Empty…")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.deleted_confirm = Some(Confirm::All);
                            cx.notify();
                        }))
                        .into_any_element()
                })
            });

        let body = if items.is_empty() {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(icon(IconName::Trash, 28., theme::muted()))
                .child(div().text_lg().font_weight(FontWeight::MEDIUM).child("Nothing deleted"))
                .child(div().max_w(px(440.)).text_center().text_color(theme::muted()).child(
                    "Games and clips you delete go to the Trash and show up here, so you can restore them or delete them for good.",
                ))
                .into_any_element()
        } else {
            div()
                .id("deleted-list")
                .flex_1()
                .overflow_y_scroll()
                .child(
                    div()
                        .max_w(px(860.))
                        .px_4()
                        .py_3()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().px_3().pb_2().text_sm().text_color(theme::muted()).child(
                            "These are in your Mac's Trash. Restore puts them back with their notes. Delete forever, or emptying the Trash in Finder, frees the space.",
                        ))
                        .children(items.iter().map(|it| self.deleted_row(it, cx)).collect::<Vec<_>>())
                        .child(div().px_3().pt_4().text_xs().text_color(theme::muted()).child(
                            "Tip: macOS can empty the Trash for you after 30 days. Finder → Settings → Advanced → Remove items from the Trash after 30 days.",
                        )),
                )
                .into_any_element()
        };
        div().size_full().flex().flex_col().child(header).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn titles_games_from_their_file_names() {
        assert_eq!(title_from_file(Path::new("/x/2026-11-05_16-11-15_Rammus.mp4")), "Rammus, Nov 5 at 16:11");
        assert_eq!(title_from_file(Path::new("/x/2026-11-05_16-11-15.mp4")), "Game, Nov 5 at 16:11");
        assert_eq!(title_from_file(Path::new("/x/Full game.mp4")), "Full game");
    }
}
