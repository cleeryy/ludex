-- OIDC identity linking (PostgreSQL edition, mirror of 0002_oidc.sql).
--
-- A user is linked to at most one (issuer, subject) pair. Local accounts keep
-- their password hash; OIDC login never changes it.

ALTER TABLE users ADD COLUMN oidc_issuer TEXT;
ALTER TABLE users ADD COLUMN oidc_subject TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_oidc_identity
    ON users(oidc_issuer, oidc_subject)
    WHERE oidc_issuer IS NOT NULL AND oidc_subject IS NOT NULL;
