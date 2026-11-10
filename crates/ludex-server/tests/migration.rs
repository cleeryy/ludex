//! GameVault import tests (#6).
//!
//! The importer consumes a JSON document produced by
//! `scripts/gamevault-export.sql`; these tests build that document by hand and
//! assert the matching, mapping, idempotency and dry-run guarantees against a
//! real SQLite database.

use std::path::{Path, PathBuf};

use ludex_server::config::Config;
use ludex_server::gamevault::{self, ImportOptions, MatchStatus};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use tempfile::TempDir;

const NOW: &str = "2026-01-01T00:00:00Z";
const MIB: i64 = 1024 * 1024;

struct Fixture {
    _temp: TempDir,
    config: Config,
    pool: SqlitePool,
    dump_path: PathBuf,
}

impl Fixture {
    async fn new(dump: Value) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let games_dir = temp.path().join("games");
        let data_dir = temp.path().join("data");
        let media_dir = temp.path().join("media");
        for dir in [&games_dir, &data_dir, &media_dir] {
            std::fs::create_dir_all(dir).expect("mkdir");
        }
        let config = Config::for_local(games_dir, data_dir, media_dir);
        let pool = ludex_server::db::connect(&config.database_url)
            .await
            .expect("connect");
        ludex_server::roots::bootstrap(&pool, &config)
            .await
            .expect("bootstrap roots");
        let dump_path = temp.path().join("dump.json");
        std::fs::write(
            &dump_path,
            serde_json::to_vec_pretty(&dump).expect("serialise dump"),
        )
        .expect("write dump");
        Self {
            _temp: temp,
            config,
            pool,
            dump_path,
        }
    }

    async fn run(&self, dry_run: bool) -> gamevault::ImportOutcome {
        gamevault::run(ImportOptions {
            dump: self.dump_path.clone(),
            dry_run,
            config: self.config.clone(),
        })
        .await
        .expect("import")
    }

    async fn add_game(&self, title: &str) -> i64 {
        let result = sqlx::query(
            "INSERT INTO games (title, sort_title, created_at, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind(title)
        .bind(title.to_lowercase())
        .bind(NOW)
        .bind(NOW)
        .execute(&self.pool)
        .await
        .expect("insert game");
        result.last_insert_rowid()
    }

    async fn add_version(&self, game_id: i64, path: &str, size: i64) {
        sqlx::query(
            "INSERT INTO game_versions (game_id, root_id, file_path, file_name, size, indexed_at)
             VALUES (?, 1, ?, ?, ?, ?)",
        )
        .bind(game_id)
        .bind(path)
        .bind(
            Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        )
        .bind(size)
        .bind(NOW)
        .execute(&self.pool)
        .await
        .expect("insert version");
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&self.pool)
            .await
            .expect("count")
    }

    async fn game_description(&self, id: i64) -> Option<String> {
        sqlx::query_scalar::<_, Option<String>>("SELECT description FROM games WHERE id = ?")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .expect("description")
    }

    async fn progress_row(&self) -> (String, i64) {
        let row = sqlx::query("SELECT state, minutes_played FROM progress LIMIT 1")
            .fetch_one(&self.pool)
            .await
            .expect("progress row");
        (row.get("state"), row.get("minutes_played"))
    }
}

fn dump_with_one_game() -> Value {
    json!({
        "format": "gamevault-export",
        "version": 1,
        "users": [
            {"id": 1, "username": "gvplayer", "role": "USER", "activated": true},
            {"id": 2, "username": "gvadmin", "role": "ADMIN", "activated": false}
        ],
        "games": [
            {
                "id": 10,
                "title": "Hades",
                "description": "GameVault edited description",
                "providerSlug": "igdb",
                "providerId": "113112",
                "genres": ["Action", "Roguelike"],
                "versions": [
                    {"filePath": "Hades (2020).zip", "size": MIB * 2}
                ]
            }
        ],
        "progress": [
            {
                "userId": 1,
                "gameId": 10,
                "state": "COMPLETED",
                "minutesPlayed": 180,
                "lastPlayedAt": "2026-01-02T10:00:00Z"
            }
        ],
        "bookmarks": [{"userId": 1, "gameId": 10}],
        "news": [
            {
                "title": "Server maintenance",
                "body": "Back at 20:00",
                "authorId": 1,
                "createdAt": "2026-01-03T09:00:00Z"
            }
        ]
    })
}

#[tokio::test]
async fn dry_run_reports_without_writing() {
    let fixture = Fixture::new(dump_with_one_game()).await;
    let game_id = fixture.add_game("Hades").await;
    fixture
        .add_version(game_id, "Hades (2020).zip", MIB * 2)
        .await;

    let outcome = fixture.run(true).await;
    let report = &outcome.report;
    assert!(report.dry_run);
    assert_eq!(report.dump_format, "gamevault-export");
    assert_eq!(report.dump_version, 1);
    assert_eq!(report.matched, 1, "{report:#?}");
    assert_eq!(report.unmatched, 0);
    assert_eq!(report.ignored, 0);
    assert_eq!(report.users_created, 2);
    assert_eq!(report.progress_imported, 1);
    assert_eq!(report.bookmarks_imported, 1);
    assert_eq!(report.news_imported, 1);
    assert_eq!(report.metadata_updated, 1);
    assert!(
        outcome.credentials.is_empty(),
        "a dry run reveals no password"
    );

    // Nothing was written.
    assert_eq!(fixture.count("users").await, 0);
    assert_eq!(fixture.count("progress").await, 0);
    assert_eq!(fixture.count("bookmarks").await, 0);
    assert_eq!(fixture.count("news").await, 0);
    assert_eq!(fixture.game_description(game_id).await, None);
}

