//! Local memory: settings and conversation history in one SQLite file in the app data folder.
//! Only this module opens the file. The webview calls the narrow commands at the bottom, never
//! sees SQL, and never names an account: the signed-in account comes from the API client.
//!
//! All SQL is parameterized, every input is checked before it reaches the database, and message
//! text is never logged. If the file cannot be opened the app keeps running without it.

use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::api::{Api, Failure};

const DB_FILE: &str = "moby.db";
const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_initial.sql")];

const LANGUAGES: [&str; 3] = ["uz", "ru", "en"];
const ROLES: [&str; 2] = ["user", "moby"];
const MAX_TITLE_CHARS: usize = 80;
/// Above the API's 8 KiB per text, so a long reply still saves. The chat clips what it sends.
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_ACCOUNT_ID_BYTES: usize = 64;
const MAX_RETENTION_DAYS: u32 = 3650;
const LIST_LIMIT: i64 = 200;
const LOAD_LIMIT: i64 = 500;
const DAY_MS: i64 = 86_400_000;

#[derive(Debug)]
enum StoreError {
    Unavailable,
    Invalid(&'static str),
    NotFound,
    Database(rusqlite::Error),
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl From<StoreError> for Failure {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Unavailable => Failure::new(
                "store_unavailable",
                "Moby can't open its history on this computer, so nothing is saved.",
            ),
            StoreError::Invalid(what) => {
                Failure::new("invalid_input", &format!("That {what} isn't valid."))
            }
            StoreError::NotFound => {
                Failure::new("not_found", "That conversation is no longer here.")
            }
            StoreError::Database(error) => {
                // SQLite messages name tables and columns, never the stored values.
                eprintln!("moby: history database error: {error}");
                Failure::new(
                    "store_failed",
                    "Moby couldn't use its history file. Try again.",
                )
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Settings {
    language: String,
    /// None keeps history until the person deletes it.
    retention_days: Option<u32>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct ConversationSummary {
    id: String,
    title: String,
    updated_at: i64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredMessage {
    id: String,
    role: String,
    text: String,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Account ids come from the server's profile and become row keys, so keep them plain.
pub fn is_account_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ACCOUNT_ID_BYTES
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn check_account(value: &str) -> Result<(), StoreError> {
    if is_account_id(value) {
        Ok(())
    } else {
        Err(StoreError::Invalid("account"))
    }
}

/// Only canonical lowercase UUIDs, the form `crypto.randomUUID()` and this module produce.
fn check_id(value: &str) -> Result<(), StoreError> {
    match Uuid::parse_str(value) {
        Ok(parsed) if parsed.hyphenated().to_string() == value => Ok(()),
        _ => Err(StoreError::Invalid("id")),
    }
}

fn check_text(text: &str) -> Result<(), StoreError> {
    if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
        Err(StoreError::Invalid("message"))
    } else {
        Ok(())
    }
}

fn check_title(title: &str) -> Result<(), StoreError> {
    if title.trim().is_empty()
        || title.chars().count() > MAX_TITLE_CHARS
        || title.chars().any(char::is_control)
    {
        Err(StoreError::Invalid("title"))
    } else {
        Ok(())
    }
}

/// Applies each step once, in order, and records the version in the same transaction.
fn migrate(conn: &mut Connection, steps: &[&str]) -> Result<(), StoreError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let current = usize::try_from(current).unwrap_or(usize::MAX);
    if current > steps.len() {
        // A newer app wrote this file. Opening it could lose data, so refuse.
        return Err(StoreError::Invalid("history file version"));
    }
    for (index, sql) in steps.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i64::try_from(index + 1).unwrap_or(0))?;
        tx.commit()?;
    }
    Ok(())
}

struct Db {
    conn: Connection,
}

impl Db {
    /// Opens the file, brings the schema up to date, and applies retention.
    fn open(path: &Path, now: i64) -> Result<Self, StoreError> {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // Deleted rows are overwritten with zeros instead of left in free pages.
        conn.pragma_update(None, "secure_delete", true)?;
        migrate(&mut conn, MIGRATIONS)?;
        let mut db = Self { conn };
        db.enforce_retention(now)?;
        Ok(db)
    }

    fn settings(&self) -> Result<Settings, StoreError> {
        Ok(self.conn.query_row(
            "SELECT language, retention_days FROM settings WHERE id = 1",
            [],
            |row| {
                Ok(Settings {
                    language: row.get(0)?,
                    retention_days: row.get(1)?,
                })
            },
        )?)
    }

    fn set_language(&self, language: &str) -> Result<(), StoreError> {
        if !LANGUAGES.contains(&language) {
            return Err(StoreError::Invalid("language"));
        }
        self.conn.execute(
            "UPDATE settings SET language = ?1 WHERE id = 1",
            params![language],
        )?;
        Ok(())
    }

    /// Saves the setting and applies it now. Returns how many conversations it removed.
    fn set_retention(&mut self, days: Option<u32>, now: i64) -> Result<usize, StoreError> {
        if matches!(days, Some(days) if !(1..=MAX_RETENTION_DAYS).contains(&days)) {
            return Err(StoreError::Invalid("retention"));
        }
        self.conn.execute(
            "UPDATE settings SET retention_days = ?1 WHERE id = 1",
            params![days],
        )?;
        self.enforce_retention(now)
    }

    /// Drops conversations (and their messages) and audit entries older than the retention.
    fn enforce_retention(&mut self, now: i64) -> Result<usize, StoreError> {
        let Some(days) = self.settings()?.retention_days else {
            return Ok(0);
        };
        let cutoff = now.saturating_sub(i64::from(days) * DAY_MS);
        let tx = self.conn.transaction()?;
        let removed = tx.execute(
            "DELETE FROM conversations WHERE updated_at < ?1",
            params![cutoff],
        )?;
        tx.execute("DELETE FROM audit_log WHERE at < ?1", params![cutoff])?;
        tx.commit()?;
        Ok(removed)
    }

    fn require_conversation(&self, user: &str, id: &str) -> Result<(), StoreError> {
        self.conn
            .query_row(
                "SELECT 1 FROM conversations WHERE id = ?1 AND user_id = ?2",
                params![id, user],
                |_| Ok(()),
            )
            .optional()?
            .ok_or(StoreError::NotFound)
    }

    fn create_conversation(
        &self,
        user: &str,
        title: &str,
        now: i64,
    ) -> Result<ConversationSummary, StoreError> {
        check_account(user)?;
        check_title(title)?;
        let id = Uuid::new_v4().hyphenated().to_string();
        self.conn.execute(
            "INSERT INTO conversations (id, user_id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, user, title, now],
        )?;
        Ok(ConversationSummary {
            id,
            title: title.to_owned(),
            updated_at: now,
        })
    }

    fn list_conversations(&self, user: &str) -> Result<Vec<ConversationSummary>, StoreError> {
        check_account(user)?;
        let mut statement = self.conn.prepare(
            "SELECT id, title, updated_at FROM conversations WHERE user_id = ?1 ORDER BY updated_at DESC, rowid DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![user, LIST_LIMIT], |row| {
            Ok(ConversationSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The newest messages of one conversation, oldest first.
    fn load_messages(&self, user: &str, id: &str) -> Result<Vec<StoredMessage>, StoreError> {
        check_account(user)?;
        check_id(id)?;
        self.require_conversation(user, id)?;
        let mut statement = self.conn.prepare(
            "SELECT id, role, text FROM (SELECT seq, id, role, text FROM messages WHERE conversation_id = ?1 ORDER BY seq DESC LIMIT ?2) ORDER BY seq",
        )?;
        let rows = statement.query_map(params![id, LOAD_LIMIT], |row| {
            Ok(StoredMessage {
                id: row.get(0)?,
                role: row.get(1)?,
                text: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn append_message(
        &mut self,
        user: &str,
        conversation_id: &str,
        message: &StoredMessage,
        now: i64,
    ) -> Result<(), StoreError> {
        check_account(user)?;
        check_id(conversation_id)?;
        check_id(&message.id)?;
        if !ROLES.contains(&message.role.as_str()) {
            return Err(StoreError::Invalid("role"));
        }
        check_text(&message.text)?;
        let tx = self.conn.transaction()?;
        let touched = tx.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2 AND user_id = ?3",
            params![now, conversation_id, user],
        )?;
        if touched == 0 {
            return Err(StoreError::NotFound);
        }
        tx.execute(
            "INSERT INTO messages (id, conversation_id, role, text, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![message.id, conversation_id, message.role, message.text, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn delete_conversation(&self, user: &str, id: &str) -> Result<(), StoreError> {
        check_account(user)?;
        check_id(id)?;
        let removed = self.conn.execute(
            "DELETE FROM conversations WHERE id = ?1 AND user_id = ?2",
            params![id, user],
        )?;
        if removed == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    /// Removes every conversation, message and audit entry of every account on this computer.
    /// Settings stay. VACUUM then rebuilds the file so it also shrinks.
    fn delete_all_history(&mut self) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM messages; DELETE FROM conversations; DELETE FROM audit_log;",
        )?;
        tx.commit()?;
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }
}

/// The open database, or nothing when it could not be opened.
pub struct Store {
    db: Option<Mutex<Db>>,
}

impl Store {
    pub fn open(dir: &Path) -> Self {
        let opened = std::fs::create_dir_all(dir)
            .map_err(|error| eprintln!("moby: can't create the data folder: {error}"))
            .ok()
            .and_then(|()| {
                Db::open(&dir.join(DB_FILE), now_ms())
                    .map_err(|error| eprintln!("moby: history is not available: {error:?}"))
                    .ok()
            });
        Self {
            db: opened.map(Mutex::new),
        }
    }

    fn with<T>(&self, run: impl FnOnce(&mut Db) -> Result<T, StoreError>) -> Result<T, Failure> {
        let db = self.db.as_ref().ok_or(StoreError::Unavailable)?;
        // A panic while the lock was held leaves the SQLite file consistent, so keep going.
        let mut db = db.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(run(&mut db)?)
    }
}

#[tauri::command]
pub async fn get_settings(store: State<'_, Store>) -> Result<Settings, Failure> {
    store.with(|db| db.settings())
}

#[tauri::command]
pub async fn set_language(store: State<'_, Store>, language: String) -> Result<(), Failure> {
    store.with(|db| db.set_language(&language))
}

/// Returns how many conversations the new setting removed.
#[tauri::command]
pub async fn set_retention(store: State<'_, Store>, days: Option<u32>) -> Result<usize, Failure> {
    store.with(|db| db.set_retention(days, now_ms()))
}

#[tauri::command]
pub async fn list_conversations(
    api: State<'_, Api>,
    store: State<'_, Store>,
) -> Result<Vec<ConversationSummary>, Failure> {
    let user = api.account_id()?;
    store.with(|db| db.list_conversations(&user))
}

#[tauri::command]
pub async fn create_conversation(
    api: State<'_, Api>,
    store: State<'_, Store>,
    title: String,
) -> Result<ConversationSummary, Failure> {
    let user = api.account_id()?;
    store.with(|db| db.create_conversation(&user, &title, now_ms()))
}

#[tauri::command]
pub async fn load_messages(
    api: State<'_, Api>,
    store: State<'_, Store>,
    conversation_id: String,
) -> Result<Vec<StoredMessage>, Failure> {
    let user = api.account_id()?;
    store.with(|db| db.load_messages(&user, &conversation_id))
}

#[tauri::command]
pub async fn append_message(
    api: State<'_, Api>,
    store: State<'_, Store>,
    conversation_id: String,
    message: StoredMessage,
) -> Result<(), Failure> {
    let user = api.account_id()?;
    store.with(|db| db.append_message(&user, &conversation_id, &message, now_ms()))
}

#[tauri::command]
pub async fn delete_conversation(
    api: State<'_, Api>,
    store: State<'_, Store>,
    id: String,
) -> Result<(), Failure> {
    let user = api.account_id()?;
    store.with(|db| db.delete_conversation(&user, &id))
}

/// Works without an account, so it is also available before signing in.
#[tauri::command]
pub async fn delete_all_history(store: State<'_, Store>) -> Result<(), Failure> {
    store.with(Db::delete_all_history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const NOW: i64 = 1_800_000_000_000;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("moby-test-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join(DB_FILE)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn open() -> (TempDir, Db) {
        let dir = TempDir::new();
        let db = Db::open(&dir.file(), NOW).unwrap();
        (dir, db)
    }

    fn message(role: &str, text: &str) -> StoredMessage {
        StoredMessage {
            id: Uuid::new_v4().hyphenated().to_string(),
            role: role.into(),
            text: text.into(),
        }
    }

    fn count(db: &Db, table: &str) -> i64 {
        db.conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn file_contains(dir: &TempDir, needle: &str) -> bool {
        let bytes = std::fs::read(dir.file()).unwrap();
        bytes
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    #[test]
    fn a_new_file_gets_the_whole_schema_and_a_second_open_changes_nothing() {
        let (dir, db) = open();
        let tables: Vec<String> = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            tables,
            [
                "allowed_apps",
                "audit_log",
                "conversations",
                "devices",
                "grants",
                "messages",
                "projects",
                "reminders",
                "settings",
            ]
        );
        db.create_conversation("alice", "Hello", NOW).unwrap();
        drop(db);

        let again = Db::open(&dir.file(), NOW).unwrap();
        assert_eq!(count(&again, "conversations"), 1);
        let version: i64 = again
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, i64::try_from(MIGRATIONS.len()).unwrap());
    }

    #[test]
    fn migrations_run_in_order_keep_data_and_roll_back_a_failed_step() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(
            &mut conn,
            &["CREATE TABLE a (x TEXT); INSERT INTO a VALUES ('kept');"],
        )
        .unwrap();

        let steps = [
            "CREATE TABLE a (x TEXT); INSERT INTO a VALUES ('kept');",
            "CREATE TABLE b (y TEXT);",
            "CREATE TABLE c (z TEXT); THIS IS NOT SQL;",
        ];
        assert!(migrate(&mut conn, &steps).is_err());

        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2, "the good step stays, the failed one is undone");
        let kept: String = conn
            .query_row("SELECT x FROM a", [], |row| row.get(0))
            .unwrap();
        assert_eq!(kept, "kept");
        assert!(conn.prepare("SELECT * FROM b").is_ok());
        assert!(conn.prepare("SELECT * FROM c").is_err());
    }

    #[test]
    fn a_file_from_a_newer_app_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(
            migrate(&mut conn, MIGRATIONS),
            Err(StoreError::Invalid(_))
        ));
    }

    #[test]
    fn settings_start_with_the_agreed_defaults_and_reject_bad_values() {
        let (_dir, mut db) = open();
        let settings = db.settings().unwrap();
        assert_eq!(settings.language, "uz");
        assert_eq!(settings.retention_days, Some(30));

        db.set_language("ru").unwrap();
        db.set_retention(None, NOW).unwrap();
        let settings = db.settings().unwrap();
        assert_eq!(settings.language, "ru");
        assert_eq!(settings.retention_days, None);

        assert!(db.set_language("de").is_err());
        assert!(db.set_language("uz'; DROP TABLE settings;--").is_err());
        assert!(db.set_retention(Some(0), NOW).is_err());
        assert!(db.set_retention(Some(MAX_RETENTION_DAYS + 1), NOW).is_err());
        assert_eq!(db.settings().unwrap().language, "ru");
    }

    #[test]
    fn conversations_keep_their_messages_in_order_and_delete_with_them() {
        let (_dir, mut db) = open();
        let older = db.create_conversation("alice", "First", NOW).unwrap();
        let newer = db.create_conversation("alice", "Second", NOW + 1).unwrap();
        let first = message("user", "hi");
        let second = message("moby", "hello");
        db.append_message("alice", &older.id, &first, NOW + 2)
            .unwrap();
        db.append_message("alice", &older.id, &second, NOW + 3)
            .unwrap();

        let titles: Vec<String> = db
            .list_conversations("alice")
            .unwrap()
            .into_iter()
            .map(|c| c.title)
            .collect();
        assert_eq!(
            titles,
            ["First", "Second"],
            "a new message moves its conversation up"
        );

        let loaded = db.load_messages("alice", &older.id).unwrap();
        assert_eq!(loaded, [first, second]);
        assert!(db.load_messages("alice", &newer.id).unwrap().is_empty());

        db.delete_conversation("alice", &older.id).unwrap();
        assert_eq!(
            count(&db, "messages"),
            0,
            "messages go with their conversation"
        );
        assert_eq!(db.list_conversations("alice").unwrap().len(), 1);
        assert!(matches!(
            db.delete_conversation("alice", &older.id),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn one_account_cannot_see_change_or_delete_another_accounts_history() {
        let (_dir, mut db) = open();
        let theirs = db.create_conversation("alice", "Private", NOW).unwrap();
        let note = message("user", "alice only");
        db.append_message("alice", &theirs.id, &note, NOW).unwrap();

        assert!(db.list_conversations("bob").unwrap().is_empty());
        assert!(matches!(
            db.load_messages("bob", &theirs.id),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            db.append_message("bob", &theirs.id, &message("user", "intruder"), NOW),
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            db.delete_conversation("bob", &theirs.id),
            Err(StoreError::NotFound)
        ));

        assert_eq!(db.load_messages("alice", &theirs.id).unwrap(), [note]);
        assert_eq!(count(&db, "messages"), 1);
        assert_eq!(db.list_conversations("alice").unwrap().len(), 1);
    }

    #[test]
    fn retention_removes_only_what_is_older_and_runs_on_change_and_at_startup() {
        let (dir, mut db) = open();
        let old = db
            .create_conversation("alice", "Old", NOW - 31 * DAY_MS)
            .unwrap();
        db.create_conversation("alice", "Recent", NOW - 29 * DAY_MS)
            .unwrap();
        db.append_message(
            "alice",
            &old.id,
            &message("user", "old text"),
            NOW - 31 * DAY_MS,
        )
        .unwrap();
        db.conn
            .execute(
                "INSERT INTO audit_log (user_id, tool, decision, at) VALUES ('alice', 'git_status', 'allowed', ?1), ('alice', 'git_status', 'allowed', ?2)",
                params![NOW - 40 * DAY_MS, NOW - DAY_MS],
            )
            .unwrap();

        // The 30-day default is already in force, so a fresh start clears the old rows.
        drop(db);
        let mut db = Db::open(&dir.file(), NOW).unwrap();
        let titles: Vec<String> = db
            .list_conversations("alice")
            .unwrap()
            .into_iter()
            .map(|c| c.title)
            .collect();
        assert_eq!(titles, ["Recent"]);
        assert_eq!(count(&db, "messages"), 0);
        assert_eq!(count(&db, "audit_log"), 1);

        assert_eq!(db.set_retention(Some(7), NOW).unwrap(), 1);
        assert!(db.list_conversations("alice").unwrap().is_empty());

        // Keeping history for ever removes nothing.
        let kept = db
            .create_conversation("alice", "Ancient", NOW - 900 * DAY_MS)
            .unwrap();
        db.set_retention(None, NOW).unwrap();
        assert_eq!(db.enforce_retention(NOW + 1000 * DAY_MS).unwrap(), 0);
        assert_eq!(db.list_conversations("alice").unwrap()[0].id, kept.id);
    }

    #[test]
    fn delete_all_history_empties_every_history_table_and_keeps_settings() {
        let (dir, mut db) = open();
        db.set_language("en").unwrap();
        for user in ["alice", "bob"] {
            let conversation = db.create_conversation(user, "Talk", NOW).unwrap();
            db.append_message(
                user,
                &conversation.id,
                &message("user", "zq-marker-delete-all"),
                NOW,
            )
            .unwrap();
        }
        db.conn
            .execute(
                "INSERT INTO audit_log (user_id, tool, decision, at) VALUES ('alice', 'open_app', 'denied', ?1)",
                params![NOW],
            )
            .unwrap();
        assert!(
            file_contains(&dir, "zq-marker-delete-all"),
            "the check below would prove nothing otherwise"
        );

        db.delete_all_history().unwrap();

        assert_eq!(count(&db, "conversations"), 0);
        assert_eq!(count(&db, "messages"), 0);
        assert_eq!(count(&db, "audit_log"), 0);
        assert_eq!(db.settings().unwrap().language, "en");
        assert!(
            !file_contains(&dir, "zq-marker-delete-all"),
            "deleted text must not stay in the file"
        );
    }

    #[test]
    fn deleting_one_conversation_does_not_leave_its_text_in_the_file() {
        let (dir, mut db) = open();
        let conversation = db
            .create_conversation("alice", "zq-marker-title", NOW)
            .unwrap();
        db.append_message(
            "alice",
            &conversation.id,
            &message("user", "zq-marker-body"),
            NOW,
        )
        .unwrap();
        assert!(file_contains(&dir, "zq-marker-body"));
        assert!(file_contains(&dir, "zq-marker-title"));

        db.delete_conversation("alice", &conversation.id).unwrap();

        assert!(!file_contains(&dir, "zq-marker-body"));
        assert!(!file_contains(&dir, "zq-marker-title"));
    }

    #[test]
    fn hostile_text_is_stored_exactly_as_written() {
        let (_dir, mut db) = open();
        let title = "x'); DROP TABLE messages; --";
        let conversation = db.create_conversation("alice", title, NOW).unwrap();
        let texts = [
            "'; DELETE FROM conversations; --",
            "\" OR 1=1 --",
            "Robert'); DROP TABLE settings;--",
            "?1 ?2 :name @x $y %s {}",
            "emoji \u{1F40B} and \u{0}nul",
        ];
        for text in texts {
            db.append_message("alice", &conversation.id, &message("user", text), NOW)
                .unwrap();
        }

        let stored: Vec<String> = db
            .load_messages("alice", &conversation.id)
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect();
        assert_eq!(stored, texts);
        assert_eq!(db.list_conversations("alice").unwrap()[0].title, title);
        assert_eq!(count(&db, "conversations"), 1);
        assert_eq!(db.settings().unwrap().retention_days, Some(30));
    }

    #[test]
    fn bad_input_is_rejected_before_it_reaches_the_database() {
        let (_dir, mut db) = open();
        let conversation = db.create_conversation("alice", "Fine", NOW).unwrap();
        let id = conversation.id.as_str();

        let biggest = "a".repeat(MAX_TEXT_BYTES);
        db.append_message("alice", id, &message("user", &biggest), NOW)
            .unwrap();
        let too_big = "a".repeat(MAX_TEXT_BYTES + 1);
        let multibyte = "\u{1F40B}".repeat(MAX_TEXT_BYTES / 4 + 1);
        for text in [too_big.as_str(), multibyte.as_str(), "", "  \n "] {
            assert!(matches!(
                db.append_message("alice", id, &message("user", text), NOW),
                Err(StoreError::Invalid(_))
            ));
        }
        assert!(
            db.append_message("alice", id, &message("system", "hi"), NOW)
                .is_err()
        );
        assert!(
            db.append_message("alice", "../etc", &message("user", "hi"), NOW)
                .is_err()
        );
        assert!(
            db.append_message("alice", &id.to_uppercase(), &message("user", "hi"), NOW)
                .is_err()
        );
        let mut odd_id = message("user", "hi");
        odd_id.id = "not-a-uuid".into();
        assert!(db.append_message("alice", id, &odd_id, NOW).is_err());
        assert!(
            db.append_message("al ice", id, &message("user", "hi"), NOW)
                .is_err()
        );
        assert!(
            db.append_message("", id, &message("user", "hi"), NOW)
                .is_err()
        );
        assert_eq!(
            count(&db, "messages"),
            1,
            "only the one at the limit was saved"
        );

        assert!(db.create_conversation("alice", "", NOW).is_err());
        assert!(
            db.create_conversation("alice", &"t".repeat(MAX_TITLE_CHARS + 1), NOW)
                .is_err()
        );
        assert!(db.create_conversation("alice", "two\nlines", NOW).is_err());
        assert!(
            db.create_conversation(&"u".repeat(MAX_ACCOUNT_ID_BYTES + 1), "Fine", NOW)
                .is_err()
        );
        assert_eq!(count(&db, "conversations"), 1);
    }

    #[test]
    fn a_message_for_a_missing_conversation_is_not_saved() {
        let (_dir, mut db) = open();
        let missing = Uuid::new_v4().hyphenated().to_string();
        assert!(matches!(
            db.append_message("alice", &missing, &message("user", "hi"), NOW),
            Err(StoreError::NotFound)
        ));
        assert_eq!(count(&db, "messages"), 0);
    }

    #[test]
    fn a_missing_database_reports_unavailable_instead_of_panicking() {
        let dir = TempDir::new();
        // A file where the data folder should be makes the open fail.
        let blocker = dir.0.join("blocked");
        std::fs::write(&blocker, b"x").unwrap();
        let store = Store::open(&blocker);
        let failure = store.with(|db| db.settings()).unwrap_err();
        assert_eq!(failure.code(), "store_unavailable");
    }

    #[test]
    fn account_ids_are_plain_tokens() {
        assert!(is_account_id("7f3a90c2-1111-4222-8333-444455556666"));
        assert!(!is_account_id(""));
        assert!(!is_account_id("a/b"));
        assert!(!is_account_id("a b"));
        assert!(!is_account_id(&"a".repeat(MAX_ACCOUNT_ID_BYTES + 1)));
    }
}
