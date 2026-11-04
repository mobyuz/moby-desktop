//! Reminders: a timer that runs in Rust and shows a Windows notification when one is due. The
//! webview takes no part, and notifications are shown only for the signed-in account.
//!
//! A reminder that came due while Moby was not running, or while nobody was signed in, is marked
//! 'missed' the next time its account is signed in, and the reminders list shows it as missed.

use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::api::Api;
use crate::registry::AuditRecord;
use crate::store::{Db, Store, StoreError, now_ms};

const TICK: Duration = Duration::from_secs(15);
/// Due this recently counts as on time; anything older was missed.
pub const GRACE_MS: i64 = 60_000;

/// Shows every reminder that is due for `user`, once each. Returns how many were shown.
pub(crate) fn fire_due(db: &Db, user: &str, now: i64, show: &mut dyn FnMut(&str)) -> Result<usize, StoreError> {
    let mut shown = 0;
    for reminder in db.due_reminders(user, now)? {
        // Marking first means a crash between the two steps loses a reminder instead of repeating it.
        if db.finish_reminder(user, &reminder.id, now)? {
            show(&reminder.text);
            shown += 1;
            let record = AuditRecord { tool: "reminder_due", resource_id: None, decision: "scheduled", duration_ms: None, result_code: Some("ok") };
            let _ = db.audit_add(user, &record, now);
        }
    }
    Ok(shown)
}

pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut checked: Option<(String, u64)> = None;
        loop {
            tokio::time::sleep(TICK).await;
            let api = app.state::<Api>();
            let Ok(user) = api.account_id() else {
                checked = None;
                continue;
            };
            let store = app.state::<Store>();
            let now = now_ms();
            let key = (user.clone(), api.epoch());
            if checked.as_ref() != Some(&key) {
                let _ = store.with(|db| db.mark_missed(&user, now - GRACE_MS));
                checked = Some(key);
            }
            let _ = store.with(|db| {
                fire_due(db, &user, now, &mut |text| {
                    let _ = app.notification().builder().title("Moby reminder").body(text).show();
                })
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::testing::Scratch;

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn a_due_reminder_is_shown_once_and_only_for_its_account() {
        let scratch = Scratch::new();
        let mut db = Db::open(&scratch.path("moby.db"), NOW).unwrap();
        db.add_reminder("alice", "stand up", NOW + 1000, NOW).unwrap();
        db.add_reminder("alice", "later", NOW + 100_000, NOW).unwrap();
        db.add_reminder("bob", "not alice", NOW + 1000, NOW).unwrap();
        let mut shown = Vec::new();
        assert_eq!(fire_due(&db, "alice", NOW, &mut |t| shown.push(t.to_owned())).unwrap(), 0);
        assert_eq!(fire_due(&db, "alice", NOW + 2000, &mut |t| shown.push(t.to_owned())).unwrap(), 1);
        assert_eq!(fire_due(&db, "alice", NOW + 3000, &mut |t| shown.push(t.to_owned())).unwrap(), 0, "never twice");
        assert_eq!(shown, ["stand up"]);
        assert_eq!(db.reminders("bob").unwrap()[0].state, "pending");
        let states: Vec<_> = db.reminders("alice").unwrap().into_iter().map(|r| r.state).collect();
        assert_eq!(states, ["done", "pending"]);
    }

    #[test]
    fn reminders_that_came_due_while_closed_show_as_missed_not_as_notifications() {
        let scratch = Scratch::new();
        let mut db = Db::open(&scratch.path("moby.db"), NOW).unwrap();
        db.add_reminder("alice", "while closed", NOW - 3_600_000, NOW - 4_000_000).unwrap();
        db.add_reminder("alice", "just now", NOW - 5_000, NOW - 10_000).unwrap();
        assert_eq!(db.mark_missed("alice", NOW - GRACE_MS).unwrap(), 1);
        let mut shown = Vec::new();
        fire_due(&db, "alice", NOW, &mut |t| shown.push(t.to_owned())).unwrap();
        assert_eq!(shown, ["just now"]);
        let states: Vec<_> = db.reminders("alice").unwrap().into_iter().map(|r| (r.text, r.state)).collect();
        assert_eq!(states[0], ("while closed".to_owned(), "missed".to_owned()));
    }
}
