//! The extensions that come with Orbit. Each can be turned off, and then it's gone from the
//! launcher: its commands aren't listed or searched, and nothing of it runs.
//!
//! Some are features with views of their own (Files, Weather, Speed Test), whose commands are
//! [`Builtin`]s. Others answer the way an extension from a folder does, with a list of items
//! (GitHub, AI Usage), but from Orbit's own code instead of a program. Folder extensions can't use
//! these ids.

use crate::action::Builtin;
use crate::extension::{CommandSpec, Mode};
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltinExt {
    Files,
    OnePassword,
    EnteAuth,
    Weather,
    SpeedTest,
    GitHub,
    AiUsage,
}

/// A command of an extension that answers with a list, as its manifest would say it.
pub struct ListCommand {
    pub id: &'static str,
    pub title: &'static str,
    pub subtitle: &'static str,
    /// Typing it and a space opens the command. It can be changed in Settings.
    pub keyword: &'static str,
    pub keywords: &'static str,
    pub placeholder: &'static str,
}

impl ListCommand {
    pub fn spec(&self) -> CommandSpec {
        CommandSpec {
            id: self.id.into(),
            title: self.title.into(),
            subtitle: self.subtitle.into(),
            keyword: self.keyword.into(),
            keywords: self.keywords.into(),
            mode: Mode::List,
            placeholder: self.placeholder.into(),
        }
    }
}

const GITHUB: &[ListCommand] = &[
    ListCommand {
        id: "issues",
        title: "My Issues",
        subtitle: "Open issues you made, were given, or are mentioned in",
        keyword: "ghi",
        keywords: "github issue bug tracker",
        placeholder: "Search your issues\u{2026}",
    },
    ListCommand {
        id: "pulls",
        title: "My Pull Requests",
        subtitle: "Open pull requests you made, review or are mentioned in",
        keyword: "ghp",
        keywords: "github pull request pr review merge",
        placeholder: "Search your pull requests\u{2026}",
    },
    ListCommand {
        id: "notifications",
        title: "GitHub Notifications",
        subtitle: "What's new for you on GitHub",
        keyword: "ghn",
        keywords: "github inbox unread mentions",
        placeholder: "Search notifications\u{2026}",
    },
    ListCommand {
        id: "starred",
        title: "Starred Repositories",
        subtitle: "The repositories you starred on GitHub",
        keyword: "ghs",
        keywords: "github stars repo repository favourite",
        placeholder: "Search your stars\u{2026}",
    },
];

const AI_USAGE: &[ListCommand] = &[ListCommand {
    id: "usage",
    title: "AI Usage",
    subtitle: "How much of Claude's and Codex's limits you've used",
    keyword: "quota",
    keywords: "ai limits quota rate claude code codex chatgpt anthropic openai tokens",
    placeholder: "AI usage",
}];

impl BuiltinExt {
    /// In the order Settings lists them.
    pub const ALL: [BuiltinExt; 7] = [
        BuiltinExt::Files,
        BuiltinExt::OnePassword,
        BuiltinExt::EnteAuth,
        BuiltinExt::Weather,
        BuiltinExt::SpeedTest,
        BuiltinExt::GitHub,
        BuiltinExt::AiUsage,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            BuiltinExt::Files => "files",
            BuiltinExt::OnePassword => "1password",
            BuiltinExt::EnteAuth => "ente-auth",
            BuiltinExt::Weather => "weather",
            BuiltinExt::SpeedTest => "speed-test",
            BuiltinExt::GitHub => "github",
            BuiltinExt::AiUsage => "ai-usage",
        }
    }

    pub fn from_id(id: &str) -> Option<BuiltinExt> {
        BuiltinExt::ALL.into_iter().find(|ext| ext.id() == id)
    }

    pub const fn name(self) -> &'static str {
        match self {
            BuiltinExt::Files => "Everything",
            BuiltinExt::OnePassword => "1Password",
            BuiltinExt::EnteAuth => "Ente Auth",
            BuiltinExt::Weather => "Weather",
            BuiltinExt::SpeedTest => "Speed Test",
            BuiltinExt::GitHub => "GitHub",
            BuiltinExt::AiUsage => "AI Usage",
        }
    }

    /// One short line for Settings' table of extensions.
    pub const fn about(self) -> &'static str {
        match self {
            BuiltinExt::Files => "Find any file or folder on this PC",
            BuiltinExt::OnePassword => "Copy passwords and one-time codes",
            BuiltinExt::EnteAuth => "Copy two-factor codes",
            BuiltinExt::Weather => "The forecast for any place",
            BuiltinExt::SpeedTest => "Your internet speed, by Ookla",
            BuiltinExt::GitHub => "Your issues, pull requests and stars",
            BuiltinExt::AiUsage => "Your Claude and Codex limits",
        }
    }

    pub fn is_on(self, settings: &Settings) -> bool {
        match self {
            BuiltinExt::Files => settings.extensions.everything.enabled,
            BuiltinExt::OnePassword => settings.integrations.one_password,
            BuiltinExt::EnteAuth => settings.integrations.ente_auth,
            BuiltinExt::Weather => settings.fetch_weather,
            BuiltinExt::SpeedTest => settings.extensions.speed_test,
            BuiltinExt::GitHub => settings.extensions.github,
            BuiltinExt::AiUsage => settings.extensions.ai_usage,
        }
    }

    pub fn set_on(self, settings: &mut Settings, on: bool) {
        match self {
            BuiltinExt::Files => settings.extensions.everything.enabled = on,
            BuiltinExt::OnePassword => settings.integrations.one_password = on,
            BuiltinExt::EnteAuth => settings.integrations.ente_auth = on,
            BuiltinExt::Weather => {
                settings.fetch_weather = on;
                if !on {
                    settings.weather_place = None;
                }
            }
            BuiltinExt::SpeedTest => settings.extensions.speed_test = on,
            BuiltinExt::GitHub => settings.extensions.github = on,
            BuiltinExt::AiUsage => settings.extensions.ai_usage = on,
        }
    }

    /// Orbit's own commands that belong to it, which go when it's turned off.
    pub const fn builtins(self) -> &'static [Builtin] {
        match self {
            BuiltinExt::Files => &[Builtin::SearchFiles],
            BuiltinExt::Weather => &[Builtin::Weather],
            BuiltinExt::SpeedTest => &[Builtin::SpeedTest],
            _ => &[],
        }
    }

    /// Its commands that answer with a list, as a folder extension's would.
    pub const fn list_commands(self) -> &'static [ListCommand] {
        match self {
            BuiltinExt::GitHub => GITHUB,
            BuiltinExt::AiUsage => AI_USAGE,
            _ => &[],
        }
    }

    pub fn list_command(self, id: &str) -> Option<&'static ListCommand> {
        self.list_commands().iter().find(|command| command.id == id)
    }

    /// Whether its commands answer with a list from Orbit's own code.
    pub const fn is_provider(self) -> bool {
        !self.list_commands().is_empty()
    }

    /// The extension a command of Orbit's own belongs to, if it belongs to one.
    pub fn of_builtin(command: Builtin) -> Option<BuiltinExt> {
        BuiltinExt::ALL.into_iter().find(|ext| ext.builtins().contains(&command))
    }
}

