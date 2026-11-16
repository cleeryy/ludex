//! Helpers shared by the transfer unit tests.

use std::sync::Arc;

use object_store::memory::InMemory;
use tokio::sync::mpsc;

use crate::models::VersionRow;
use crate::state::AppState;

/// Test state with archive storage replaced by an in-memory object store.
pub(crate) async fn state_with_memory_store() -> (tempfile::TempDir, AppState) {
    let temp = tempfile::tempdir().expect("tempdir");
    for name in ["games", "data", "media"] {
        std::fs::create_dir_all(temp.path().join(name)).expect("mkdir");
    }
    let config = crate::config::Config::for_local(
        temp.path().join("games"),
        temp.path().join("data"),
        temp.path().join("media"),
    );
    let (scan_tx, _scan_rx) = mpsc::channel(1);
    let mut state = AppState::for_tests(config, scan_tx).await;
    state.storage = crate::storage::ArchiveStorage::from_store(Arc::new(InMemory::new()));
    (temp, state)
}

/// A minimal `game_versions` row for transfer tests.
pub(crate) fn version_row(key: Option<String>, backend: &str) -> VersionRow {
    VersionRow {
        id: 1,
        root_id: None,
        file_path: "Cat Quest II.zip".to_string(),
        file_name: "Cat Quest II.zip".to_string(),
        size: 16,
        version: None,
        release_date: None,
        early_access: 0,
        missing: 0,
        root_unavailable: 0,
        archive_hash: None,
        content_sha256: None,
        indexed_at: None,
        updated_at: None,
        storage_backend: Some(backend.to_string()),
        object_key: key,
        etag: None,
    }
}
