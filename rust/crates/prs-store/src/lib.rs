#![forbid(unsafe_code)]

use prs_model::{ReviewActivityRecord, ReviewActivityStoreState, ReviewSessionState};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::Builder;
use thiserror::Error;
use tokio::sync::Mutex;

const CURRENT_REVIEW_SCHEMA: i32 = 5;
const CURRENT_ACTIVITY_SCHEMA: i32 = 1;
const REVIEW_COLLECTIONS: [&str; 5] = [
    "ViewedFiles",
    "ChangedFiles",
    "Comments",
    "RemoteComments",
    "Iterations",
];

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("saved state is invalid at {path}: {source}")]
    InvalidJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("session key is empty")]
    EmptySessionKey,
}

#[derive(Clone)]
pub struct ReviewStateStore {
    root: PathBuf,
    write_gate: Arc<Mutex<()>>,
}

impl ReviewStateStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            write_gate: Arc::new(Mutex::new(())),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn load(&self, session_key: &str) -> Result<Option<ReviewSessionState>, StoreError> {
        let path = self.path_for(session_key)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => parse_state(&path, &bytes).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn load_all(&self) -> Result<Vec<ReviewSessionState>, StoreError> {
        let mut directory = match tokio::fs::read_dir(&self.root).await {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };

        let mut states = Vec::new();
        while let Some(entry) = directory.next_entry().await? {
            let path = entry.path();
            let is_json = path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("json"));
            if !is_json || !entry.file_type().await?.is_file() {
                continue;
            }

            states.push(parse_state(&path, &tokio::fs::read(&path).await?)?);
        }

        states.sort_by_key(|state| std::cmp::Reverse(state.last_opened_utc));
        Ok(states)
    }

    pub async fn save(&self, state: &ReviewSessionState) -> Result<(), StoreError> {
        let _guard = self.write_gate.lock().await;
        let destination = self.path_for(&state.details.reference.session_key)?;
        atomic_json_write(&destination, state).await
    }

    pub async fn delete(&self, state: &ReviewSessionState) -> Result<(), StoreError> {
        let _guard = self.write_gate.lock().await;
        let path = self.path_for(&state.details.reference.session_key)?;
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn path_for(&self, session_key: &str) -> Result<PathBuf, StoreError> {
        if session_key.is_empty() {
            return Err(StoreError::EmptySessionKey);
        }

        let hash = Sha256::digest(session_key.as_bytes());
        let file_name = format!("{}.json", hex::encode(&hash[..12]));
        Ok(self.root.join(file_name))
    }
}

#[derive(Clone)]
pub struct ReviewActivityStore {
    path: PathBuf,
    write_gate: Arc<Mutex<()>>,
}

impl ReviewActivityStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            write_gate: Arc::new(Mutex::new(())),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn load(&self) -> Result<Vec<ReviewActivityRecord>, StoreError> {
        let bytes = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };

        Ok(parse_activity_state(&self.path, &bytes)?.records)
    }

    pub async fn record(&self, record: ReviewActivityRecord) -> Result<(), StoreError> {
        let _guard = self.write_gate.lock().await;
        let mut state = match tokio::fs::read(&self.path).await {
            Ok(bytes) => parse_activity_state(&self.path, &bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ReviewActivityStoreState::default()
            }
            Err(error) => return Err(error.into()),
        };

        if state.records.iter().any(|item| item.id == record.id) {
            return Ok(());
        }

        state.schema_version = state.schema_version.max(CURRENT_ACTIVITY_SCHEMA);
        state.records.push(record);
        atomic_json_write(&self.path, &state).await
    }
}

fn parse_state(path: &Path, bytes: &[u8]) -> Result<ReviewSessionState, StoreError> {
    let mut value = parse_with_defaults::<ReviewSessionState>(path, bytes, true)?;
    if let Value::Object(object) = &mut value {
        for key in REVIEW_COLLECTIONS {
            if matches!(object.get(key), Some(Value::Null)) {
                object.insert(key.to_owned(), Value::Array(Vec::new()));
            }
        }
    }

    let mut state: ReviewSessionState =
        serde_json::from_value(value).map_err(|source| StoreError::InvalidJson {
            path: path.to_path_buf(),
            source,
        })?;
    state.schema_version = state.schema_version.max(CURRENT_REVIEW_SCHEMA);
    Ok(state)
}

