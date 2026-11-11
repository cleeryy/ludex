//! Backup and restore round-trips (#49).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use ludex_server::backup::{
    create_backup, restore_backup, verify_backup, BackupRequest, RestoreRequest,
};
use ludex_server::config::Config;
use ludex_server::db;
use ludex_server::storage::ArchiveStorage;
use object_store::memory::InMemory;
use sqlx::SqlitePool;
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    data: PathBuf,
    media: PathBuf,
    games: PathBuf,
    database_url: String,
}

async fn fixture() -> Fixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let data = temp.path().join("data");
    let media = temp.path().join("media");
    let games = temp.path().join("games");
    for directory in [&data, &media, &games] {
        std::fs::create_dir_all(directory).expect("mkdir");
    }
    std::fs::create_dir_all(media.join("covers")).expect("media dir");
    std::fs::write(media.join("covers/cat.jpg"), b"cover-bytes").expect("media file");
    std::fs::create_dir_all(data.join("saves/1/2")).expect("saves dir");
    std::fs::write(data.join("saves/1/2/3-save.zip"), b"save-bytes").expect("save file");

    let config = Config::for_local(games.clone(), data.clone(), media.clone());
    let pool = db::connect(&config.database_url).await.expect("db");
    seed(&pool, "Cat Quest II").await;
    pool.close().await;

    Fixture {
        temp,
        data,
        media,
        games,
        database_url: config.database_url,
    }
}

fn config(fixture: &Fixture) -> Config {
    Config::for_local(
        fixture.games.clone(),
        fixture.data.clone(),
        fixture.media.clone(),
    )
}

