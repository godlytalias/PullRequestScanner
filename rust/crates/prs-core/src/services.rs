use chrono::{Datelike, Local, NaiveDate, Utc};
use prs_model::{ReviewActivityKind, ReviewActivityRecord, ReviewSessionState};
use prs_store::{ReviewActivityStore, ReviewStateStore, StoreError};
use std::collections::HashSet;
use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewActivityDay {
    pub date: NaiveDate,
    pub pull_request_count: i32,
    pub comment_count: i32,
    pub review_count: i32,
}

#[derive(Debug, Error)]
pub enum ReviewActivityServiceError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("invalid activity month {year}-{month:02}")]
    InvalidMonth { year: i32, month: u32 },
    #[error("activity count overflow for {date}")]
    CountOverflow { date: NaiveDate },
}

#[derive(Clone)]
pub struct ReviewActivityService {
    store: Arc<ReviewActivityStore>,
}

impl ReviewActivityService {
    #[must_use]
    pub fn new(store: impl Into<Arc<ReviewActivityStore>>) -> Self {
        Self {
            store: store.into(),
        }
    }

    #[must_use]
    pub fn store(&self) -> &ReviewActivityStore {
        &self.store
    }

    pub async fn record_opened(&self, state: &ReviewSessionState) -> Result<(), StoreError> {
        let at_utc = Utc::now();
        let id = format!(
            "opened:{}:{}",
            state.details.reference.session_key,
            at_utc.date_naive().format("%Y-%m-%d")
        );
        self.record(state, ReviewActivityKind::Opened, id, at_utc, 0)
            .await
    }

    pub async fn record_published_comment(
        &self,
        state: &ReviewSessionState,
        remote_id: &str,
    ) -> Result<(), StoreError> {
        self.record(
            state,
            ReviewActivityKind::CommentPublished,
            format!("comment:{remote_id}"),
            Utc::now(),
            1,
        )
        .await
    }

    pub async fn record_reply(
        &self,
        state: &ReviewSessionState,
        remote_id: &str,
    ) -> Result<(), StoreError> {
        self.record(
            state,
            ReviewActivityKind::ReplyPublished,
            format!("reply:{remote_id}"),
            Utc::now(),
            1,
        )
        .await
    }

    pub async fn record_reviewed(&self, state: &ReviewSessionState) -> Result<(), StoreError> {
        let id = format!(
            "reviewed:{}:{}",
            state.details.reference.session_key,
            state
                .last_reviewed_iteration_id
                .as_deref()
                .unwrap_or_default()
        );
        self.record(state, ReviewActivityKind::MarkedReviewed, id, Utc::now(), 0)
            .await
    }

    pub async fn record_completed(&self, state: &ReviewSessionState) -> Result<(), StoreError> {
        self.record(
            state,
            ReviewActivityKind::Completed,
            format!("completed:{}", state.details.reference.session_key),
            Utc::now(),
            0,
        )
        .await
    }