fn parse_activity_state(path: &Path, bytes: &[u8]) -> Result<ReviewActivityStoreState, StoreError> {
    let mut value = parse_with_defaults::<ReviewActivityStoreState>(path, bytes, false)?;
    if let Value::Object(object) = &mut value
        && matches!(object.get("Records"), Some(Value::Null))
    {
        object.insert("Records".to_owned(), Value::Array(Vec::new()));
    }

    let mut state: ReviewActivityStoreState =
        serde_json::from_value(value).map_err(|source| StoreError::InvalidJson {
            path: path.to_path_buf(),
            source,
        })?;
    state.schema_version = state.schema_version.max(CURRENT_ACTIVITY_SCHEMA);
    Ok(state)
}

fn parse_with_defaults<T>(
    path: &Path,
    bytes: &[u8],
    case_insensitive: bool,
) -> Result<Value, StoreError>
where
    T: Default + DeserializeOwned + Serialize,
{
    let mut value: Value =
        serde_json::from_slice(bytes).map_err(|source| StoreError::InvalidJson {
            path: path.to_path_buf(),
            source,
        })?;
    let defaults =
        serde_json::to_value(T::default()).map_err(|source| StoreError::InvalidJson {
            path: path.to_path_buf(),
            source,
        })?;
    fill_defaults(&mut value, &defaults, case_insensitive);
    Ok(value)
}

fn fill_defaults(value: &mut Value, defaults: &Value, case_insensitive: bool) {
    let (Value::Object(object), Value::Object(default_object)) = (value, defaults) else {
        return;
    };

    for (expected_key, default_value) in default_object {
        if let Some(actual_value) = object.get_mut(expected_key) {
            fill_defaults(actual_value, default_value, case_insensitive);
            continue;
        }

        let actual_key = case_insensitive.then(|| {
            object
                .keys()
                .find(|key| key.eq_ignore_ascii_case(expected_key))
                .cloned()
        });
        if let Some(actual_key) = actual_key.flatten() {
            let mut actual_value = object
                .remove(&actual_key)
                .expect("the matching key was just found");
            fill_defaults(&mut actual_value, default_value, case_insensitive);
            object.insert(expected_key.clone(), actual_value);
        } else {
            object.insert(expected_key.clone(), default_value.clone());
        }
    }
}

async fn atomic_json_write<T>(destination: &Path, value: &T) -> Result<(), StoreError>
where
    T: Serialize,
{
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await?;

    let json = serde_json::to_vec_pretty(value).map_err(|source| StoreError::InvalidJson {
        path: destination.to_path_buf(),
        source,
    })?;
    atomic_replace_bytes(parent.to_path_buf(), destination.to_path_buf(), json).await?;
    Ok(())
}

