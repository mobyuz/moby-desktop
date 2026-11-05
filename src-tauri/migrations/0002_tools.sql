-- Version 2: what the local tools need beyond version 1.
-- Reminders gain the state 'missed' (due while Moby was not running) and a notification time.
-- SQLite cannot change a CHECK in place, so the table is rebuilt; it holds no data before this stage.

CREATE TABLE reminders_new (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    text TEXT NOT NULL,
    due_at INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'done', 'missed', 'cancelled')),
    created_at INTEGER NOT NULL,
    notified_at INTEGER
);
INSERT INTO reminders_new (id, user_id, text, due_at, state, created_at)
    SELECT id, user_id, text, due_at, state, created_at FROM reminders;
DROP TABLE reminders;
ALTER TABLE reminders_new RENAME TO reminders;
CREATE INDEX reminders_by_user ON reminders (user_id, due_at);

-- Every lookup in the registry, the grants and the audit list is scoped to one account.
CREATE INDEX projects_by_user ON projects (user_id);
CREATE INDEX allowed_apps_by_user ON allowed_apps (user_id);
CREATE INDEX grants_by_resource ON grants (user_id, tool, resource_id);
CREATE INDEX audit_log_by_user ON audit_log (user_id, at DESC);
