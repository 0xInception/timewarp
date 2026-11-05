//! Settings page. Changes save immediately and apply from the next recording.

use gpui_kit::component::switch::Switch;
use gpui_kit::*;
use gpui_kit::assets::IconName;
use gpui_kit::prelude::FluentBuilder;
use timewarp_engine::Codec;

use crate::{TimewarpApp, theme, ui};

impl TimewarpApp {
    fn setting_row(&self, title: &str, help: &str, control: impl IntoElement) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_6()
            .py_4()
            .border_b_1()
            .border_color(theme::line())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(div().child(title.to_string()))
                    .child(div().text_sm().text_color(theme::muted()).child(help.to_string())),
            )
            .child(div().flex_none().child(control))
    }

    fn section(title: &str) -> impl IntoElement {
        div().pt_8().pb_1().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(title.to_string())
    }

    fn toggle(&self, id: &'static str, on: bool, set: fn(&mut timewarp_engine::Config, bool), cx: &mut Context<Self>) -> impl IntoElement {
        Switch::new(id).checked(on).on_click(cx.listener(move |this, v: &bool, _, cx| {
            let v = *v;
            this.update_config(|c| set(c, v), cx)
        }))
    }

    pub fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Save recordings here".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(dir) = paths.into_iter().next() {
                    let _ = this.update(cx, |app, cx| app.update_config(|c| c.output_dir = dir, cx));
                }
            }
        })
        .detach();
    }

    // -- backup and transfer ---------------------------------------------------

    /// Recordings an export would include (all, or what the Games filter shows).
    fn export_recordings(&self) -> Vec<std::path::PathBuf> {
        if self.export_filtered_only {
            self.visible_games().into_iter().map(|i| self.games[i].path.clone()).collect()
        } else {
            self.games.iter().map(|g| g.path.clone()).collect()
        }
    }

    pub fn start_export(&mut self, cx: &mut Context<Self>) {
        let parts = self.transfer_parts;
        let recordings = if parts.recordings { self.export_recordings() } else { vec![] };
        let clips: Vec<std::path::PathBuf> =
            if parts.clips { self.clips.iter().map(|c| c.path.clone()).collect() } else { vec![] };
        self.export_with(parts, recordings, clips, cx);
    }

    /// Ask for a folder and export `recordings` / `clips` plus the chosen parts.
    pub fn export_with(
        &mut self,
        parts: crate::transfer::Parts,
        recordings: Vec<std::path::PathBuf>,
        clips: Vec<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Export here".into()),
        });
        let config = self.config.clone();
        let store_json = {
            self.store.update(cx, |s, _| s.flush());
            serde_json::to_vec(&*self.store.read(cx)).unwrap_or_default()
        };
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(parent) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |app, cx| {
                app.transfer_busy = true;
                app.toast(IconName::LoaderCircle, "Exporting…", None, cx);
                if let Some(t) = app.toasts.last_mut() {
                    t.until = std::time::Instant::now() + std::time::Duration::from_secs(3600);
                }
            });
            let task = cx
                .background_executor()
                .spawn(async move { crate::transfer::export(&parent, parts, &config, &store_json, &recordings, &clips) });
            let result = task.await;
            let _ = this.update(cx, |app, cx| {
                app.transfer_busy = false;
                app.toasts.retain(|t| t.text != "Exporting…");
                match result {
                    Ok((dir, m)) => app.toast(
                        IconName::FolderOpen,
                        format!(
                            "Exported {} recordings, {} clips, notes for {} games{}.",
                            m.recordings,
                            m.clips,
                            m.notes,
                            if m.settings { " and your settings" } else { "" }
                        ),
                        Some(ui::ToastAction::Reveal(dir)),
                        cx,
                    ),
                    Err(e) => app.toast(IconName::X, format!("Export failed: {e}"), None, cx),
                }
            });
        })
        .detach();
    }

    pub fn choose_import(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Import this export".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(dir) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |app, cx| match crate::transfer::read_manifest(&dir) {
                Ok(m) => {
                    // Pre-tick whatever the export contains.
                    let parts = crate::transfer::Parts {
                        notes: m.notes > 0,
                        matchups: m.matchups > 0,
                        clips: m.clips > 0,
                        recordings: m.recordings > 0,
                        settings: m.settings,
                    };
                    app.pending_import = Some((dir, m, parts));
                    cx.notify();
                }
                Err(e) => app.toast(IconName::X, e, None, cx),
            });
        })
        .detach();
    }

    fn run_import(&mut self, cx: &mut Context<Self>) {
        let Some((dir, _, parts)) = self.pending_import.take() else { return };
        self.transfer_busy = true;
        self.toast(IconName::LoaderCircle, "Importing…", None, cx);
        if let Some(t) = self.toasts.last_mut() {
            t.until = std::time::Instant::now() + std::time::Duration::from_secs(3600);
        }
        let output = self.config.output_dir.clone();
        let task = cx.background_executor().spawn({
            let dir = dir.clone();
            async move { crate::transfer::import_files(&dir, parts, &output) }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |app, cx| {
                app.transfer_busy = false;
                app.toasts.retain(|t| t.text != "Importing…");
                let (games, matchups) = app.store.update(cx, |s, cx| {
                    let r = s.merge_import(&dir, parts);
                    cx.notify();
                    r
                });
                let mut settings_note = "";
                if parts.settings {
                    match crate::transfer::read_settings(&dir, &app.config) {
                        Ok(cfg) => {
                            app.update_config(|c| *c = cfg, cx);
                            settings_note = " Settings applied (your save location was kept).";
                        }
                        Err(e) => app.toast(IconName::X, e, None, cx),
                    }
                }
                app.reload_games();
                app.reload_clips();
                match result {
                    Ok(n) => {
                        let mut msg = format!(
                            "Imported {} recordings, {} clips, notes for {} games and {} matchups.",
                            n.recordings, n.clips, games, matchups
                        );
                        if n.skipped > 0 {
                            msg.push_str(&format!(" {} already here were skipped.", n.skipped));
                        }
                        msg.push_str(settings_note);
                        app.toast(IconName::Check, msg, None, cx);
                    }
                    Err(e) => app.toast(IconName::X, format!("Import stopped: {e}"), None, cx),
                }
            });
        })
        .detach();
    }

    fn check_row(
        &self,
        id: &'static str,
        label: String,
        on: bool,
        indent: bool,
        set: fn(&mut TimewarpApp, bool),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div().when(indent, |d| d.pl_7()).py_1().child(
            gpui_kit::component::checkbox::Checkbox::new(id)
                .label(label)
                .checked(on)
                .on_click(cx.listener(move |this, v: &bool, _, cx| {
                    set(this, *v);
                    cx.notify();
                })),
        )
    }

    fn transfer_section(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.transfer_parts;
        let noted = self.store.read(cx).games.values().filter(|g| g.is_protected()).count();
        let matchup_notes = self.store.read(cx).matchups.values().filter(|m| !m.notes.trim().is_empty()).count();
        let clip_bytes: u64 = self.clips.iter().map(|c| c.bytes).sum();
        let recs = self.export_recordings();
        let rec_bytes: u64 = recs.iter().filter_map(|r| self.games.iter().find(|g| &g.path == r)).map(|g| g.bytes).sum();
        let nothing = !(p.notes || p.matchups || p.clips || p.recordings || p.settings);
        let narrowed = self.filter.is_narrowed();

        let export = div()
            .flex()
            .flex_col()
            .gap_1()
            .py_4()
            .border_b_1()
            .border_color(theme::line())
            .child(div().child("Export"))
            .child(div().text_sm().text_color(theme::muted()).pb_2().child(
                "Copies what you pick into a new folder you can back up or move to another Mac. Nothing is removed from Timewarp.",
            ))
            .child(self.check_row(
                "ex-settings",
                "Settings (everything except the save location)".into(),
                p.settings,
                false,
                |a, v| a.transfer_parts.settings = v,
                cx,
            ))
            .child(self.check_row("ex-notes", format!("Notes and saved moments ({noted} games)"), p.notes, false, |a, v| a.transfer_parts.notes = v, cx))
            .child(self.check_row("ex-matchups", format!("Matchup notes ({matchup_notes})"), p.matchups, false, |a, v| a.transfer_parts.matchups = v, cx))
            .child(self.check_row(
                "ex-clips",
                format!("Clips ({}, {})", self.clips.len(), ui::human_bytes(clip_bytes)),
                p.clips,
                false,
                |a, v| a.transfer_parts.clips = v,
                cx,
            ))
            .child(self.check_row(
                "ex-recordings",
                format!("Recordings ({}, {})", recs.len(), ui::human_bytes(rec_bytes)),
                p.recordings,
                false,
                |a, v| a.transfer_parts.recordings = v,
                cx,
            ))
            .when(p.recordings && (narrowed || self.export_filtered_only), |d| {
                d.child(self.check_row(
                    "ex-filtered",
                    format!("Only games matching the filters on the Games page ({})", self.visible_games().len()),
                    self.export_filtered_only,
                    true,
                    |a, v| a.export_filtered_only = v,
                    cx,
                ))
            })
            .child(
                div().pt_3().child(
                    ui::solid_button("export", if nothing || self.transfer_busy { theme::raised() } else { theme::victory() })
                        .w(px(160.))
                        .text_color(if nothing || self.transfer_busy { theme::muted() } else { theme::bg() })
                        .child(ui::icon(IconName::FolderOutput, 14., if nothing { theme::muted() } else { theme::bg() }))
                        .child("Export…")
                        .when(!nothing && !self.transfer_busy, |b| b.on_click(cx.listener(|this, _, _, cx| this.start_export(cx)))),
                ),
            );

        let import = div()
            .flex()
            .flex_col()
            .gap_1()
            .py_4()
            .border_b_1()
            .border_color(theme::line())
            .child(div().child("Import"))
            .child(div().text_sm().text_color(theme::muted()).pb_2().child(
                "Bring in a Timewarp export. Anything you already have is kept: notes are combined, and recordings or clips that are already here are skipped.",
            ))
            .map(|d| match &self.pending_import {
                None => d.child(
                    div().pt_1().child(
                        ui::solid_button("import", theme::raised())
                            .w(px(160.))
                            .text_color(theme::text())
                            .child(ui::icon(IconName::FolderInput, 14., theme::text()))
                            .child("Import…")
                            .when(!self.transfer_busy, |b| b.on_click(cx.listener(|this, _, _, cx| this.choose_import(cx)))),
                    ),
                ),
                Some((dir, m, parts)) => {
                    let parts = *parts;
                    let when = m.exported_at.map(|t| t.format("%b %-d, %H:%M").to_string()).unwrap_or_default();
                    let name = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
                    d.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_3()
                            .rounded_lg()
                            .bg(theme::panel())
                            .border_1()
                            .border_color(theme::line())
                            .child(div().font_weight(FontWeight::MEDIUM).child(name))
                            .child(div().text_sm().text_color(theme::muted()).pb_1().child(format!(
                                "Exported {when}: {} recordings, {} clips, notes for {} games, {} matchup notes{}.",
                                m.recordings,
                                m.clips,
                                m.notes,
                                m.matchups,
                                if m.settings { ", settings" } else { "" }
                            )))
                            .when(m.settings, |d| {
                                d.child(self.check_row("im-settings", "Settings (replaces yours, keeps your save location)".into(), parts.settings, false, |a, v| {
                                    if let Some((_, _, p)) = &mut a.pending_import {
                                        p.settings = v
                                    }
                                }, cx))
                            })
                            .when(m.notes > 0, |d| {
                                d.child(self.check_row("im-notes", "Notes and saved moments".into(), parts.notes, false, |a, v| {
                                    if let Some((_, _, p)) = &mut a.pending_import {
                                        p.notes = v
                                    }
                                }, cx))
                            })
                            .when(m.matchups > 0, |d| {
                                d.child(self.check_row("im-matchups", "Matchup notes".into(), parts.matchups, false, |a, v| {
                                    if let Some((_, _, p)) = &mut a.pending_import {
                                        p.matchups = v
                                    }
                                }, cx))
                            })
                            .when(m.clips > 0, |d| {
                                d.child(self.check_row("im-clips", format!("Clips ({})", m.clips), parts.clips, false, |a, v| {
                                    if let Some((_, _, p)) = &mut a.pending_import {
                                        p.clips = v
                                    }
                                }, cx))
                            })
                            .when(m.recordings > 0, |d| {
                                d.child(self.check_row("im-recordings", format!("Recordings ({})", m.recordings), parts.recordings, false, |a, v| {
                                    if let Some((_, _, p)) = &mut a.pending_import {
                                        p.recordings = v
                                    }
                                }, cx))
                            })
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .pt_2()
                                    .child(
                                        ui::solid_button("import-go", theme::victory())
                                            .text_color(theme::bg())
                                            .child("Import")
                                            .on_click(cx.listener(|this, _, _, cx| this.run_import(cx))),
                                    )
                                    .child(ui::ghost_button("import-cancel").child("Cancel").on_click(cx.listener(|this, _, _, cx| {
                                        this.pending_import = None;
                                        cx.notify();
                                    }))),
                            ),
                    )
                }
            });

        div().child(export).child(import)
    }

    fn keep_chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let keep = self.config.storage.keep;
        let chip = |id: &'static str, label: &'static str, on: bool, color: Hsla, f: fn(&mut timewarp_engine::KeepKinds)| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1p5()
                .h(px(26.))
                .px_2p5()
                .rounded_full()
                .border_1()
                .text_sm()
                .cursor_pointer()
                .border_color(if on { color } else { theme::line() })
                .text_color(if on { theme::text() } else { theme::muted() })
                .bg(if on { theme::raised() } else { theme::bg() })
                .child(div().size(px(7.)).rounded_full().bg(if on { color } else { theme::line() }))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| this.update_config(|c| f(&mut c.storage.keep), cx)))
        };
        div()
            .flex()
            .gap_1p5()
            .child(chip("keep-kills", "Kills", keep.kills, theme::kill(), |k| k.kills = !k.kills))
            .child(chip("keep-deaths", "Deaths", keep.deaths, theme::death(), |k| k.deaths = !k.deaths))
            .child(chip("keep-assists", "Assists", keep.assists, theme::assist(), |k| k.assists = !k.assists))
            .child(chip("keep-objectives", "Objectives", keep.objectives, theme::objective(), |k| k.objectives = !k.objectives))
    }

    pub fn settings_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.config.capture;
        let entity = cx.entity().downgrade();
        let set = move |f: Box<dyn FnOnce(&mut timewarp_engine::Config)>, cx: &mut App| {
            if let Some(app) = entity.upgrade() {
                app.update(cx, |app, cx| app.update_config(f, cx));
            }
        };
        let (set_res, set_fps, set_codec, set_key, set_gb, set_days) =
            (set.clone(), set.clone(), set.clone(), set.clone(), set.clone(), set.clone());
        let (set_record, set_condense, set_clip) = (set.clone(), set.clone(), set);
        let condense_due = self.condense_candidates(cx);
        let condense_saves = self.condense_savings(&condense_due, cx);

        // What the storage policy would remove right now.
        let plan = self.cleanup_plan(cx);
        let plan_bytes: u64 =
            plan.iter().filter_map(|p| self.games.iter().find(|g| &g.path == p)).map(|g| g.bytes).sum();
        let total_bytes: u64 = self.games.iter().map(|g| g.bytes).sum();
        let key_options: Vec<(Option<&'static str>, &'static str)> =
            std::iter::once((None, "Off")).chain(crate::hotkey::KEYS.iter().map(|k| (Some(*k), *k))).collect();
        let current_key = self.config.bookmark_key.as_deref().and_then(|k| crate::hotkey::KEYS.iter().copied().find(|x| *x == k));

        let header = ui::page_header()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Settings"))
            .child(ui::drag_area());

        let body = div()
            .id("settings")
            .flex_1()
            .overflow_y_scroll()
            .child(
                div()
                    .max_w(px(720.))
                    .px_6()
                    .pb_10()
                    .child(Self::section("Recording"))
                    .child(self.setting_row(
                        "Record matches automatically",
                        "Starts when the game window opens and stops when the match ends.",
                        self.toggle("auto", self.config.auto_record, |c, v| c.auto_record = v, cx),
                    ))
                    .child(self.setting_row(
                        "Which games",
                        "Skipping games you'll never review saves the most space.",
                        ui::segmented(
                            "record-filter",
                            &[
                                (timewarp_engine::RecordFilter::All, "All"),
                                (timewarp_engine::RecordFilter::SkipPractice, "No practice or customs"),
                                (timewarp_engine::RecordFilter::RankedOnly, "Ranked only"),
                            ],
                            self.config.record,
                            move |v, _, cx| set_record(Box::new(move |c| c.record = v), cx),
                        ),
                    ))
                    .child(self.setting_row(
                        "Game audio",
                        "Only League's sound is recorded, never Discord or music.",
                        self.toggle("audio", c.game_audio, |c, v| c.capture.game_audio = v, cx),
                    ))
                    .child(self.setting_row(
                        "Microphone",
                        "Adds your voice to the recording. macOS asks for permission the first time.",
                        self.toggle("mic", c.microphone, |c, v| c.capture.microphone = v, cx),
                    ))
                    .child(self.setting_row(
                        "Show cursor",
                        "Include the mouse cursor in the video.",
                        self.toggle("cursor", c.show_cursor, |c, v| c.capture.show_cursor = v, cx),
                    ))
                    .child(self.setting_row(
                        "Bookmark key",
                        "Press it in game to flag a moment you want to look at later. You'll hear a soft pop; it isn't recorded. On a MacBook keyboard you may need to hold fn.",
                        ui::segmented("bookmark-key", &key_options, current_key, move |v, _, cx| {
                            set_key(Box::new(move |c| c.bookmark_key = v.map(str::to_owned)), cx)
                        }),
                    ))
                    .child(Self::section("Video"))
                    .child(self.setting_row(
                        "Resolution",
                        "Lower resolutions make smaller files. Encoding is done in hardware either way.",
                        ui::segmented(
                            "res",
                            &[(Some(720), "720p"), (Some(1080), "1080p"), (Some(1440), "1440p"), (None, "Native")],
                            c.max_height,
                            move |v, _, cx| set_res(Box::new(move |c| c.capture.max_height = v), cx),
                        ),
                    ))
                    .child(self.setting_row(
                        "Frame rate",
                        "60 fps matches League's default frame cap.",
                        ui::segmented("fps", &[(30u32, "30"), (60, "60")], c.fps, move |v, _, cx| {
                            set_fps(Box::new(move |c| c.capture.fps = v), cx)
                        }),
                    ))
                    .child(self.setting_row(
                        "Format",
                        "HEVC files are about half the size. H.264 plays everywhere, including Discord previews.",
                        ui::segmented("codec", &[(Codec::Hevc, "HEVC"), (Codec::H264, "H.264")], c.codec, move |v, _, cx| {
                            set_codec(Box::new(move |c| c.capture.codec = v), cx)
                        }),
                    ))
                    .child(Self::section("Clips"))
                    .child(self.setting_row(
                        "Clip format",
                        "Shareable clips play right inside Discord and on any device. Original is instant and full quality, but uses the recording's format (HEVC), which Discord can't preview.",
                        ui::segmented(
                            "clip-format",
                            &[
                                (timewarp_engine::ClipFormat::Shareable, "Shareable (H.264)"),
                                (timewarp_engine::ClipFormat::Original, "Original"),
                            ],
                            self.config.clips.format,
                            {
                                let set = set_clip.clone();
                                move |v, _, cx| set(Box::new(move |c| c.clips.format = v), cx)
                            },
                        ),
                    ))
                    .child(self.setting_row(
                        "Clip size limit",
                        "Clips over the limit are saved again at a lower resolution until they fit. Discord allows 10 MB, 50 MB with Nitro Basic and 500 MB with Nitro.",
                        ui::segmented(
                            "clip-size",
                            &[(None, "No limit"), (Some(10), "10 MB"), (Some(50), "50 MB"), (Some(500), "500 MB")],
                            self.config.clips.max_mb,
                            move |v, _, cx| set_clip(Box::new(move |c| c.clips.max_mb = v), cx),
                        ),
                    ))
                    .child(Self::section("Updates"))
                    .child(self.setting_row(
                        "Check for updates automatically",
                        "Once a day, Timewarp looks for a new version and lets you know. Nothing downloads or installs until you click Update in About.",
                        self.toggle("check-updates", self.config.check_updates, |c, v| c.check_updates = v, cx),
                    ))
                    .child(Self::section("Permissions"))
                    .child(self.setting_row(
                        "Permissions and setup",
                        "Screen Recording, microphone, save folder and disk space, checked continuously.",
                        ui::ghost_button("open-setup")
                            .text_color(theme::text())
                            .child(ui::icon(IconName::ShieldCheck, 14., theme::text()))
                            .child("Open")
                            .on_click(cx.listener(|this, _, _, cx| this.go(crate::Page::Setup, cx))),
                    ))
                    .child(Self::section("Backup and transfer"))
                    .child(self.transfer_section(cx))
                    .child(Self::section("Storage"))
                    .child(self.setting_row(
                        "Save location",
                        &self.config.output_dir.display().to_string(),
                        div()
                            .flex()
                            .gap_1()
                            .child(ui::ghost_button("open-dir").child("Show in Finder").on_click({
                                let dir = self.config.output_dir.clone();
                                move |_, _, _| {
                                    let _ = std::fs::create_dir_all(&dir);
                                    let _ = std::process::Command::new("open").arg(&dir).spawn();
                                }
                            }))
                            .child(
                                ui::ghost_button("change-dir")
                                    .child("Change…")
                                    .on_click(cx.listener(|this, _, _, cx| this.choose_folder(cx))),
                            ),
                    ))
                    .child(self.setting_row(
                        "Keep only the moments",
                        "Cuts old games down to their moments and deletes the rest of the video. No quality is lost, and a 30-minute game usually shrinks by 90% or more.",
                        ui::segmented(
                            "condense-after",
                            &[
                                (None, "Off"),
                                (Some(0), "After the game"),
                                (Some(1), "1 day"),
                                (Some(3), "3 days"),
                                (Some(7), "1 week"),
                                (Some(30), "1 month"),
                            ],
                            self.config.storage.condense_after_days,
                            move |v, _, cx| set_condense(Box::new(move |c| c.storage.condense_after_days = v), cx),
                        ),
                    ))
                    .child(self.setting_row(
                        "Moments to keep",
                        "Your notes, in-game bookmarks and saved moments are always kept.",
                        self.keep_chips(cx),
                    ))
                    .child(self.setting_row(
                        "Delete after",
                        "Removes whole recordings, including their kept moments, once they're this old.",
                        ui::segmented(
                            "max-days",
                            &[
                                (None, "Never"),
                                (Some(7), "1 week"),
                                (Some(14), "2 weeks"),
                                (Some(30), "1 month"),
                                (Some(90), "3 months"),
                                (Some(180), "6 months"),
                            ],
                            self.config.storage.max_days,
                            move |v, _, cx| set_days(Box::new(move |c| c.storage.max_days = v), cx),
                        ),
                    ))
                    .child(self.setting_row(
                        "Storage limit",
                        &format!("Recordings use {} now. Over the limit, the oldest games are deleted first.", ui::human_bytes(total_bytes)),
                        ui::segmented(
                            "max-gb",
                            &[(None, "Off"), (Some(50), "50 GB"), (Some(100), "100 GB"), (Some(250), "250 GB"), (Some(500), "500 GB")],
                            self.config.storage.max_gb,
                            move |v, _, cx| set_gb(Box::new(move |c| c.storage.max_gb = v), cx),
                        ),
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .py_4()
                            .border_b_1()
                            .border_color(theme::line())
                            .child(ui::icon(gpui_kit::assets::IconName::Pin, 14., theme::muted()))
                            .child(div().flex_1().text_sm().text_color(theme::muted()).child({
                                let mut parts = vec![];
                                if !condense_due.is_empty() {
                                    parts.push(format!(
                                        "cut {} game{} to moments (saves about {})",
                                        condense_due.len(),
                                        if condense_due.len() == 1 { "" } else { "s" },
                                        ui::human_bytes(condense_saves)
                                    ));
                                }
                                if !plan.is_empty() {
                                    parts.push(format!(
                                        "delete {} recording{} ({})",
                                        plan.len(),
                                        if plan.len() == 1 { "" } else { "s" },
                                        ui::human_bytes(plan_bytes)
                                    ));
                                }
                                let base = "Games you mark Keep are never touched. Games with notes or saved moments are never deleted.";
                                if parts.is_empty() {
                                    format!("{base} Nothing is due right now.")
                                } else {
                                    format!("{base} Runs after each game; right now it would {}.", parts.join(" and "))
                                }
                            }))
                            .when(!plan.is_empty() || !condense_due.is_empty(), |d| {
                                d.child(
                                    ui::solid_button("cleanup-now", theme::raised())
                                        .flex_none()
                                        .text_color(theme::text())
                                        .child("Apply now")
                                        .on_click(cx.listener(|this, _, _, cx| this.run_maintenance(cx))),
                                )
                            }),
                    )
                    .child(div().pt_6().text_sm().text_color(theme::muted()).child(
                        "Changes apply from the next recording. Timewarp never draws over the game or reads its memory; it uses macOS screen capture and Riot's official local game data.",
                    )),
            );

        div().size_full().flex().flex_col().child(header).child(body)
    }
}