    pub async fn get_month(
        &self,
        year: i32,
        month: u32,
    ) -> Result<Vec<ReviewActivityDay>, ReviewActivityServiceError> {
        let start = NaiveDate::from_ymd_opt(year, month, 1)
            .filter(|date| (1..=9999).contains(&date.year()))
            .ok_or(ReviewActivityServiceError::InvalidMonth { year, month })?;
        let end = if month == 12 {
            NaiveDate::from_ymd_opt(year + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(year, month + 1, 1)
        }
        .filter(|date| (1..=9999).contains(&date.year()))
        .ok_or(ReviewActivityServiceError::InvalidMonth { year, month })?;

        let mut days = (0..(end - start).num_days())
            .map(|offset| DayAccumulator::new(start + chrono::Duration::days(offset)))
            .collect::<Vec<_>>();
        for record in self.store.load().await? {
            let date = record.at_utc.with_timezone(&Local).date_naive();
            if date < start || date >= end {
                continue;
            }

            let index = (date - start).num_days() as usize;
            let day = &mut days[index];
            match record.kind {
                ReviewActivityKind::Opened
                | ReviewActivityKind::Completed
                | ReviewActivityKind::MarkedReviewed => {
                    day.session_keys.insert(record.session_key);
                }
                ReviewActivityKind::CommentPublished | ReviewActivityKind::ReplyPublished => {}
            }
            day.comment_count = day
                .comment_count
                .checked_add(record.comment_delta)
                .ok_or(ReviewActivityServiceError::CountOverflow { date })?;
            if record.kind == ReviewActivityKind::MarkedReviewed {
                day.review_count = day
                    .review_count
                    .checked_add(1)
                    .ok_or(ReviewActivityServiceError::CountOverflow { date })?;
            }
        }

        Ok(days.into_iter().map(DayAccumulator::finish).collect())
    }

    pub async fn get_recent_reviews(
        &self,
        limit: usize,
    ) -> Result<Vec<ReviewActivityRecord>, StoreError> {
        let mut records = self
            .store
            .load()
            .await?
            .into_iter()
            .filter(|record| {
                matches!(
                    record.kind,
                    ReviewActivityKind::MarkedReviewed | ReviewActivityKind::Completed
                )
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|record| std::cmp::Reverse(record.at_utc));

        let mut session_keys = HashSet::new();
        Ok(records
            .into_iter()
            .filter(|record| session_keys.insert(record.session_key.clone()))
            .take(limit)
            .collect())
    }

    async fn record(
        &self,
        state: &ReviewSessionState,
        kind: ReviewActivityKind,
        id: String,
        at_utc: chrono::DateTime<Utc>,
        comment_delta: i32,
    ) -> Result<(), StoreError> {
        self.store
            .record(ReviewActivityRecord {
                id,
                session_key: state.details.reference.session_key.clone(),
                provider: state.details.reference.provider,
                repository: state.details.reference.repository.clone(),
                pull_request_number: state.details.reference.number,
                pull_request_title: state.details.title.clone(),
                kind,
                at_utc,
                comment_delta,
            })
            .await
    }
}

#[derive(Debug, Error)]
pub enum ReviewSessionServiceError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("this pull request has no review iterations yet")]
    NoReviewIterations,
}

#[derive(Clone)]
pub struct ReviewSessionService {
    store: Arc<ReviewStateStore>,
    activity: ReviewActivityService,
}

impl ReviewSessionService {
    #[must_use]
    pub fn new(store: impl Into<Arc<ReviewStateStore>>, activity: ReviewActivityService) -> Self {
        Self {
            store: store.into(),
            activity,
        }
    }

    #[must_use]
    pub fn store(&self) -> &ReviewStateStore {
        &self.store
    }

    #[must_use]
    pub fn activity(&self) -> &ReviewActivityService {
        &self.activity
    }

    pub async fn load(&self, session_key: &str) -> Result<Option<ReviewSessionState>, StoreError> {
        self.store.load(session_key).await
    }

    pub async fn load_all(&self) -> Result<Vec<ReviewSessionState>, StoreError> {
        self.store.load_all().await
    }

    pub async fn save(&self, state: &ReviewSessionState) -> Result<(), StoreError> {
        self.store.save(state).await
    }

    pub async fn delete(&self, state: &ReviewSessionState) -> Result<(), StoreError> {
        self.store.delete(state).await
    }

    pub async fn open(
        &self,
        state: &mut ReviewSessionState,
    ) -> Result<(), ReviewSessionServiceError> {
        state.last_opened_utc = Utc::now();
        self.store.save(state).await?;
        self.activity.record_opened(state).await?;
        Ok(())
    }

    pub async fn mark_iteration_reviewed(
        &self,
        state: &mut ReviewSessionState,
    ) -> Result<(), ReviewSessionServiceError> {
        let latest_id = state
            .iterations
            .last()
            .map(|iteration| iteration.id.clone())
            .ok_or(ReviewSessionServiceError::NoReviewIterations)?;
        state.last_reviewed_iteration_id = Some(latest_id);
        state.last_reviewed_at_utc = Some(Utc::now());
        self.store.save(state).await?;
        self.activity.record_reviewed(state).await?;
        Ok(())
    }
}

struct DayAccumulator {
    date: NaiveDate,
    session_keys: HashSet<String>,
    comment_count: i32,
    review_count: i32,
}

impl DayAccumulator {
    fn new(date: NaiveDate) -> Self {
        Self {
            date,
            session_keys: HashSet::new(),
            comment_count: 0,
            review_count: 0,
        }
    }

