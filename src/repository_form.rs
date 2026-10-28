//! Repository fields shared by Add Repository and the onboarding draft.

use gpui_kit::component::{
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::{App, AppContext as _, Div, Entity, ParentElement as _, Styled as _, Window, div};
use serde::{Deserialize, Serialize};

use crate::data::{CheckoutInspection, NewRepositoryInput};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RepositoryDraft {
    pub(crate) key: String,
    pub(crate) display_name: String,
    pub(crate) owner: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) tags: String,
    pub(crate) clone_url: String,
    pub(crate) base_branch: String,
}

impl RepositoryDraft {
    pub(crate) fn input(&self, space: &str) -> NewRepositoryInput {
        let present = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        };
        NewRepositoryInput {
            display_name: present(&self.display_name),
            owner: present(&self.owner),
            name: present(&self.name),
            description: present(&self.description),
            space: present(space),
            tags: self.tags.split(',').map(str::to_owned).collect(),
            clone_url: present(&self.clone_url),
            base_branch: present(&self.base_branch),
        }
    }

    pub(crate) fn refresh_from_checkout(
        &mut self,
        inspection: &CheckoutInspection,
        previous: Option<&Self>,
    ) -> Self {
        let detected = Self {
            key: inspection.suggested_key.clone(),
            display_name: inspection.name.clone(),
            owner: inspection.owner.clone().unwrap_or_default(),
            name: inspection.name.clone(),
            clone_url: inspection.clone_url.clone().unwrap_or_default(),
            base_branch: inspection.base_branch.clone(),
            ..Default::default()
        };
        for (field, value, previous) in [
            (&mut self.key, &detected.key, previous.map(|p| &p.key)),
            (
                &mut self.display_name,
                &detected.display_name,
                previous.map(|p| &p.display_name),
            ),
            (&mut self.owner, &detected.owner, previous.map(|p| &p.owner)),
            (&mut self.name, &detected.name, previous.map(|p| &p.name)),
            (
                &mut self.clone_url,
                &detected.clone_url,
                previous.map(|p| &p.clone_url),
            ),
            (
                &mut self.base_branch,
                &detected.base_branch,
                previous.map(|p| &p.base_branch),
            ),
        ] {
            // Refresh detected defaults when switching checkouts; keep actual edits.
            if field.trim().is_empty() || previous == Some(field) {
                *field = value.to_owned();
            }
        }
        detected
    }
}

pub(crate) struct RepositoryForm {
    pub(crate) key: Entity<InputState>,
    pub(crate) display_name: Entity<InputState>,
    pub(crate) owner: Entity<InputState>,
    pub(crate) name: Entity<InputState>,
    pub(crate) description: Entity<InputState>,
    pub(crate) tags: Entity<InputState>,
    pub(crate) clone_url: Entity<InputState>,
    pub(crate) base_branch: Entity<InputState>,
}