/// Whether one of Orbit's own commands is there to use: not when its extension is off.
pub fn builtin_available(settings: &Settings, command: Builtin) -> bool {
    BuiltinExt::of_builtin(command).is_none_or(|ext| ext.is_on(settings))
}

/// Whether an extension, built in or from a folder, is turned on.
pub fn extension_on(settings: &Settings, id: &str) -> bool {
    match BuiltinExt::from_id(id) {
        Some(ext) => ext.is_on(settings),
        None => settings.extensions.enabled.iter().any(|on| on == id),
    }
}

/// The keyword a list command answers to: the one set in Settings, or its own.
pub fn command_keyword<'a>(settings: &'a Settings, ext: &str, command: &str, own: &'a str) -> &'a str {
    settings.extensions.keywords.get(&format!("{ext}/{command}")).map_or(own, String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_found_again() {
        for ext in BuiltinExt::ALL {
            assert_eq!(BuiltinExt::from_id(ext.id()), Some(ext));
            assert!(crate::extension::Manifest::parse(&format!(r#"{{"id":"{}","name":"x","run":["a"],"commands":[{{"id":"a","title":"A"}}]}}"#, ext.id())).is_ok(), "{} is a valid id", ext.id());
        }
        let mut ids: Vec<_> = BuiltinExt::ALL.iter().map(|ext| ext.id()).collect();
        ids.dedup();
        assert_eq!(ids.len(), BuiltinExt::ALL.len());
    }

    #[test]
    fn turning_one_off_takes_its_commands_away() {
        let mut settings = Settings::default();
        assert!(builtin_available(&settings, Builtin::SpeedTest), "on by default, as before");
        assert!(builtin_available(&settings, Builtin::Weather));
        BuiltinExt::SpeedTest.set_on(&mut settings, false);
        assert!(!builtin_available(&settings, Builtin::SpeedTest));
        assert!(builtin_available(&settings, Builtin::Settings), "commands of no extension stay");
        BuiltinExt::Weather.set_on(&mut settings, false);
        assert!(!settings.fetch_weather, "Weather's switch is the one that lets it go online");
    }

    #[test]
    fn the_new_ones_start_off() {
        let settings = Settings::default();
        assert!(!BuiltinExt::GitHub.is_on(&settings));
        assert!(!BuiltinExt::AiUsage.is_on(&settings));
        assert!(BuiltinExt::Files.is_on(&settings));
    }

    #[test]
    fn list_commands_have_valid_keywords() {
        for ext in BuiltinExt::ALL {
            assert_eq!(ext.is_provider(), !ext.list_commands().is_empty());
            for command in ext.list_commands() {
                assert!(crate::extension::valid_keyword(command.keyword), "{}", command.keyword);
                assert_eq!(ext.list_command(command.id).map(|c| c.title), Some(command.title));
            }
        }
    }

    #[test]
    fn a_changed_keyword_wins() {
        let mut settings = Settings::default();
        assert_eq!(command_keyword(&settings, "github", "issues", "ghi"), "ghi");
        settings.extensions.keywords.insert("github/issues".into(), "is".into());
        assert_eq!(command_keyword(&settings, "github", "issues", "ghi"), "is");
        settings.extensions.keywords.insert("github/issues".into(), String::new());
        assert_eq!(command_keyword(&settings, "github", "issues", "ghi"), "", "an empty one means no keyword");
    }

    #[test]
    fn folder_extensions_are_on_only_when_turned_on() {
        let mut settings = Settings::default();
        assert!(!extension_on(&settings, "windows-settings"));
        settings.extensions.enabled.push("windows-settings".into());
        assert!(extension_on(&settings, "windows-settings"));
        assert!(extension_on(&settings, "files"));
    }
}