    fn finish(self) -> ReviewActivityDay {
        ReviewActivityDay {
            date: self.date,
            pull_request_count: i32::try_from(self.session_keys.len()).unwrap_or(i32::MAX),
            comment_count: self.comment_count,
            review_count: self.review_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use prs_model::{PullRequestDetails, PullRequestReference, ReviewProvider};

    fn state(session_key: &str) -> ReviewSessionState {
        ReviewSessionState {
            details: PullRequestDetails {
                reference: PullRequestReference {
                    provider: ReviewProvider::GitHub,
                    session_key: session_key.into(),
                    repository: "widget".into(),
                    number: 7,
                    ..Default::default()
                },
                title: "Review widget".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn record(
        id: &str,
        session_key: &str,
        kind: ReviewActivityKind,
        at_utc: chrono::DateTime<Utc>,
        comment_delta: i32,
    ) -> ReviewActivityRecord {
        ReviewActivityRecord {
            id: id.into(),
            session_key: session_key.into(),
            provider: ReviewProvider::GitHub,
            repository: "widget".into(),
            pull_request_number: 7,
            pull_request_title: "Review widget".into(),
            kind,
            at_utc,
            comment_delta,
        }
    }

    #[tokio::test]
    async fn aggregates_one_daily_row_and_deduplicates_pull_requests_and_recent_reviews() {
        let temp = tempfile::tempdir().unwrap();
        let store = ReviewActivityStore::new(temp.path().join("review-activity.json"));
        let service = ReviewActivityService::new(store.clone());
        let first_day = Utc.with_ymd_and_hms(2026, 11, 12, 12, 0, 0).unwrap();
        store
            .record(record(
                "open-1",
                "pr-1",
                ReviewActivityKind::Opened,
                first_day,
                0,
            ))
            .await
            .unwrap();
        store
            .record(record(
                "completed-1",
                "pr-1",
                ReviewActivityKind::Completed,
                first_day,
                0,
            ))
            .await
            .unwrap();
        store
            .record(record(
                "comment-1",
                "pr-1",
                ReviewActivityKind::CommentPublished,
                first_day,
                1,
            ))
            .await
            .unwrap();
        store
            .record(record(
                "reply-1",
                "pr-1",
                ReviewActivityKind::ReplyPublished,
                first_day,
                1,
            ))
            .await
            .unwrap();
        store
            .record(record(
                "review-1",
                "pr-1",
                ReviewActivityKind::MarkedReviewed,
                first_day,
                0,
            ))
            .await
            .unwrap();
        store
            .record(record(
                "outside",
                "pr-2",
                ReviewActivityKind::Completed,
                first_day + chrono::Duration::days(30),
                0,
            ))
            .await
            .unwrap();

        let days = service.get_month(2026, 11).await.unwrap();

        assert_eq!(days.len(), 30);
        let day = days
            .iter()
            .find(|day| day.date == first_day.date_naive())
            .unwrap();
        assert_eq!(day.pull_request_count, 1);
        assert_eq!(day.comment_count, 2);
        assert_eq!(day.review_count, 1);
        assert!(days.iter().all(|other| {
            other.date == day.date
                || (other.pull_request_count == 0
                    && other.comment_count == 0
                    && other.review_count == 0)
        }));

        let recent = service.get_recent_reviews(10).await.unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].session_key, "pr-2");
        assert_eq!(recent[1].session_key, "pr-1");
    }

    #[tokio::test]
    async fn ordinary_save_preserves_last_opened_and_open_updates_it() {
        let temp = tempfile::tempdir().unwrap();
        let state_store = ReviewStateStore::new(temp.path().join("Reviews"));
        let activity_store =
            ReviewActivityStore::new(temp.path().join("Activity/review-activity.json"));
        let service = ReviewSessionService::new(
            state_store.clone(),
            ReviewActivityService::new(activity_store),
        );
        let mut review = state("github:example.test/octo-org/widget#7");
        let old_opened = Utc.with_ymd_and_hms(2025, 1, 2, 3, 4, 5).unwrap();
        review.last_opened_utc = old_opened;

        service.save(&review).await.unwrap();
        let saved = service
            .load(&review.details.reference.session_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.last_opened_utc, old_opened);

        service.open(&mut review).await.unwrap();

        let opened = service
            .load(&review.details.reference.session_key)
            .await
            .unwrap()
            .unwrap();
        assert!(opened.last_opened_utc > old_opened);
        assert_eq!(service.activity().store().load().await.unwrap().len(), 1);
    }
}
