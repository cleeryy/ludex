-- OIDC configured from the admin UI (#103).
--
-- One row (id = 1) holds the non-secret provider settings. The client secret
-- is encrypted at rest with a key derived from the server secret
-- (HKDF-SHA256 -> AES-256-GCM) and stored as `v1.<base64url(nonce||ciphertext)>`
-- in `client_secret_enc`; no endpoint ever returns it. Rotating the server
-- secret (`LUDEX_JWT_SECRET` / `data/jwt.secret`) makes the stored value
-- undecryptable: the server then disables SSO until an administrator enters
-- the secret again.
--
-- Environment variables keep precedence per field; the row is only the
-- fallback for fields that are not set in the environment.

CREATE TABLE IF NOT EXISTS oidc_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled INTEGER NOT NULL DEFAULT 0,
    issuer TEXT,
    client_id TEXT,
    redirect_url TEXT,
    public_base_url TEXT,
    scopes TEXT NOT NULL DEFAULT 'openid profile email',
    display_name TEXT NOT NULL DEFAULT 'SSO',
    auto_create INTEGER NOT NULL DEFAULT 1,
    default_role TEXT NOT NULL DEFAULT 'user',
    groups_claim TEXT NOT NULL DEFAULT 'groups',
    admin_groups TEXT NOT NULL DEFAULT '[]',
    client_secret_enc TEXT,
    updated_at TEXT NOT NULL
);