async fn seed(pool: &ludex_server::db::Database, title: &str) {
    let now = db::db_time(db::now());
    let game_id = ludex_server::db::query(
        "INSERT INTO games (title, sort_title, group_key, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(title)
    .bind(title.to_ascii_lowercase())
    .bind(title.to_ascii_lowercase())
    .bind(&now)
    .bind(&now)
    .returning_id(pool)
    .await
    .expect("insert game");
    ludex_server::db::query(
        "INSERT INTO game_versions
            (game_id, root_id, file_path, file_name, size, indexed_at)
         VALUES (?, NULL, ?, ?, 1024, ?)",
    )
    .bind(game_id)
    .bind(format!("{title}.zip"))
    .bind(format!("{title}.zip"))
    .bind(&now)
    .execute(pool)
    .await
    .expect("insert version");
}

fn request(fixture: &Fixture, output: PathBuf) -> BackupRequest {
    BackupRequest {
        data_dir: fixture.data.clone(),
        media_dir: fixture.media.clone(),
        games_dir: fixture.games.clone(),
        database_url: fixture.database_url.clone(),
        output: Some(output),
        dry_run: false,
        include_media: true,
        upload_s3: false,
        storage: None,
        config: Some(config(fixture)),
    }
}

async fn snapshot_pool(database: &Path) -> SqlitePool {
    // `db::connect` applies migrations; the snapshot already has the schema, so
    // use a plain read-only pool.
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .read_only(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open snapshot");
    pool
}

#[tokio::test]
async fn backup_and_restore_round_trip() {
    let fixture = fixture().await;
    let archive = fixture.temp.path().join("backups/backup.tar.gz");
    let report = create_backup(&request(&fixture, archive.clone()))
        .await
        .expect("backup");
    assert!(!report.dry_run);
    assert!(report.bytes > 0);
    assert!(archive.is_file());
    assert!(report.files >= 4, "db, config, media and save are archived");

    let manifest = verify_backup(&archive).await.expect("verify");
    assert_eq!(manifest.counts.games, 1);
    assert_eq!(manifest.counts.versions, 1);
    assert_eq!(manifest.format_version, 1);

    let restore_data = fixture.temp.path().join("restored/data");
    let restore_media = fixture.temp.path().join("restored/media");
    let report = restore_backup(&RestoreRequest {
        archive: archive.clone(),
        data_dir: restore_data.clone(),
        media_dir: restore_media.clone(),
        force: false,
        dry_run: false,
        include_media: true,
    })
    .await
    .expect("restore");
    assert_eq!(report.games, 1);
    assert_eq!(report.versions, 1);
    assert!(restore_data.join("ludex.db").is_file());
    assert_eq!(
        std::fs::read(restore_media.join("covers/cat.jpg")).expect("media"),
        b"cover-bytes"
    );
    assert_eq!(
        std::fs::read(restore_data.join("saves/1/2/3-save.zip")).expect("save"),
        b"save-bytes"
    );

    let pool = snapshot_pool(&restore_data.join("ludex.db")).await;
    let games = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM games")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(games, 1);
    pool.close().await;
}

#[tokio::test]
async fn restore_rejects_a_checksum_mismatch() {
    let fixture = fixture().await;
    let archive = fixture.temp.path().join("backups/backup.tar.gz");
    create_backup(&request(&fixture, archive.clone()))
        .await
        .expect("backup");

    let extracted = fixture.temp.path().join("extracted");
    std::fs::create_dir_all(&extracted).expect("mkdir");
    let file = std::fs::File::open(&archive).expect("open");
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    tar.unpack(&extracted).expect("unpack");

    std::fs::write(extracted.join("media/covers/cat.jpg"), b"tampered!!!").expect("tamper");
    let tampered = fixture.temp.path().join("tampered.tar.gz");
    write_tarball(&extracted, &tampered);

    let error = restore_backup(&RestoreRequest {
        archive: tampered,
        data_dir: fixture.temp.path().join("restored/data"),
        media_dir: fixture.temp.path().join("restored/media"),
        force: false,
        dry_run: false,
        include_media: true,
    })
    .await
    .expect_err("checksum mismatch");
    assert!(
        format!("{error:#}").contains("checksum mismatch"),
        "unexpected error: {error:#}"
    );
}

#[tokio::test]
async fn restore_dry_run_writes_nothing() {
    let fixture = fixture().await;
    let archive = fixture.temp.path().join("backups/backup.tar.gz");
    create_backup(&request(&fixture, archive.clone()))
        .await
        .expect("backup");

    let restore_data = fixture.temp.path().join("dry/data");
    let restore_media = fixture.temp.path().join("dry/media");
    let report = restore_backup(&RestoreRequest {
        archive,
        data_dir: restore_data.clone(),
        media_dir: restore_media.clone(),
        force: false,
        dry_run: true,
        include_media: true,
    })
    .await
    .expect("dry run");
    assert!(report.dry_run);
    assert_eq!(report.games, 1);
    assert!(!restore_data.exists());
    assert!(!restore_media.exists());
}

#[tokio::test]
async fn restore_refuses_non_empty_data_without_force() {
    let fixture = fixture().await;
    let archive = fixture.temp.path().join("backups/backup.tar.gz");
    create_backup(&request(&fixture, archive.clone()))
        .await
        .expect("backup");

    let target_data = fixture.temp.path().join("target/data");
    let target_media = fixture.temp.path().join("target/media");
    std::fs::create_dir_all(&target_data).expect("mkdir");
    std::fs::write(target_data.join("ludex.db"), b"existing").expect("existing db");

    let error = restore_backup(&RestoreRequest {
        archive: archive.clone(),
        data_dir: target_data.clone(),
        media_dir: target_media.clone(),
        force: false,
        dry_run: false,
        include_media: true,
    })
    .await
    .expect_err("must refuse");
    assert!(format!("{error:#}").contains("--force"));

    restore_backup(&RestoreRequest {
        archive,
        data_dir: target_data.clone(),
        media_dir: target_media,
        force: true,
        dry_run: false,
        include_media: true,
    })
    .await
    .expect("forced restore");
    let pool = snapshot_pool(&target_data.join("ludex.db")).await;
    let games = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM games")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(games, 1);
    pool.close().await;
}

#[tokio::test]
async fn backup_is_consistent_under_concurrent_writes() {
    let fixture = fixture().await;
    let pool = db::connect(&fixture.database_url).await.expect("pool");
    let writer_pool = pool.clone();
    let writer = tokio::spawn(async move {
        for index in 0..30 {
            seed(&writer_pool, &format!("Concurrent {index}")).await;
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
    });

    let archive = fixture.temp.path().join("backups/concurrent.tar.gz");
    let report = create_backup(&request(&fixture, archive.clone()))
        .await
        .expect("backup");
    assert!(report.files >= 3);
    writer.await.expect("writer");

    let snapshot = fixture.temp.path().join("snapshot.db");
    let file = std::fs::File::open(&archive).expect("open backup");
    let mut tar = tar::Archive::new(GzDecoder::new(file));
    for entry in tar.entries().expect("entries") {
        let mut entry = entry.expect("entry");
        if entry.path().expect("path") == Path::new("ludex.db") {
            entry.unpack(&snapshot).expect("unpack db");
        }
    }
    let pool = snapshot_pool(&snapshot).await;
    let integrity: String = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity");
    assert_eq!(integrity, "ok");
    let games = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM games")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert!(
        (1..=31).contains(&games),
        "unexpected snapshot size {games}"
    );
    pool.close().await;
}

#[tokio::test]
async fn backup_uploads_to_object_storage() {
    let fixture = fixture().await;
    let archive = fixture.temp.path().join("backups/upload.tar.gz");
    let storage = ArchiveStorage::from_store(Arc::new(InMemory::new()));
    let mut backup = request(&fixture, archive);
    backup.upload_s3 = true;
    backup.storage = Some(storage.clone());
    let report = create_backup(&backup).await.expect("backup");
    let key = report.uploaded_key.expect("uploaded key");
    assert!(key.starts_with("backups/"), "unexpected key {key}");
    let object = storage.open(&key, None).await.expect("object");
    assert!(object.size > 0);
}

fn write_tarball(source: &Path, destination: &Path) {
    let file = std::fs::File::create(destination).expect("create tar");
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for entry in walkdir::WalkDir::new(source).sort_by_file_name() {
        let entry = entry.expect("walk");
        if entry.file_type().is_file() {
            let relative = entry.path().strip_prefix(source).expect("relative");
            builder
                .append_path_with_name(entry.path(), relative)
                .expect("append");
        }
    }
    let encoder = builder.into_inner().expect("finish");
    encoder.finish().expect("finish gzip");
}