impl RepositoryForm {
    pub(crate) fn new(draft: &RepositoryDraft, window: &mut Window, cx: &mut App) -> Self {
        let mut input = |value: &str, placeholder: &str, cx: &mut App| {
            cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(value)
                    .placeholder(placeholder)
            })
        };
        Self {
            key: input(&draft.key, "e.g. croftkind-devcroft", cx),
            display_name: input(&draft.display_name, "e.g. Devcroft", cx),
            owner: input(&draft.owner, "e.g. croftkind", cx),
            name: input(&draft.name, "e.g. devcroft", cx),
            description: input(&draft.description, "What is this repository for?", cx),
            tags: input(&draft.tags, "Comma-separated, e.g. rust, desktop", cx),
            clone_url: input(
                &draft.clone_url,
                "e.g. git@github.com:croftkind/devcroft.git",
                cx,
            ),
            base_branch: input(&draft.base_branch, "e.g. main", cx),
        }
    }

    pub(crate) fn inputs(&self) -> [&Entity<InputState>; 8] {
        [
            &self.key,
            &self.display_name,
            &self.owner,
            &self.name,
            &self.description,
            &self.tags,
            &self.clone_url,
            &self.base_branch,
        ]
    }

    pub(crate) fn snapshot(&self, cx: &App) -> RepositoryDraft {
        let text = |state: &Entity<InputState>| state.read(cx).value().to_string();
        RepositoryDraft {
            key: text(&self.key),
            display_name: text(&self.display_name),
            owner: text(&self.owner),
            name: text(&self.name),
            description: text(&self.description),
            tags: text(&self.tags),
            clone_url: text(&self.clone_url),
            base_branch: text(&self.base_branch),
        }
    }

    pub(crate) fn set_values(&self, draft: &RepositoryDraft, window: &mut Window, cx: &mut App) {
        for (state, value) in self.inputs().into_iter().zip([
            &draft.key,
            &draft.display_name,
            &draft.owner,
            &draft.name,
            &draft.description,
            &draft.tags,
            &draft.clone_url,
            &draft.base_branch,
        ]) {
            if state.read(cx).value().as_ref() != value.as_str() {
                state.update(cx, |state, cx| state.set_value(value.clone(), window, cx));
            }
        }
    }

    pub(crate) fn apply_inspection(
        &self,
        inspection: &CheckoutInspection,
        previous: Option<&RepositoryDraft>,
        window: &mut Window,
        cx: &mut App,
    ) -> RepositoryDraft {
        let mut draft = self.snapshot(cx);
        let detected = draft.refresh_from_checkout(inspection, previous);
        self.set_values(&draft, window, cx);
        detected
    }

    pub(crate) fn field(label: &str, state: &Entity<InputState>) -> Div {
        v_flex()
            .gap_1()
            .child(div().text_xs().opacity(0.7).child(label.to_owned()))
            .child(Input::new(state).aria_label(label.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_trims_and_splits_tags() {
        let draft = RepositoryDraft {
            display_name: " Display ".into(),
            name: "name".into(),
            description: "  ".into(),
            tags: "rust, desktop ,,rust".into(),
            clone_url: " https://example.com/o/r.git ".into(),
            ..Default::default()
        };
        let input = draft.input("personal");
        assert_eq!(input.display_name.as_deref(), Some("Display"));
        assert!(input.owner.is_none());
        assert_eq!(input.name.as_deref(), Some("name"));
        assert!(input.description.is_none());
        assert_eq!(input.space.as_deref(), Some("personal"));
        assert_eq!(
            input.clone_url.as_deref(),
            Some("https://example.com/o/r.git")
        );
        assert!(input.base_branch.is_none());
        assert_eq!(input.tags, vec!["rust", " desktop ", "", "rust"]);
    }

    #[test]
    fn checkout_detection_fills_blanks_and_preserves_edits() {
        let inspection = CheckoutInspection {
            checkout_path: "/tmp/project".into(),
            remote_name: "origin".into(),
            clone_url: Some("https://example.com/team/project.git".into()),
            owner: Some("team".into()),
            name: "project".into(),
            base_branch: "main".into(),
            suggested_key: "team-project".into(),
        };
        let mut draft = RepositoryDraft {
            key: "my-project".into(),
            display_name: "My project".into(),
            base_branch: "develop".into(),
            description: "Keep me".into(),
            tags: "rust".into(),
            ..Default::default()
        };
        let detected = draft.refresh_from_checkout(&inspection, None);
        assert_eq!(draft.key, "my-project");
        assert_eq!(draft.display_name, "My project");
        assert_eq!(draft.base_branch, "develop");
        assert_eq!(draft.description, "Keep me");
        assert_eq!(draft.tags, "rust");
        assert_eq!(draft.owner, "team");
        assert_eq!(draft.name, "project");
        assert_eq!(draft.clone_url, inspection.clone_url.clone().unwrap());
        let next = CheckoutInspection {
            suggested_key: "other-project".into(),
            name: "other".into(),
            owner: Some("other-team".into()),
            clone_url: Some("https://example.com/other-team/other.git".into()),
            base_branch: "trunk".into(),
            ..inspection
        };
        draft.refresh_from_checkout(&next, Some(&detected));
        assert_eq!(draft.key, "my-project");
        assert_eq!(draft.display_name, "My project");
        assert_eq!(draft.base_branch, "develop");
        assert_eq!(draft.owner, "other-team");
        assert_eq!(draft.name, "other");
        assert_eq!(draft.clone_url, next.clone_url.unwrap());
    }
}
