-- Optional first-run onboarding wizard (#119).
--
-- `onboarding_completed_at` is set when the account finishes the wizard or
-- chooses "Don't show again"; clearing it restarts the wizard. `onboarding_step`
-- stores the canonical step index so a half-finished tour resumes across web
-- and desktop. Both are per-account preferences, never identity data: only the
-- account itself can change them through `PUT /api/auth/onboarding`, and the
-- administrator user PATCH ignores them.

ALTER TABLE users ADD COLUMN onboarding_completed_at TEXT;
ALTER TABLE users ADD COLUMN onboarding_step INTEGER NOT NULL DEFAULT 0;
