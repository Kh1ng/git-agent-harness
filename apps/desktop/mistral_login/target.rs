//! Browser storage is isolated for each named connection; the legacy default
//! keeps its existing sign-in store and private Cookie file.
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};
static CHECKS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub(super) struct Checking(String);
impl Drop for Checking {
    fn drop(&mut self) {
        if let Ok(mut checks) = CHECKS.get_or_init(Default::default).lock() {
            checks.remove(&self.0);
        }
    }
}
#[derive(Clone, Default)]
pub(super) struct LoginTarget {
    pub credential_id: Option<String>,
    pub account_label: Option<String>,
}

impl LoginTarget {
    pub fn start_check(&self) -> Option<Checking> {
        let label = self.window_label();
        let mut checks = CHECKS.get_or_init(Default::default).lock().ok()?;
        checks.insert(label.clone()).then(|| Checking(label))
    }
    pub fn new(id: Option<String>, label: Option<String>) -> Result<Self, String> {
        match (&id, &label) {
            (None, None) => Ok(Self::default()),
            (Some(id), Some(label))
                if !id.is_empty()
                    && id.len() <= 64
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                    && !label.trim().is_empty()
                    && label.len() <= 128
                    && !label.chars().any(char::is_control) =>
            {
                Ok(Self {
                    credential_id: Some(id.clone()),
                    account_label: Some(label.clone()),
                })
            }
            _ => Err("A valid named Mistral connection is required.".into()),
        }
    }

    pub fn window_label(&self) -> String {
        self.credential_id.as_ref().map_or_else(
            || super::WINDOW.to_owned(),
            |id| format!("{}-{id}", super::WINDOW),
        )
    }

    pub fn isolated(&self) -> bool {
        self.credential_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_connections_have_distinct_ephemeral_browser_targets() {
        let first = LoginTarget::new(Some("mistral-work".into()), Some("Work".into())).unwrap();
        let second =
            LoginTarget::new(Some("mistral-personal".into()), Some("Personal".into())).unwrap();
        assert!(first.isolated() && second.isolated());
        assert_ne!(first.window_label(), second.window_label());
        assert_ne!(first.window_label(), LoginTarget::default().window_label());
        assert!(!LoginTarget::default().isolated());
        for id in ["../other", "a/b", "", "provider.login", "a\n"] {
            assert!(LoginTarget::new(Some(id.into()), Some("Work".into())).is_err());
        }
        assert!(LoginTarget::new(Some("work".into()), None).is_err());
    }

    #[test]
    fn connection_checks_are_independent_and_cannot_overlap_the_same_slot() {
        let first = LoginTarget::new(Some("check-work".into()), Some("Work".into())).unwrap();
        let second =
            LoginTarget::new(Some("check-personal".into()), Some("Personal".into())).unwrap();
        let active = first.start_check().unwrap();
        assert!(first.start_check().is_none());
        assert!(second.start_check().is_some());
        drop(active);
        assert!(first.start_check().is_some());
    }
}
