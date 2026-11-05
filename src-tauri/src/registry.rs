//! What the person has registered, allowed, scheduled and done, kept in the local database: the
//! folders and programs the tools may touch, the read grants on them, reminders, and the activity
//! log. Everything is scoped to one signed-in account. The model never sees a path from here, only
//! the ids and display names; the paths stay in this file's rows.
//!
//! Ids are `proj_<uuid>`, `app_<uuid>` and `root_<uuid>`, which are also valid protocol ids. A
//! project is its own search root: `root_X` names the same folder as `proj_X`.

use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use uuid::Uuid;

use crate::store::{Db, StoreError, check_account};
use crate::text;

pub const MAX_PROJECTS: i64 = 20;
pub const MAX_APPS: i64 = 20;
pub const MAX_PENDING_REMINDERS: i64 = 50;
pub const NAME_CHARS: usize = 60;
const AUDIT_LIST_LIMIT: i64 = 200;

/// The tools whose grants may be permanent: they only read.
pub const READ_TOOLS: [&str; 2] = ["git_status", "search_files"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct App {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Grant {
    pub id: String,
    pub tool: String,
    pub resource_id: String,
    /// `permanent_read`. Session grants live in memory and are not listed here.
    pub level: String,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditEntry {
    pub id: i64,
    pub tool: String,
    pub resource_id: Option<String>,
    pub decision: String,
    pub at: i64,
    pub duration_ms: Option<i64>,
    pub result_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reminder {
    pub id: String,
    pub text: String,
    pub due_at: i64,
    pub state: String,
    pub created_at: i64,
}

/// Metadata about one tool call. There is no field for content on purpose.
pub struct AuditRecord<'a> {
    pub tool: &'a str,
    pub resource_id: Option<&'a str>,
    pub decision: &'a str,
    pub duration_ms: Option<i64>,
    pub result_code: Option<&'a str>,
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().hyphenated())
}

/// The id of the search root that is the same folder as a project.
pub fn root_of(project_id: &str) -> Option<String> {
    project_id
        .strip_prefix("proj_")
        .map(|rest| format!("root_{rest}"))
}

/// The project that a search root stands for.
pub fn project_of_root(root_id: &str) -> Option<String> {
    root_id
        .strip_prefix("root_")
        .map(|rest| format!("proj_{rest}"))
}

/// `prefix_` and a canonical lowercase uuid, nothing else.
pub fn is_resource_id(prefix: &str, value: &str) -> bool {
    value
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('_'))
        .is_some_and(|rest| {
            Uuid::parse_str(rest).is_ok_and(|id| id.hyphenated().to_string() == rest)
        })
}

/// Audit fields are fixed vocabulary, not text: letters, digits and underscores. A caller that
/// tried to put content here would be stopped.
fn check_token(value: &str) -> Result<(), StoreError> {
    if !value.is_empty()
        && value.len() <= 40
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        Ok(())
    } else {
        Err(StoreError::Invalid("activity entry"))
    }
}

fn check_name(name: &str) -> Result<(), StoreError> {
    if name.trim().is_empty()
        || name.chars().count() > NAME_CHARS
        || name.chars().any(text::is_unsafe_char)
    {
        Err(StoreError::Invalid("name"))
    } else {
        Ok(())
    }
}

fn count_for(db: &Db, table: &str, user: &str) -> Result<i64, StoreError> {
    Ok(db.conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE user_id = ?1"),
        params![user],
        |row| row.get(0),
    )?)
}

