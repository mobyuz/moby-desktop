//! Grants that last until Moby closes or the person signs out. They live only in memory, are
//! filed under one account and one sign-in (`epoch`), and expire on their own after a few hours.
//! Permanent read grants live in the database (`registry.rs`).

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

pub const SESSION_TTL: Duration = Duration::from_secs(8 * 3600);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SessionGrant {
    pub user: String,
    #[serde(skip)]
    pub epoch: u64,
    pub tool: String,
    pub key: String,
}

#[derive(Default)]
pub struct SessionGrants {
    items: Mutex<Vec<(SessionGrant, Instant)>>,
}

impl SessionGrants {
    fn with<T>(&self, run: impl FnOnce(&mut Vec<(SessionGrant, Instant)>) -> T) -> T {
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        items.retain(|(_, expires)| *expires > now);
        run(&mut items)
    }

    pub fn add(&self, user: &str, epoch: u64, tool: &str, key: &str) {
        self.add_for(user, epoch, tool, key, SESSION_TTL);
    }

    pub(crate) fn add_for(&self, user: &str, epoch: u64, tool: &str, key: &str, ttl: Duration) {
        let grant = SessionGrant {
            user: user.to_owned(),
            epoch,
            tool: tool.to_owned(),
            key: key.to_owned(),
        };
        self.with(|items| {
            if !items.iter().any(|(g, _)| *g == grant) {
                items.push((grant, Instant::now() + ttl));
            }
        });
    }

    pub fn has(&self, user: &str, epoch: u64, tool: &str, key: &str) -> bool {
        self.with(|items| {
            items
                .iter()
                .any(|(g, _)| g.user == user && g.epoch == epoch && g.tool == tool && g.key == key)
        })
    }

    pub fn list(&self, user: &str, epoch: u64) -> Vec<SessionGrant> {
        self.with(|items| {
            items
                .iter()
                .filter(|(g, _)| g.user == user && g.epoch == epoch)
                .map(|(g, _)| g.clone())
                .collect()
        })
    }

    pub fn revoke(&self, user: &str, epoch: u64, tool: &str, key: &str) -> bool {
        self.with(|items| {
            let before = items.len();
            items.retain(|(g, _)| {
                !(g.user == user && g.epoch == epoch && g.tool == tool && g.key == key)
            });
            items.len() != before
        })
    }

    /// Drops every grant of a resource that was removed from the registry.
    pub fn forget_resource(&self, user: &str, resource: &str) {
        self.with(|items| {
            items
                .retain(|(g, _)| !(g.user == user && g.key.split('+').any(|part| part == resource)))
        });
    }

    pub fn clear(&self) {
        self.with(Vec::clear);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_belongs_to_one_account_and_one_sign_in() {
        let grants = SessionGrants::default();
        grants.add("alice", 1, "open_app", "app_1");
        assert!(grants.has("alice", 1, "open_app", "app_1"));
        assert!(
            !grants.has("bob", 1, "open_app", "app_1"),
            "another account"
        );
        assert!(
            !grants.has("alice", 2, "open_app", "app_1"),
            "after signing out and in again"
        );
        assert!(
            !grants.has("alice", 1, "open_project", "app_1"),
            "another tool"
        );
        assert!(
            !grants.has("alice", 1, "open_app", "app_2"),
            "another resource"
        );
        assert!(grants.list("bob", 1).is_empty());
    }

    #[test]
    fn grants_expire_revoke_and_clear() {
        let grants = SessionGrants::default();
        grants.add_for("alice", 1, "open_app", "app_1", Duration::from_millis(30));
        grants.add("alice", 1, "open_app", "app_2");
        std::thread::sleep(Duration::from_millis(60));
        assert!(!grants.has("alice", 1, "open_app", "app_1"), "expired");
        assert!(grants.revoke("alice", 1, "open_app", "app_2"));
        assert!(!grants.revoke("alice", 1, "open_app", "app_2"));
        grants.add("alice", 1, "open_project", "proj_1+app_1");
        grants.forget_resource("alice", "app_1");
        assert!(
            grants.list("alice", 1).is_empty(),
            "removing an app removes grants that mention it"
        );
        grants.add("alice", 1, "open_app", "app_3");
        grants.clear();
        assert!(grants.list("alice", 1).is_empty());
    }
}
