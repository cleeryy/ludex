-- Invitations, TOTP second factor and forced password resets (#166).
--
-- Invitation tokens are single-use by default and only ever stored as a
-- SHA-256 hash: a database dump cannot be replayed as a working link. The same
-- applies to MFA challenges and recovery codes.
--
-- TOTP secrets are stored in the clear because the server must be able to
-- compute the current code; they are never returned by any endpoint after
-- enrolment and never written to the audit trail.

CREATE TABLE IF NOT EXISTS invitations (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    token_hash  TEXT NOT NULL UNIQUE,
    role        TEXT NOT NULL DEFAULT 'user',
    email       TEXT,
    note        TEXT,
    created_by  INTEGER REFERENCES users(id) ON DELETE SET NULL,
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    max_uses    INTEGER NOT NULL DEFAULT 1,
    uses        INTEGER NOT NULL DEFAULT 0,
    last_used_at TEXT,
    revoked_at  TEXT
);

CREATE INDEX IF NOT EXISTS idx_invitations_expires_at ON invitations (expires_at);

-- One row per account that turned on a second factor.
CREATE TABLE IF NOT EXISTS user_mfa (
    user_id        INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    totp_secret    TEXT,
    enabled_at     TEXT,
    -- Last accepted TOTP time step, so a code cannot be replayed inside its
    -- 30 second window.
    last_used_step INTEGER,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS mfa_recovery_codes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash  TEXT NOT NULL,
    created_at TEXT NOT NULL,
    used_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_mfa_recovery_codes_user
    ON mfa_recovery_codes (user_id, used_at);

-- Short-lived login challenge issued after a correct password when the account
-- has a second factor. The row is created, never a session: no token exists
-- until the TOTP or recovery code is verified.
CREATE TABLE IF NOT EXISTS mfa_challenges (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    user_agent TEXT,
    ip         TEXT,
    attempts   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    used_at    TEXT
);

CREATE INDEX IF NOT EXISTS idx_mfa_challenges_expires_at ON mfa_challenges (expires_at);

-- Set by an administrator password reset; cleared by the account password
-- change. Advisory for clients (the web shows "choose a new password"), never
-- a reason to hand out fewer permissions.
ALTER TABLE users ADD COLUMN must_change_password INTEGER NOT NULL DEFAULT 0;
