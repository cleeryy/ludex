-- OIDC configured from the admin UI (#103), PostgreSQL edition.

CREATE TABLE IF NOT EXISTS oidc_settings (
    id                BIGINT PRIMARY KEY CHECK (id = 1),
    enabled           BIGINT NOT NULL DEFAULT 0,
    issuer            TEXT,
    client_id         TEXT,
    redirect_url      TEXT,
    public_base_url   TEXT,
    scopes            TEXT NOT NULL DEFAULT 'openid profile email',
    display_name      TEXT NOT NULL DEFAULT 'SSO',
    auto_create       BIGINT NOT NULL DEFAULT 1,
    default_role      TEXT NOT NULL DEFAULT 'user',
    groups_claim      TEXT NOT NULL DEFAULT 'groups',
    admin_groups      TEXT NOT NULL DEFAULT '[]',
    client_secret_enc TEXT,
    updated_at        TEXT NOT NULL
);