#[tokio::test]
async fn import_is_idempotent_and_maps_states() {
    let fixture = Fixture::new(dump_with_one_game()).await;
    let game_id = fixture.add_game("Hades").await;
    fixture
        .add_version(game_id, "Hades (2020).zip", MIB * 2)
        .await;

    let first = fixture.run(false).await;
    assert_eq!(first.report.matched, 1, "{:#?}", first.report);
    assert_eq!(first.report.users_created, 2);
    assert_eq!(first.report.progress_imported, 1);
    assert_eq!(first.report.bookmarks_imported, 1);
    assert_eq!(first.report.news_imported, 1);
    assert_eq!(
        first.credentials.len(),
        2,
        "one password per created account"
    );

    // Temporary passwords are real Argon2 credentials, never stored in clear.
    for credential in &first.credentials {
        let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE username = ?")
            .bind(&credential.username)
            .fetch_one(&fixture.pool)
            .await
            .expect("hash");
        assert!(ludex_server::auth::verify_password(
            &credential.password,
            &hash
        ));
        assert_ne!(hash, credential.password);
    }

    // A disabled GameVault account stays disabled.
    let activated: i64 =
        sqlx::query_scalar("SELECT activated FROM users WHERE username = 'gvadmin'")
            .fetch_one(&fixture.pool)
            .await
            .expect("activated");
    assert_eq!(activated, 0);

    let (state, minutes) = fixture.progress_row().await;
    assert_eq!(state, "completed");
    assert_eq!(minutes, 180);
    assert_eq!(
        fixture.game_description(game_id).await.as_deref(),
        Some("GameVault edited description")
    );

    // Re-running the same dump changes nothing and keeps passwords.
    let second = fixture.run(false).await;
    assert_eq!(second.report.users_created, 0);
    assert_eq!(second.report.users_matched, 2);
    assert_eq!(second.report.progress_imported, 0);
    assert_eq!(second.report.bookmarks_imported, 0);
    assert_eq!(second.report.news_imported, 0);
    assert_eq!(second.report.metadata_updated, 0);
    assert!(second.credentials.is_empty());
    assert_eq!(fixture.count("users").await, 2);
    assert_eq!(fixture.count("progress").await, 1);
    assert_eq!(fixture.count("bookmarks").await, 1);
    assert_eq!(fixture.count("news").await, 1);
}

#[tokio::test]
async fn matches_by_title_and_size_and_never_guesses() {
    let fixture = Fixture::new(json!({
        "format": "gamevault-export",
        "version": 1,
        "users": [],
        "games": [
            {"id": 1, "title": "Other Game", "versions": [{"size": 3 * MIB}]},
            {"id": 2, "title": "Same Game", "versions": [{"size": MIB}]},
            {"id": 3, "title": "Unknown Game", "versions": [{"filePath": "missing.zip", "size": MIB}]},
            {"id": 4, "title": "No Files"}
        ],
        "progress": [],
        "bookmarks": [],
        "news": []
    }))
    .await;

    let other = fixture.add_game("Other Game").await;
    fixture
        .add_version(other, "other/other-game.7z", 3 * MIB)
        .await;
    // Two games share the same title and archive size: ambiguous.
    let one = fixture.add_game("Same Game").await;
    fixture.add_version(one, "same/one.zip", MIB).await;
    let two = fixture.add_game("Same Game").await;
    fixture.add_version(two, "same/two.zip", MIB).await;

    let outcome = fixture.run(true).await;
    let statuses: Vec<(i64, MatchStatus)> = outcome
        .report
        .games
        .iter()
        .map(|game| (game.gamevault_id, game.status))
        .collect();
    assert_eq!(
        statuses,
        vec![
            (1, MatchStatus::Matched),
            (2, MatchStatus::Unmatched),
            (3, MatchStatus::Unmatched),
            (4, MatchStatus::Ignored),
        ],
        "{:#?}",
        outcome.report.games
    );
    assert_eq!(outcome.report.matched, 1);
    assert_eq!(outcome.report.unmatched, 2);
    assert_eq!(outcome.report.ignored, 1);
    let ambiguous = outcome
        .report
        .games
        .iter()
        .find(|game| game.gamevault_id == 2)
        .expect("ambiguous row");
    assert!(
        ambiguous
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("ambiguous"),
        "{ambiguous:#?}"
    );
}

#[tokio::test]
async fn rejects_invalid_documents() {
    let fixture = Fixture::new(json!({"format": "something-else", "version": 1})).await;
    let error = gamevault::run(ImportOptions {
        dump: fixture.dump_path.clone(),
        dry_run: true,
        config: fixture.config.clone(),
    })
    .await
    .expect_err("wrong format must fail");
    assert!(
        format!("{error:#}").contains("gamevault-export"),
        "{error:#}"
    );

    let future = Fixture::new(json!({"format": "gamevault-export", "version": 99})).await;
    let error = gamevault::run(ImportOptions {
        dump: future.dump_path.clone(),
        dry_run: true,
        config: future.config.clone(),
    })
    .await
    .expect_err("future contract versions must fail");
    assert!(format!("{error:#}").contains("version"), "{error:#}");

    let missing = Fixture::new(json!({})).await;
    let error = gamevault::run(ImportOptions {
        dump: missing.dump_path.with_file_name("absent.json"),
        dry_run: true,
        config: missing.config.clone(),
    })
    .await
    .expect_err("missing dump must fail");
    assert!(format!("{error:#}").contains("absent.json"), "{error:#}");
}
