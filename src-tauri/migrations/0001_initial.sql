-- Version 1: the whole local schema, so later stages add behaviour and not tables.
-- Times are Unix milliseconds. Only settings, conversations and messages are used so far.

-- One row for this computer. Retention is in days, and NULL keeps history until the person deletes it.
CREATE TABLE settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    language TEXT NOT NULL DEFAULT 'uz' CHECK (language IN ('uz', 'ru', 'en')),
    voice TEXT NOT NULL DEFAULT 'default',
    sensitivity INTEGER NOT NULL DEFAULT 50 CHECK (sensitivity BETWEEN 0 AND 100),
    terms_version TEXT,
    privacy_policy_version TEXT,
    retention_days INTEGER DEFAULT 30 CHECK (retention_days IS NULL OR retention_days BETWEEN 1 AND 3650)
);
INSERT INTO settings (id) VALUES (1);

-- Everything below that has a user_id belongs to one signed-in account and is never shown to another.
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX conversations_by_user ON conversations (user_id, updated_at DESC);

CREATE TABLE messages (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('user', 'moby')),
    text TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX messages_by_conversation ON messages (conversation_id, seq);

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE allowed_apps (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- level: once, for this session, or permanent (read-only tools only).
CREATE TABLE grants (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    level TEXT NOT NULL CHECK (level IN ('once', 'session', 'permanent_read')),
    expires_at INTEGER,
    revoked INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1)),
    created_at INTEGER NOT NULL
);

CREATE TABLE reminders (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    text TEXT NOT NULL,
    due_at INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'done', 'cancelled')),
    created_at INTEGER NOT NULL
);

-- What was decided, never what was in it: no file names, queries, or message text.
CREATE TABLE audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    resource_id TEXT,
    decision TEXT NOT NULL,
    at INTEGER NOT NULL,
    duration_ms INTEGER,
    result_code TEXT
);
CREATE INDEX audit_log_by_time ON audit_log (at);

-- A paired robot. Its pair_key is deliberately not stored here.
CREATE TABLE devices (
    device_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
