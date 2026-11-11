-- Optional first-run onboarding wizard (#119), PostgreSQL edition.

ALTER TABLE users ADD COLUMN onboarding_completed_at TEXT;
ALTER TABLE users ADD COLUMN onboarding_step BIGINT NOT NULL DEFAULT 0;
