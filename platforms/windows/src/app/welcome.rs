//! The launcher's side of Orbit Setup (`setup.rs`): opening its window inside Orbit, and doing
//! what it decides. Setup only asks; the launcher alone changes settings and data.

use super::*;
use crate::setup::{self, Change, Mode};
use orbit_core::import::{Haul, Merged};

impl App {
    fn setup_info(&self, at: Option<(i32, i32)>) -> setup::InApp {
        setup::InApp {
            main: self.send,
            theme: self.theme,
            active_hotkey: self.active_hotkey.clone(),
            everything_on: self.settings.extensions.everything.enabled,
            files_keyword: self.settings.extensions.everything.alias.clone(),
            tools_on: [self.settings.integrations.one_password, self.settings.integrations.ente_auth],
            at,
        }
    }

    /// The Import page on its own, from Settings or the launcher.
    pub(super) fn open_import(&mut self) {
        self.hide(false);
        setup::open_in_app(Mode::Import, self.setup_info(None));
    }

    /// The welcome: on a first start, or carrying on from Setup.exe where its window was.
    pub(super) fn open_welcome(&mut self, installed: bool, at: Option<(i32, i32)>) {
        setup::open_in_app(Mode::Welcome { installed }, self.setup_info(at));
    }

    pub(super) fn on_setup(&mut self, change: Change) {
        match change {
            Change::Import { row, from, haul } => self.import_haul(row, &from, haul),
            Change::ImportOrbitFile(path) => {
                if path.file_name().is_some_and(|name| name.eq_ignore_ascii_case("settings.json")) {
                    self.import_settings(&path);
                } else {
                    self.worker.send(Req::Import { path });
                }
            }
            Change::Hotkey(chord) => {
                let mut settings = self.settings.clone();
                settings.launcher.hotkey = Some(chord);
                self.apply_settings(settings);
                self.reopen_settings();
                setup::hotkey_changed(&self.active_hotkey);
            }
            Change::Everything(on) => {
                let mut settings = self.settings.clone();
                settings.extensions.everything.enabled = on;
                self.apply_settings(settings);
                self.reopen_settings();
            }
            Change::Tool { tool, on } => {
                let mut settings = self.settings.clone();
                match tool {
                    crate::integrations::Tool::OnePassword => settings.integrations.one_password = on,
                    crate::integrations::Tool::EnteAuth => settings.integrations.ente_auth = on,
                }
                self.apply_settings(settings);
                self.reopen_settings();
            }
            Change::Finished { open } => {
                if !self.settings.onboarded {
                    self.settings.onboarded = true;
                    self.save_settings();
                }
                if open {
                    self.show(Scope::Root, "setup", qpc());
                }
            }
        }
    }

    /// Links go straight into the settings. Snippets and notes go to the data thread, which
    /// answers with `on_haul_imported`.
    fn import_haul(&mut self, row: usize, from: &str, haul: Haul) {
        let mut links = self.settings.links.clone();
        let apps = &self.catalog.apps;
        let merged = orbit_core::import::merge_links(&haul.links, &mut links, &self.settings.custom_commands, |name| crate::importer::app_named(apps, name).map(|app| app.id.clone()));
        if merged.added > 0 {
            self.settings.links = links;
            self.links_saved();
        }
        if haul.snippets.is_empty() && haul.notes.is_empty() {
            self.finish_import(row, merged, Ok(((0, 0), (0, 0))), &haul.left_out);
            return;
        }
        let json = crate::importer::bundle(&haul, from, now_unix());
        self.worker.send(Req::ImportHaul { json, row, links: merged, left_out: haul.left_out });
    }

    pub(super) fn on_haul_imported(&mut self, row: usize, links: Merged, left_out: Vec<String>, result: Result<((usize, usize), (usize, usize)), String>) {
        if result.as_ref().is_ok_and(|((snippets, _), _)| *snippets > 0) {
            // New snippets may bring keywords the running hook hasn't heard of.
            if self.triggers_needed() {
                self.worker.send(Req::SnippetTriggers);
            }
            crate::settings_win::snippets_changed();
        }
        self.finish_import(row, links, result, &left_out);
    }

    fn finish_import(&mut self, row: usize, links: Merged, result: Result<((usize, usize), (usize, usize)), String>, left_out: &[String]) {
        let (text, ok) = match result {
            Ok((snippets, notes)) => {
                let mut text = orbit_core::import::result_text(links, snippets, notes);
                if !left_out.is_empty() {
                    text.push_str(&format!(" Left behind: {}.", orbit_core::import::join_list(left_out)));
                }
                (text, true)
            }
            Err(why) => {
                let added = if links.added > 0 { format!(" The {} came over.", if links.added == 1 { "quicklink" } else { "quicklinks" }) } else { String::new() };
                (format!("Couldn't bring the snippets and notes over: {why}.{added}"), false)
            }
        };
        if setup::is_open() {
            setup::imported(row, &text, ok);
        } else {
            self.report(&text);
        }
    }
}

impl App {
    /// How reading an Orbit export or settings file went, on the Import page if it's open.
    pub(super) fn orbit_file_report(&mut self, text: &str, ok: bool) {
        if setup::is_open() {
            setup::orbit_file_imported(text, ok);
        } else {
            self.report(text);
        }
    }
}