async fn atomic_replace_bytes(
    parent: PathBuf,
    destination: PathBuf,
    bytes: Vec<u8>,
) -> std::io::Result<()> {
    // Keep the temporary file on the destination filesystem. `NamedTempFile::persist`
    // atomically replaces an existing path: its Windows implementation uses
    // MoveFileExW(MOVEFILE_REPLACE_EXISTING), while Unix uses an atomic rename.
    // The platform-specific implementation and its unsafe boundary remain inside
    // the maintained tempfile crate. The file is synced before the atomic replacement.
    tokio::task::spawn_blocking(move || {
        let mut temporary = Builder::new()
            .prefix(".prs-store-")
            .suffix(".tmp")
            .tempfile_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&destination)
            .map_err(|error| error.error)?;
        Ok(())
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use prs_model::{
        PullRequestDetails, PullRequestReference, ReviewActivityKind, ReviewComment, ReviewProvider,
    };
    use serde_json::Value;

    fn session_key(number: u32) -> String {
        format!("github:example.test/octo-org/widget#{number}")
    }

    fn state(number: u32, last_opened: chrono::DateTime<Utc>) -> ReviewSessionState {
        ReviewSessionState {
            details: PullRequestDetails {
                reference: PullRequestReference {
                    provider: ReviewProvider::GitHub,
                    session_key: session_key(number),
                    repository: "widget".into(),
                    ..Default::default()
                },
                title: "Test review".into(),
                ..Default::default()
            },
            last_opened_utc: last_opened,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn review_state_round_trips_without_changing_last_opened() {
        let temp = tempfile::tempdir().unwrap();
        let store = ReviewStateStore::new(temp.path());
        let last_opened = Utc.with_ymd_and_hms(2026, 11, 20, 8, 30, 0).unwrap();
        let mut review = state(7, last_opened);
        review.notes = "Remember to check lifetime.".into();
        review.comments.push(ReviewComment {
            file_path: "src/Foo.cs".into(),
            line_number: 7,
            body: "Please guard the null case.".into(),
            ..Default::default()
        });

        store.save(&review).await.unwrap();
        review.notes = "Ordinary save".into();
        store.save(&review).await.unwrap();
        let restored = store
            .load(&review.details.reference.session_key)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(restored.notes, review.notes);
        assert_eq!(review.last_opened_utc, last_opened);
        assert_eq!(restored.last_opened_utc, last_opened);
        assert_eq!(restored.details.reference.provider, ReviewProvider::GitHub);
        assert_eq!(
            restored
                .comments
                .first()
                .map(|comment| comment.body.as_str()),
            Some("Please guard the null case.")
        );

        let json: Value = serde_json::from_slice(
            &tokio::fs::read(store.path_for(&session_key(7)).unwrap())
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(json["SchemaVersion"], 5);
        assert_eq!(json["Details"]["Reference"]["Provider"], "GitHub");
        assert_eq!(json["Comments"][0]["Status"], "Pending");
    }

    #[tokio::test]
    async fn load_all_sorts_by_last_opened_descending() {
        let temp = tempfile::tempdir().unwrap();
        let store = ReviewStateStore::new(temp.path());
        let older = Utc.with_ymd_and_hms(2026, 11, 19, 0, 0, 0).unwrap();
        let newer = Utc.with_ymd_and_hms(2026, 11, 20, 0, 0, 0).unwrap();
        store.save(&state(1, older)).await.unwrap();
        store.save(&state(2, newer)).await.unwrap();

        let states = store.load_all().await.unwrap();

        assert_eq!(states.len(), 2);
        assert_eq!(states[0].details.reference.session_key, session_key(2));
        assert_eq!(states[1].details.reference.session_key, session_key(1));
    }

    #[tokio::test]
    async fn invalid_state_reports_the_invalid_path() {
        let temp = tempfile::tempdir().unwrap();
        let store = ReviewStateStore::new(temp.path());
        let path = store.path_for(&session_key(3)).unwrap();
        tokio::fs::write(&path, br#"{"SchemaVersion":"invalid"}"#)
            .await
            .unwrap();

        let error = store.load(&session_key(3)).await.unwrap_err();

        assert!(matches!(
            error,
            StoreError::InvalidJson {
                path: actual,
                ..
            } if actual == path
        ));
    }

    #[tokio::test]
    async fn atomic_overwrite_leaves_only_valid_destination() {
        let temp = tempfile::tempdir().unwrap();
        let store = ReviewStateStore::new(temp.path());
        let mut review = state(4, Utc.with_ymd_and_hms(2026, 11, 20, 8, 30, 0).unwrap());
        review.notes = "first".into();
        store.save(&review).await.unwrap();
        review.notes = "second".into();

        store.save(&review).await.unwrap();

        let restored = store
            .load(&review.details.reference.session_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.notes, "second");
        let temporary_files = std::fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|value| value == "tmp"))
            .count();
        assert_eq!(temporary_files, 0);
    }

    #[tokio::test]
    async fn migration_fills_missing_collections_and_raises_schema() {
        let temp = tempfile::tempdir().unwrap();
        let store = ReviewStateStore::new(temp.path());
        let key = session_key(9);
        let legacy = serde_json::json!({
            "schemaVersion": 2,
            "details": {
                "reference": {
                    "provider": "GitHub",
                    "sessionKey": key,
                    "repository": "widget"
                }
            },
            "notes": "Legacy review",
            "viewedFiles": null
        });
        tokio::fs::write(
            store.path_for(&key).unwrap(),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .await
        .unwrap();

        let migrated = store.load(&key).await.unwrap().unwrap();

        assert_eq!(migrated.schema_version, 5);
        assert!(migrated.viewed_files.is_empty());
        assert!(migrated.changed_files.is_empty());
        assert!(migrated.comments.is_empty());
        assert!(migrated.remote_comments.is_empty());
        assert!(migrated.iterations.is_empty());
        assert_eq!(migrated.details.reference.session_key, key);
    }

    #[tokio::test]
    async fn activity_store_deduplicates_record_ids() {
        let temp = tempfile::tempdir().unwrap();
        let store = ReviewActivityStore::new(temp.path().join("review-activity.json"));
        let record = ReviewActivityRecord {
            id: "opened:one:2026-11-20".into(),
            session_key: "pr-1".into(),
            repository: "widget".into(),
            pull_request_number: 1,
            kind: ReviewActivityKind::Opened,
            at_utc: Utc.with_ymd_and_hms(2026, 11, 20, 12, 0, 0).unwrap(),
            ..Default::default()
        };

        store.record(record.clone()).await.unwrap();
        store.record(record).await.unwrap();

        let records = store.load().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "opened:one:2026-11-20");
        let json: Value =
            serde_json::from_slice(&tokio::fs::read(store.path()).await.unwrap()).unwrap();
        assert_eq!(json["SchemaVersion"], 1);
        assert_eq!(json["Records"][0]["Kind"], "Opened");
    }
}