impl Db {
    /// Registers a folder the person picked and gives it the read grants (git status and file-name
    /// search) in one step. The grants are revocable. `path` must already be a verified canonical path.
    pub(crate) fn add_project(
        &mut self,
        user: &str,
        name: &str,
        path: &str,
        now: i64,
    ) -> Result<Project, StoreError> {
        check_account(user)?;
        check_name(name)?;
        if count_for(self, "projects", user)? >= MAX_PROJECTS {
            return Err(StoreError::Full("projects"));
        }
        let lowered = path.to_lowercase();
        if self
            .projects(user)?
            .iter()
            .any(|p| p.path.to_lowercase() == lowered)
        {
            return Err(StoreError::Invalid("folder (it is already added)"));
        }
        let id = new_id("proj");
        let root = root_of(&id).ok_or(StoreError::Invalid("id"))?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO projects (id, user_id, name, path, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, user, name, path, now],
        )?;
        for (tool, resource) in [("git_status", &id), ("search_files", &root)] {
            tx.execute(
                "INSERT INTO grants (id, user_id, tool, resource_id, level, expires_at, revoked, created_at) VALUES (?1, ?2, ?3, ?4, 'permanent_read', NULL, 0, ?5)",
                params![Uuid::new_v4().hyphenated().to_string(), user, tool, resource, now],
            )?;
        }
        tx.commit()?;
        Ok(Project {
            id,
            name: name.to_owned(),
            path: path.to_owned(),
            created_at: now,
        })
    }

    pub(crate) fn projects(&self, user: &str) -> Result<Vec<Project>, StoreError> {
        check_account(user)?;
        let mut statement = self
            .conn
            .prepare("SELECT id, name, path, created_at FROM projects WHERE user_id = ?1 ORDER BY created_at, rowid")?;
        let rows = statement.query_map(params![user], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub(crate) fn project(&self, user: &str, id: &str) -> Result<Option<Project>, StoreError> {
        check_account(user)?;
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, path, created_at FROM projects WHERE user_id = ?1 AND id = ?2",
                params![user, id],
                |row| {
                    Ok(Project {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        path: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// Removes a project and every grant on it, as one step.
    pub(crate) fn remove_project(&mut self, user: &str, id: &str) -> Result<(), StoreError> {
        check_account(user)?;
        if !is_resource_id("proj", id) {
            return Err(StoreError::Invalid("id"));
        }
        let root = root_of(id).ok_or(StoreError::Invalid("id"))?;
        let tx = self.conn.transaction()?;
        let removed = tx.execute(
            "DELETE FROM projects WHERE user_id = ?1 AND id = ?2",
            params![user, id],
        )?;
        tx.execute(
            "DELETE FROM grants WHERE user_id = ?1 AND (resource_id = ?2 OR resource_id = ?3)",
            params![user, id, root],
        )?;
        tx.commit()?;
        if removed == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    pub(crate) fn add_app(
        &mut self,
        user: &str,
        name: &str,
        path: &str,
        now: i64,
    ) -> Result<App, StoreError> {
        check_account(user)?;
        check_name(name)?;
        if count_for(self, "allowed_apps", user)? >= MAX_APPS {
            return Err(StoreError::Full("programs"));
        }
        let lowered = path.to_lowercase();
        if self
            .apps(user)?
            .iter()
            .any(|a| a.path.to_lowercase() == lowered)
        {
            return Err(StoreError::Invalid("program (it is already added)"));
        }
        let id = new_id("app");
        self.conn.execute(
            "INSERT INTO allowed_apps (id, user_id, name, path, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, user, name, path, now],
        )?;
        Ok(App {
            id,
            name: name.to_owned(),
            path: path.to_owned(),
            created_at: now,
        })
    }

    pub(crate) fn apps(&self, user: &str) -> Result<Vec<App>, StoreError> {
        check_account(user)?;
        let mut statement = self
            .conn
            .prepare("SELECT id, name, path, created_at FROM allowed_apps WHERE user_id = ?1 ORDER BY created_at, rowid")?;
        let rows = statement.query_map(params![user], |row| {
            Ok(App {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub(crate) fn app(&self, user: &str, id: &str) -> Result<Option<App>, StoreError> {
        check_account(user)?;
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, path, created_at FROM allowed_apps WHERE user_id = ?1 AND id = ?2",
                params![user, id],
                |row| Ok(App { id: row.get(0)?, name: row.get(1)?, path: row.get(2)?, created_at: row.get(3)? }),
            )
            .optional()?)
    }

    pub(crate) fn remove_app(&mut self, user: &str, id: &str) -> Result<(), StoreError> {
        check_account(user)?;
        if !is_resource_id("app", id) {
            return Err(StoreError::Invalid("id"));
        }
        let tx = self.conn.transaction()?;
        let removed = tx.execute(
            "DELETE FROM allowed_apps WHERE user_id = ?1 AND id = ?2",
            params![user, id],
        )?;
        tx.execute(
            "DELETE FROM grants WHERE user_id = ?1 AND resource_id = ?2",
            params![user, id],
        )?;
        tx.commit()?;
        if removed == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    /// A permanent read grant. Only the read-only tools can have one, and only on a resource that
    /// exists. A new grant replaces an older active one for the same tool and resource.
    pub(crate) fn grant_read(
        &mut self,
        user: &str,
        tool: &str,
        resource_id: &str,
        expires_at: Option<i64>,
        now: i64,
    ) -> Result<Grant, StoreError> {
        check_account(user)?;
        if !READ_TOOLS.contains(&tool) {
            return Err(StoreError::Invalid("tool"));
        }
        let project = match tool {
            "git_status" => resource_id.to_owned(),
            _ => project_of_root(resource_id).ok_or(StoreError::Invalid("id"))?,
        };
        if !is_resource_id("proj", &project) || self.project(user, &project)?.is_none() {
            return Err(StoreError::NotFound);
        }
        if expires_at.is_some_and(|at| at <= now) {
            return Err(StoreError::Invalid("expiry"));
        }
        let id = Uuid::new_v4().hyphenated().to_string();
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE grants SET revoked = 1 WHERE user_id = ?1 AND tool = ?2 AND resource_id = ?3",
            params![user, tool, resource_id],
        )?;
        tx.execute(
            "INSERT INTO grants (id, user_id, tool, resource_id, level, expires_at, revoked, created_at) VALUES (?1, ?2, ?3, ?4, 'permanent_read', ?5, 0, ?6)",
            params![id, user, tool, resource_id, expires_at, now],
        )?;
        tx.commit()?;
        Ok(Grant {
            id,
            tool: tool.to_owned(),
            resource_id: resource_id.to_owned(),
            level: "permanent_read".into(),
            expires_at,
            created_at: now,
        })
    }

    /// Takes a grant back at once. Revoking something already revoked or unknown is an error.
    pub(crate) fn revoke_grant(&self, user: &str, grant_id: &str) -> Result<(), StoreError> {
        check_account(user)?;
        let changed = self.conn.execute(
            "UPDATE grants SET revoked = 1 WHERE user_id = ?1 AND id = ?2 AND revoked = 0",
            params![user, grant_id],
        )?;
        if changed == 0 {
            Err(StoreError::NotFound)
        } else {
            Ok(())
        }
    }

    /// Grants that are neither revoked nor expired.
    pub(crate) fn grants(&self, user: &str, now: i64) -> Result<Vec<Grant>, StoreError> {
        check_account(user)?;
        let mut statement = self.conn.prepare(
            "SELECT id, tool, resource_id, level, expires_at, created_at FROM grants WHERE user_id = ?1 AND revoked = 0 AND (expires_at IS NULL OR expires_at > ?2) ORDER BY created_at, rowid",
        )?;
        let rows = statement.query_map(params![user, now], |row| {
            Ok(Grant {
                id: row.get(0)?,
                tool: row.get(1)?,
                resource_id: row.get(2)?,
                level: row.get(3)?,
                expires_at: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub(crate) fn has_grant(
        &self,
        user: &str,
        tool: &str,
        resource_id: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        check_account(user)?;
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM grants WHERE user_id = ?1 AND tool = ?2 AND resource_id = ?3 AND level = 'permanent_read' AND revoked = 0 AND (expires_at IS NULL OR expires_at > ?4) LIMIT 1",
                params![user, tool, resource_id, now],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub(crate) fn audit_add(
        &self,
        user: &str,
        record: &AuditRecord<'_>,
        now: i64,
    ) -> Result<(), StoreError> {
        check_account(user)?;
        check_token(record.tool)?;
        check_token(record.decision)?;
        if let Some(code) = record.result_code {
            check_token(code)?;
        }
        if let Some(resource) = record.resource_id
            && (resource.len() > 64
                || !resource.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                }))
        {
            return Err(StoreError::Invalid("activity entry"));
        }
        self.conn.execute(
            "INSERT INTO audit_log (user_id, tool, resource_id, decision, at, duration_ms, result_code) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![user, record.tool, record.resource_id, record.decision, now, record.duration_ms, record.result_code],
        )?;
        Ok(())
    }

    /// Newest first.
    pub(crate) fn audit_list(&self, user: &str) -> Result<Vec<AuditEntry>, StoreError> {
        check_account(user)?;
        let mut statement = self.conn.prepare(
            "SELECT id, tool, resource_id, decision, at, duration_ms, result_code FROM audit_log WHERE user_id = ?1 ORDER BY at DESC, id DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![user, AUDIT_LIST_LIMIT], |row| {
            Ok(AuditEntry {
                id: row.get(0)?,
                tool: row.get(1)?,
                resource_id: row.get(2)?,
                decision: row.get(3)?,
                at: row.get(4)?,
                duration_ms: row.get(5)?,
                result_code: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Deletes this account's activity. Other accounts' entries stay. Freed pages are zeroed by
    /// `secure_delete`; the VACUUM also shrinks the file.
    pub(crate) fn audit_clear(&mut self, user: &str) -> Result<usize, StoreError> {
        check_account(user)?;
        let removed = self
            .conn
            .execute("DELETE FROM audit_log WHERE user_id = ?1", params![user])?;
        self.conn.execute_batch("VACUUM")?;
        Ok(removed)
    }

    pub(crate) fn add_reminder(
        &mut self,
        user: &str,
        text: &str,
        due_at: i64,
        now: i64,
    ) -> Result<String, StoreError> {
        check_account(user)?;
        if text.trim().is_empty()
            || text.chars().count() > 200
            || text.chars().any(|c| text::is_unsafe_char(c) && c != '\n')
        {
            return Err(StoreError::Invalid("reminder"));
        }
        let pending: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reminders WHERE user_id = ?1 AND state = 'pending'",
            params![user],
            |row| row.get(0),
        )?;
        if pending >= MAX_PENDING_REMINDERS {
            return Err(StoreError::Full("reminders"));
        }
        let id = new_id("rem");
        self.conn.execute(
            "INSERT INTO reminders (id, user_id, text, due_at, state, created_at) VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![id, user, text, due_at, now],
        )?;
        Ok(id)
    }

    /// Everything not cancelled, soonest first.
    pub(crate) fn reminders(&self, user: &str) -> Result<Vec<Reminder>, StoreError> {
        check_account(user)?;
        let mut statement = self.conn.prepare(
            "SELECT id, text, due_at, state, created_at FROM reminders WHERE user_id = ?1 AND state <> 'cancelled' ORDER BY due_at, rowid LIMIT 200",
        )?;
        let rows = statement.query_map(params![user], |row| {
            Ok(Reminder {
                id: row.get(0)?,
                text: row.get(1)?,
                due_at: row.get(2)?,
                state: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub(crate) fn delete_reminder(&self, user: &str, id: &str) -> Result<(), StoreError> {
        check_account(user)?;
        let removed = self.conn.execute(
            "DELETE FROM reminders WHERE user_id = ?1 AND id = ?2",
            params![user, id],
        )?;
        if removed == 0 {
            Err(StoreError::NotFound)
        } else {
            Ok(())
        }
    }

    /// Pending reminders that are due, oldest first.
    pub(crate) fn due_reminders(&self, user: &str, now: i64) -> Result<Vec<Reminder>, StoreError> {
        check_account(user)?;
        let mut statement = self.conn.prepare(
            "SELECT id, text, due_at, state, created_at FROM reminders WHERE user_id = ?1 AND state = 'pending' AND due_at <= ?2 ORDER BY due_at, rowid LIMIT 20",
        )?;
        let rows = statement.query_map(params![user, now], |row| {
            Ok(Reminder {
                id: row.get(0)?,
                text: row.get(1)?,
                due_at: row.get(2)?,
                state: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records that a pending reminder was shown. Returns false if it was no longer pending, so a
    /// reminder is never shown twice.
    pub(crate) fn finish_reminder(
        &self,
        user: &str,
        id: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        check_account(user)?;
        let changed = self.conn.execute(
            "UPDATE reminders SET state = 'done', notified_at = ?3 WHERE user_id = ?1 AND id = ?2 AND state = 'pending'",
            params![user, id, now],
        )?;
        Ok(changed == 1)
    }

    /// Pending reminders that came due before `cutoff` (Moby was not running) become 'missed'.
    pub(crate) fn mark_missed(&self, user: &str, cutoff: i64) -> Result<usize, StoreError> {
        check_account(user)?;
        Ok(self.conn.execute(
            "UPDATE reminders SET state = 'missed' WHERE user_id = ?1 AND state = 'pending' AND due_at < ?2",
            params![user, cutoff],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;

    const NOW: i64 = 1_800_000_000_000;

    fn open() -> (crate::paths::testing::Scratch, Db) {
        let scratch = crate::paths::testing::Scratch::new();
        let db = Db::open(&scratch.path("moby.db"), NOW).unwrap();
        (scratch, db)
    }

    fn column(db: &Db, sql: &str) -> Vec<String> {
        db.conn
            .prepare(sql)
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn ids_are_protocol_ids_and_roots_map_to_projects() {
        let id = new_id("proj");
        assert!(moby_protocol::StableId::new(id.clone()).is_ok());
        assert!(is_resource_id("proj", &id));
        let root = root_of(&id).unwrap();
        assert!(is_resource_id("root", &root));
        assert!(moby_protocol::StableId::new(root.clone()).is_ok());
        assert_eq!(project_of_root(&root).unwrap(), id);
        for bad in [
            "proj_x",
            "proj_",
            "app_123e4567-e89b-42d3-a456-426614174000",
            "proj_123E4567-E89B-42D3-A456-426614174000",
            "proj_../x",
        ] {
            assert!(!is_resource_id("proj", bad), "{bad}");
        }
        assert!(root_of("app_1").is_none());
    }

    #[test]
    fn a_project_comes_with_two_revocable_read_grants_and_is_scoped_to_its_account() {
        let (_dir, mut db) = open();
        let project = db
            .add_project("alice", "moby-api", r"C:\work\moby-api", NOW)
            .unwrap();
        let root = root_of(&project.id).unwrap();

        assert!(
            db.has_grant("alice", "git_status", &project.id, NOW)
                .unwrap()
        );
        assert!(db.has_grant("alice", "search_files", &root, NOW).unwrap());
        assert!(
            !db.has_grant("alice", "search_files", &project.id, NOW)
                .unwrap(),
            "a grant names one resource"
        );
        assert!(!db.has_grant("alice", "open_app", &project.id, NOW).unwrap());
        assert!(!db.has_grant("bob", "git_status", &project.id, NOW).unwrap());
        assert!(db.projects("bob").unwrap().is_empty());
        assert!(db.project("bob", &project.id).unwrap().is_none());
        assert!(matches!(
            db.remove_project("bob", &project.id),
            Err(StoreError::NotFound)
        ));

        let grants = db.grants("alice", NOW).unwrap();
        assert_eq!(grants.len(), 2);
        db.revoke_grant("alice", &grants[0].id).unwrap();
        assert!(
            matches!(
                db.revoke_grant("alice", &grants[0].id),
                Err(StoreError::NotFound)
            ),
            "revoking twice is an error"
        );
        assert!(
            matches!(
                db.revoke_grant("bob", &grants[1].id),
                Err(StoreError::NotFound)
            ),
            "another account cannot revoke it"
        );
        assert!(
            !db.has_grant("alice", &grants[0].tool, &grants[0].resource_id, NOW)
                .unwrap()
        );
        assert!(
            db.has_grant("alice", &grants[1].tool, &grants[1].resource_id, NOW)
                .unwrap()
        );
    }

    #[test]
    fn grants_expire_and_can_be_replaced() {
        let (_dir, mut db) = open();
        let project = db.add_project("alice", "p", r"C:\p", NOW).unwrap();
        let grant = db
            .grant_read("alice", "git_status", &project.id, Some(NOW + 1000), NOW)
            .unwrap();
        assert_eq!(grant.expires_at, Some(NOW + 1000));
        assert!(
            db.has_grant("alice", "git_status", &project.id, NOW + 999)
                .unwrap()
        );
        assert!(
            !db.has_grant("alice", "git_status", &project.id, NOW + 1000)
                .unwrap(),
            "expired exactly at its time"
        );
        assert!(
            !db.has_grant("alice", "git_status", &project.id, NOW + 5000)
                .unwrap()
        );
        assert!(
            db.grants("alice", NOW + 5000)
                .unwrap()
                .iter()
                .all(|g| g.tool != "git_status")
        );

        // Granting again replaces the old one, so only one active grant remains.
        let again = db
            .grant_read("alice", "git_status", &project.id, None, NOW + 6000)
            .unwrap();
        assert!(
            db.has_grant("alice", "git_status", &project.id, NOW + 7000)
                .unwrap()
        );
        let active: Vec<_> = db
            .grants("alice", NOW + 7000)
            .unwrap()
            .into_iter()
            .filter(|g| g.tool == "git_status")
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, again.id);
        assert!(
            db.grant_read("alice", "git_status", &project.id, Some(NOW), NOW)
                .is_err(),
            "an expiry in the past is refused"
        );
    }

    #[test]
    fn permanent_grants_exist_only_for_read_tools_on_real_resources() {
        let (_dir, mut db) = open();
        let project = db.add_project("alice", "p", r"C:\p", NOW).unwrap();
        for tool in ["open_app", "open_project", "create_reminder", "rm", ""] {
            assert!(
                db.grant_read("alice", tool, &project.id, None, NOW)
                    .is_err(),
                "{tool}"
            );
        }
        assert!(matches!(
            db.grant_read("alice", "git_status", &new_id("proj"), None, NOW),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            db.grant_read("bob", "git_status", &project.id, None, NOW),
            Err(StoreError::NotFound)
        ));
        assert!(
            db.grant_read(
                "alice",
                "search_files",
                &root_of(&project.id).unwrap(),
                None,
                NOW
            )
            .is_ok()
        );
        assert!(
            db.grant_read("alice", "search_files", &project.id, None, NOW)
                .is_err(),
            "search needs a root id"
        );
    }

    #[test]
    fn removing_a_project_removes_its_grants_and_nothing_else() {
        let (_dir, mut db) = open();
        let keep = db.add_project("alice", "keep", r"C:\keep", NOW).unwrap();
        let gone = db.add_project("alice", "gone", r"C:\gone", NOW).unwrap();
        db.remove_project("alice", &gone.id).unwrap();
        assert!(!db.has_grant("alice", "git_status", &gone.id, NOW).unwrap());
        assert!(
            !db.has_grant("alice", "search_files", &root_of(&gone.id).unwrap(), NOW)
                .unwrap()
        );
        assert!(db.has_grant("alice", "git_status", &keep.id, NOW).unwrap());
        assert_eq!(db.projects("alice").unwrap().len(), 1);
        assert!(db.remove_project("alice", "../x").is_err());
    }

    #[test]
    fn duplicates_limits_and_names_are_enforced() {
        let (_dir, mut db) = open();
        db.add_project("alice", "a", r"C:\Work\A", NOW).unwrap();
        assert!(
            db.add_project("alice", "again", r"c:\work\a", NOW).is_err(),
            "same folder, any case"
        );
        assert!(
            db.add_project("bob", "a", r"C:\Work\A", NOW).is_ok(),
            "another account may add it"
        );
        for bad in [
            "",
            "   ",
            "x\ny",
            "bidi\u{202E}",
            &"n".repeat(NAME_CHARS + 1),
        ] {
            assert!(
                db.add_project("alice", bad, r"C:\other", NOW).is_err(),
                "{bad:?}"
            );
        }
        for index in 1..MAX_PROJECTS {
            db.add_project("alice", "p", &format!(r"C:\many\{index}"), NOW)
                .unwrap();
        }
        assert!(matches!(
            db.add_project("alice", "one too many", r"C:\many\last", NOW),
            Err(StoreError::Full(_))
        ));

        db.add_app("alice", "Tool", r"C:\tools\tool.exe", NOW)
            .unwrap();
        assert!(
            db.add_app("alice", "Tool", r"C:\TOOLS\TOOL.EXE", NOW)
                .is_err()
        );
        assert!(db.apps("bob").unwrap().is_empty());
    }

    #[test]
    fn apps_are_scoped_and_removal_drops_session_irrelevant_rows() {
        let (_dir, mut db) = open();
        let app = db.add_app("alice", "Editor", r"C:\ed\ed.exe", NOW).unwrap();
        assert!(db.app("bob", &app.id).unwrap().is_none());
        assert!(matches!(
            db.remove_app("bob", &app.id),
            Err(StoreError::NotFound)
        ));
        db.remove_app("alice", &app.id).unwrap();
        assert!(db.app("alice", &app.id).unwrap().is_none());
        assert!(matches!(
            db.remove_app("alice", &app.id),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn the_activity_log_takes_metadata_only() {
        let (_dir, db) = open();
        let ok = AuditRecord {
            tool: "git_status",
            resource_id: Some("proj_123e4567-e89b-42d3-a456-426614174000"),
            decision: "auto",
            duration_ms: Some(12),
            result_code: Some("ok"),
        };
        db.audit_add("alice", &ok, NOW).unwrap();
        for record in [
            AuditRecord {
                tool: "git status in C:\\secret",
                ..ok_clone(&ok)
            },
            AuditRecord {
                decision: "approved because the file mentions passwords",
                ..ok_clone(&ok)
            },
            AuditRecord {
                result_code: Some("src/main.rs\nsecret"),
                ..ok_clone(&ok)
            },
            AuditRecord {
                resource_id: Some("C:\\Users\\me\\proj"),
                ..ok_clone(&ok)
            },
            AuditRecord {
                tool: "",
                ..ok_clone(&ok)
            },
        ] {
            assert!(db.audit_add("alice", &record, NOW).is_err());
        }
        assert_eq!(db.audit_list("alice").unwrap().len(), 1);
        assert!(db.audit_list("bob").unwrap().is_empty());
    }

    fn ok_clone<'a>(record: &AuditRecord<'a>) -> AuditRecord<'a> {
        AuditRecord {
            tool: record.tool,
            resource_id: record.resource_id,
            decision: record.decision,
            duration_ms: record.duration_ms,
            result_code: record.result_code,
        }
    }

    #[test]
    fn clearing_the_activity_log_removes_only_this_accounts_rows_and_their_bytes() {
        let (dir, mut db) = open();
        let record = AuditRecord {
            tool: "zq_marker_tool",
            resource_id: None,
            decision: "denied",
            duration_ms: None,
            result_code: None,
        };
        db.audit_add("alice", &record, NOW).unwrap();
        db.audit_add("alice", &record, NOW + 1).unwrap();
        db.audit_add(
            "bob",
            &AuditRecord {
                tool: "open_app",
                ..ok_clone(&record)
            },
            NOW,
        )
        .unwrap();
        let bytes = std::fs::read(dir.path("moby.db")).unwrap();
        assert!(bytes.windows(14).any(|w| w == b"zq_marker_tool"));

        assert_eq!(db.audit_clear("alice").unwrap(), 2);
        assert!(db.audit_list("alice").unwrap().is_empty());
        assert_eq!(db.audit_list("bob").unwrap().len(), 1);
        let bytes = std::fs::read(dir.path("moby.db")).unwrap();
        assert!(
            !bytes.windows(14).any(|w| w == b"zq_marker_tool"),
            "cleared entries must not stay in the file"
        );
    }

    #[test]
    fn the_activity_list_is_newest_first() {
        let (_dir, db) = open();
        for (index, tool) in ["git_status", "open_app", "search_files"]
            .into_iter()
            .enumerate()
        {
            db.audit_add(
                "alice",
                &AuditRecord {
                    tool,
                    resource_id: None,
                    decision: "auto",
                    duration_ms: None,
                    result_code: None,
                },
                NOW + i64::try_from(index).unwrap(),
            )
            .unwrap();
        }
        let tools: Vec<_> = db
            .audit_list("alice")
            .unwrap()
            .into_iter()
            .map(|e| e.tool)
            .collect();
        assert_eq!(tools, ["search_files", "open_app", "git_status"]);
    }

    #[test]
    fn reminders_fire_once_and_the_ones_missed_while_closed_are_marked() {
        let (_dir, mut db) = open();
        let soon = db
            .add_reminder("alice", "stand up", NOW + 1000, NOW)
            .unwrap();
        let old = db
            .add_reminder("alice", "while closed", NOW - 600_000, NOW - 700_000)
            .unwrap();
        db.add_reminder("bob", "not yours", NOW - 1, NOW - 10)
            .unwrap();

        assert_eq!(db.mark_missed("alice", NOW - 60_000).unwrap(), 1);
        let listed = db.reminders("alice").unwrap();
        assert_eq!(listed.iter().find(|r| r.id == old).unwrap().state, "missed");
        assert_eq!(
            listed.iter().find(|r| r.id == soon).unwrap().state,
            "pending"
        );
        assert!(db.due_reminders("alice", NOW).unwrap().is_empty());
        let due = db.due_reminders("alice", NOW + 1000).unwrap();
        assert_eq!(due.len(), 1);
        assert!(db.finish_reminder("alice", &soon, NOW + 1000).unwrap());
        assert!(
            !db.finish_reminder("alice", &soon, NOW + 1001).unwrap(),
            "never shown twice"
        );
        assert!(!db.finish_reminder("bob", &soon, NOW + 1001).unwrap());
        assert!(db.due_reminders("alice", NOW + 5000).unwrap().is_empty());
        assert_eq!(db.reminders("bob").unwrap().len(), 1);

        db.delete_reminder("alice", &old).unwrap();
        assert!(matches!(
            db.delete_reminder("alice", &old),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            db.delete_reminder("bob", &soon),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn reminder_text_and_count_are_limited() {
        let (_dir, mut db) = open();
        for bad in ["", "  ", "bidi\u{202E}text", "nul\u{0}", &"r".repeat(201)] {
            assert!(
                db.add_reminder("alice", bad, NOW + 1, NOW).is_err(),
                "{bad:?}"
            );
        }
        for _ in 0..MAX_PENDING_REMINDERS {
            db.add_reminder("alice", "x", NOW + 1, NOW).unwrap();
        }
        assert!(matches!(
            db.add_reminder("alice", "x", NOW + 1, NOW),
            Err(StoreError::Full(_))
        ));
    }

    #[test]
    fn deleting_an_account_removes_its_data_everywhere_and_only_its_data() {
        let (dir, mut db) = open();
        for user in ["alice", "bob"] {
            let project = db
                .add_project(user, "zq-marker-name", &format!(r"C:\{user}\p"), NOW)
                .unwrap();
            db.add_app(user, "App", &format!(r"C:\{user}\a.exe"), NOW)
                .unwrap();
            db.add_reminder(user, "zq-marker-reminder", NOW + 5, NOW)
                .unwrap();
            db.audit_add(
                user,
                &AuditRecord {
                    tool: "git_status",
                    resource_id: Some(&project.id),
                    decision: "auto",
                    duration_ms: None,
                    result_code: None,
                },
                NOW,
            )
            .unwrap();
            let conversation = db.create_conversation(user, "Talk", NOW).unwrap();
            db.append_message(
                user,
                &conversation.id,
                &crate::store::StoredMessage::for_test("zq-marker-text"),
                NOW,
            )
            .unwrap();
        }
        db.delete_account_data("alice").unwrap();

        for table in [
            "conversations",
            "projects",
            "allowed_apps",
            "grants",
            "reminders",
            "audit_log",
        ] {
            let owners = column(&db, &format!("SELECT DISTINCT user_id FROM {table}"));
            assert_eq!(owners, ["bob"], "{table}");
        }
        let messages = column(&db, "SELECT text FROM messages");
        assert_eq!(messages.len(), 1);
        assert!(
            db.settings().is_ok(),
            "settings belong to the computer and stay"
        );
        drop(db);
        let bytes = std::fs::read(dir.path("moby.db")).unwrap();
        assert_eq!(
            bytes
                .windows(14)
                .filter(|w| *w == b"zq-marker-text")
                .count(),
            1,
            "only bob's copy is left in the file"
        );
    }
}
