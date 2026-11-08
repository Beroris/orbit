//! Making or changing a quicklink or a snippet inside the launcher, instead of in a window of its
//! own. This is only what the form holds and how it's checked; the launcher shows it and types in
//! it with its own hidden boxes.

use orbit_core::links::{Field, check_link};
use orbit_core::settings::{CustomCommand, QuickLink};
use orbit_core::snippet::{TriggerProblem, validate_trigger};
use orbit_data::{Snippet, SnippetInput};

/// What snippets can hold besides plain text, offered under the snippet's box.
pub const PLACEHOLDERS: [(&str, &str); 4] = [("Date", "{date}"), ("Time", "{time}"), ("Clipboard", "{clipboard}"), ("Cursor", "{cursor}")];

#[derive(Clone, Debug, PartialEq)]
pub enum FormKind {
    /// Empty for a new quicklink, which gets its id when it's saved.
    Link { id: String },
    /// What the form doesn't show is kept as it was.
    Snippet { id: Option<i64>, description: String, category: String, favourite: bool, enabled: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FormField {
    pub label: &'static str,
    /// Shown in the box while it's empty.
    pub hint: &'static str,
    pub value: String,
    /// Takes several lines, so Enter is a new line rather than Save.
    pub multiline: bool,
    /// Picked from a list rather than typed. `value` is what's picked and `shown` its name.
    pub choice: bool,
    pub shown: String,
}

/// Where a quicklink's fields are in its form.
pub const LINK_TARGET: usize = 1;
pub const LINK_OPEN_WITH: usize = 2;
pub const LINK_ALIAS: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub struct InlineForm {
    pub kind: FormKind,
    pub title: String,
    pub fields: Vec<FormField>,
    /// The field being typed in.
    pub at: usize,
    /// Why the last Save didn't work, and which field it's about.
    pub problem: Option<(usize, String)>,
    /// The launcher was hidden when the form was asked for, so it hides again when it's done.
    pub hide_after: bool,
    /// Waiting to hear that a snippet was saved.
    pub saving: bool,
}

fn field(label: &'static str, hint: &'static str, value: String, multiline: bool) -> FormField {
    FormField { label, hint, value, multiline, choice: false, shown: String::new() }
}

impl InlineForm {
    /// A new quicklink, possibly with some of it filled in already, or one being changed.
    pub fn link(existing: Option<QuickLink>) -> InlineForm {
        let link = existing.unwrap_or(QuickLink { id: String::new(), name: String::new(), url: String::new(), alias: String::new(), open_with: String::new() });
        let title = if link.id.is_empty() { "Create Quicklink" } else { "Edit Quicklink" };
        InlineForm {
            kind: FormKind::Link { id: link.id },
            title: title.to_string(),
            fields: vec![
                field("Name", "Weather", link.name, false),
                field("Link", "A website, or a file or folder such as C:\\Projects", link.url, false),
                FormField { choice: true, ..field("Open with", "The usual app", link.open_with, false) },
                field("Alias", "Optional, such as w", link.alias, false),
            ],
            at: 0,
            problem: None,
            hide_after: false,
            saving: false,
        }
    }

    pub fn snippet(existing: Option<Snippet>) -> InlineForm {
        let title = if existing.is_some() { "Edit Snippet" } else { "Create Snippet" };
        let snippet = existing.unwrap_or(Snippet {
            id: 0,
            name: String::new(),
            trigger: String::new(),
            content: String::new(),
            description: String::new(),
            category: String::new(),
            favourite: false,
            enabled: true,
        });
        let id = (snippet.id != 0).then_some(snippet.id);
        InlineForm {
            kind: FormKind::Snippet { id, description: snippet.description, category: snippet.category, favourite: snippet.favourite, enabled: snippet.enabled },
            title: title.to_string(),
            fields: vec![
                field("Name", "Email sign-off", snippet.name, false),
                field("Snippet", "The text it inserts", snippet.content, true),
                field("Keyword", "Optional, such as ;sig", snippet.trigger, false),
            ],
            at: 0,
            problem: None,
            hide_after: false,
            saving: false,
        }
    }

    /// Picks the app a quicklink opens with: its id and name, or empty for the usual app.
    pub fn set_open_with(&mut self, id: &str, name: &str) {
        if let Some(field) = self.fields.get_mut(LINK_OPEN_WITH).filter(|field| field.choice) {
            field.value = id.to_string();
            field.shown = name.to_string();
        }
    }

    pub fn is_link(&self) -> bool {
        matches!(self.kind, FormKind::Link { .. })
    }

    /// The line under the fields: why Save didn't work, or else what the item will do.
    pub fn note(&self) -> String {
        if let Some((_, message)) = &self.problem {
            return message.clone();
        }
        match &self.kind {
            FormKind::Link { .. } => {
                let target = &self.fields[LINK_TARGET].value;
                let app = &self.fields[LINK_OPEN_WITH].shown;
                if orbit_core::links::is_path(target) {
                    let with = if app.is_empty() { "its usual app".to_string() } else { app.clone() };
                    format!("Opens {} with {with}.", target.trim().trim_matches('"'))
                } else {
                    let said = crate::link_form::describe_link(target, &self.fields[LINK_ALIAS].value);
                    if app.is_empty() || target.trim().is_empty() { said } else { format!("{said} Opens in {app}.") }
                }
            }
            FormKind::Snippet { .. } => {
                let keyword = self.fields[2].value.trim();
                if keyword.is_empty() {
                    "Find it by name in Orbit. A keyword also types it out anywhere, when expansion is on.".to_string()
                } else {
                    format!("Type {keyword} anywhere to insert it, when snippet expansion is on.")
                }
            }
        }
    }

    pub fn check_link(&self, links: &[QuickLink], commands: &[CustomCommand]) -> Result<QuickLink, (usize, String)> {
        let FormKind::Link { id } = &self.kind else { return Err((0, "This isn't a quicklink.".to_string())) };
        let link = QuickLink {
            id: id.clone(),
            name: self.fields[0].value.clone(),
            url: self.fields[LINK_TARGET].value.clone(),
            alias: self.fields[LINK_ALIAS].value.clone(),
            open_with: self.fields[LINK_OPEN_WITH].value.clone(),
        };
        check_link(link, links, commands).map_err(|(field, message)| {
            let at = match field {
                Field::Name => 0,
                Field::Target => LINK_TARGET,
                Field::Alias => LINK_ALIAS,
            };
            (at, message)
        })
    }

    pub fn check_snippet(&self) -> Result<(Option<i64>, SnippetInput), (usize, String)> {
        let FormKind::Snippet { id, description, category, favourite, enabled } = &self.kind else {
            return Err((0, "This isn't a snippet.".to_string()));
        };
        let name = self.fields[0].value.trim().to_string();
        let content = self.fields[1].value.replace("\r\n", "\n");
        let trigger = self.fields[2].value.trim().to_string();
        if name.is_empty() {
            return Err((0, "Give the snippet a name.".to_string()));
        }
        if content.trim().is_empty() {
            return Err((1, "Add the text this snippet inserts.".to_string()));
        }
        if !trigger.is_empty() {
            if let Err(problem) = validate_trigger(&trigger) {
                let message = match problem {
                    TriggerProblem::Empty => "The keyword is empty.",
                    TriggerProblem::TooShort => "Use at least two characters for a keyword, so ordinary typing won't set it off.",
                    TriggerProblem::TooLong => "That keyword is too long.",
                    TriggerProblem::ContainsWhitespace => "Keywords can't contain spaces.",
                };
                return Err((2, message.to_string()));
            }
        }
        let input = SnippetInput {
            name,
            trigger,
            content,
            description: description.clone(),
            category: category.clone(),
            favourite: *favourite,
            enabled: *enabled,
        };
        Ok((*id, input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(id: &str, name: &str, url: &str, alias: &str) -> QuickLink {
        QuickLink { id: id.into(), name: name.into(), url: url.into(), alias: alias.into(), open_with: String::new() }
    }

    #[test]
    fn a_quicklink_problem_points_at_its_field() {
        let mut form = InlineForm::link(None);
        assert_eq!(form.title, "Create Quicklink");
        assert_eq!(form.check_link(&[], &[]).unwrap_err().0, 0);
        form.fields[0].value = "Weather".into();
        assert_eq!(form.check_link(&[], &[]).unwrap_err().0, 1);
        form.fields[1].value = "weather.example/{city}".into();
        form.fields[LINK_ALIAS].value = "w".into();
        let taken = [link("link-x", "Wiki", "https://wiki.example", "w")];
        assert_eq!(form.check_link(&taken, &[]).unwrap_err().0, LINK_ALIAS);
        let saved = form.check_link(&[], &[]).unwrap();
        assert!(saved.id.starts_with("link-"));
        assert_eq!(saved.url, "https://weather.example/{city}");
    }

    #[test]
    fn a_quicklink_can_open_a_folder_with_a_chosen_app() {
        let mut form = InlineForm::link(None);
        assert!(form.fields[LINK_OPEN_WITH].choice);
        form.fields[0].value = "Orbit".into();
        form.fields[LINK_TARGET].value = r#""C:\Projects\orbit""#.into();
        form.set_open_with("code-id", "Visual Studio Code");
        assert_eq!(form.note(), r"Opens C:\Projects\orbit with Visual Studio Code.");
        let saved = form.check_link(&[], &[]).unwrap();
        assert_eq!((saved.url.as_str(), saved.open_with.as_str()), (r"C:\Projects\orbit", "code-id"));
        form.set_open_with("", "");
        assert_eq!(form.check_link(&[], &[]).unwrap().open_with, "", "back to the usual app");
    }

    #[test]
    fn changing_a_quicklink_keeps_its_id() {
        let form = InlineForm::link(Some(link("link-w", "Weather", "https://weather.example", "w")));
        assert_eq!(form.title, "Edit Quicklink");
        let saved = form.check_link(&[link("link-w", "Weather", "https://weather.example", "w")], &[]).unwrap();
        assert_eq!(saved.id, "link-w");
    }

    #[test]
    fn a_snippet_keeps_what_the_form_does_not_show() {
        let snippet = Snippet {
            id: 7,
            name: "Sig".into(),
            trigger: ";sig".into(),
            content: "Best,\nMe".into(),
            description: "Signs off".into(),
            category: "Mail".into(),
            favourite: true,
            enabled: false,
        };
        let mut form = InlineForm::snippet(Some(snippet));
        assert_eq!(form.title, "Edit Snippet");
        form.fields[1].value = "Best,\r\nYou".into();
        let (id, input) = form.check_snippet().unwrap();
        assert_eq!(id, Some(7));
        assert_eq!(input.content, "Best,\nYou");
        assert_eq!((input.category.as_str(), input.favourite, input.enabled), ("Mail", true, false));
    }

    #[test]
    fn a_snippet_problem_points_at_its_field() {
        let mut form = InlineForm::snippet(None);
        assert!(form.fields[1].multiline);
        assert_eq!(form.check_snippet().unwrap_err().0, 0);
        form.fields[0].value = "Sig".into();
        assert_eq!(form.check_snippet().unwrap_err().0, 1);
        form.fields[1].value = "Best".into();
        form.fields[2].value = "a b".into();
        assert_eq!(form.check_snippet().unwrap_err().0, 2);
        form.fields[2].value = String::new();
        assert_eq!(form.check_snippet().unwrap().0, None);
        assert!(form.note().contains("by name"));
    }
}
