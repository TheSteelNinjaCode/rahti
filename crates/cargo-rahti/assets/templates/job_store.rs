//! Background jobs in this application's own database.
//!
//! Written by `cargo rahti generate job-store`, and yours from then on.
//! Keeping jobs beside the data gives two things a separate queue cannot: a
//! job can commit in the same transaction as the write it follows, through
//! [`enqueue_in`], and every process using this database shares one queue.
//!
//! Claiming is a compare-and-swap: find a due row, then update it only if it
//! is still due. `UPDATE` is atomic on SQLite, Postgres and MySQL alike, so the
//! loser of a race sees no rows affected and looks again. The test that calls
//! `rahti::jobs::conformance` holds this file to the store contract.

use rahti::jobs::{
    Claim, ClaimedJob, FailedJob, Inserted, JobCounts, JobStore, JobStoreError, JobStoreFuture,
    NewJob, Terminal,
};
use sea_orm::{
    ActiveValue::Set,
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, SqlErr,
    sea_query::{Expr, ExprTrait},
};

use crate::db::{db, is_connected};
use crate::models::job::{self, Column, Entity};

const PENDING: &str = "pending";
const RUNNING: &str = "running";
/// A lost race is retried this often before the worker waits for its next poll.
const CLAIM_RETRIES: usize = 5;

fn failed(error: DbErr) -> JobStoreError {
    JobStoreError::new(error)
}

/// The connection, or an error a status report can show. `db()` panics when
/// `DATABASE_URL` was never set, and a panic here would end a worker silently.
fn connection() -> Result<&'static DatabaseConnection, JobStoreError> {
    if is_connected() {
        Ok(db())
    } else {
        Err(JobStoreError::new("the database is not connected"))
    }
}

/// Insert a job on `connection`, which can be the transaction of the write it
/// follows: both commit or neither does. A worker finds it on its next poll.
pub async fn enqueue_in<C: ConnectionTrait>(
    connection: &C,
    job: NewJob,
) -> Result<Inserted, JobStoreError> {
    let payload = job.payload.to_string();
    let row = job::ActiveModel {
        id: Set(job.id),
        name: Set(job.name),
        payload: Set(payload),
        origin_request_id: Set(job.origin_request_id),
        unique_key: Set(job.unique_key),
        status: Set(PENDING.to_string()),
        attempt: Set(0),
        run_at: Set(job.run_at_ms),
        lease_until: Set(None),
        owner: Set(None),
        last_error: Set(None),
        created_at: Set(job.run_at_ms),
        updated_at: Set(job.run_at_ms),
    };
    match Entity::insert(row).exec_without_returning(connection).await {
        Ok(_) => Ok(Inserted::Accepted),
        Err(error) if matches!(error.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
            Ok(Inserted::Duplicate)
        }
        Err(error) => Err(failed(error)),
    }
}

/// Due now: pending and past its time, or running on an expired lease.
fn due(now_ms: i64) -> Condition {
    Condition::any()
        .add(
            Condition::all()
                .add(Column::Status.eq(PENDING))
                .add(Column::RunAt.lte(now_ms)),
        )
        .add(
            Condition::all()
                .add(Column::Status.eq(RUNNING))
                .add(Column::LeaseUntil.lte(now_ms)),
        )
}

fn held(id: &str, owner: &str) -> Condition {
    Condition::all()
        .add(Column::Id.eq(id))
        .add(Column::Status.eq(RUNNING))
        .add(Column::Owner.eq(owner))
}

/// The application's database as a [`JobStore`].
pub struct SeaJobStore;

impl JobStore for SeaJobStore {
    fn insert(&self, job: NewJob) -> JobStoreFuture<'_, Inserted> {
        Box::pin(async move { enqueue_in(connection()?, job).await })
    }

    fn claim<'a>(&'a self, claim: Claim<'a>) -> JobStoreFuture<'a, Option<ClaimedJob>> {
        Box::pin(async move {
            for _ in 0..CLAIM_RETRIES {
                let Some(row) = Entity::find()
                    .filter(due(claim.now_ms))
                    .filter(Column::Name.is_in(claim.names.iter().map(String::as_str)))
                    .order_by_asc(Column::RunAt)
                    .limit(1)
                    .one(connection()?)
                    .await
                    .map_err(failed)?
                else {
                    return Ok(None);
                };

                let attempt = row.attempt + 1;
                let won = Entity::update_many()
                    .col_expr(Column::Status, Expr::value(RUNNING))
                    .col_expr(Column::Attempt, Expr::value(attempt))
                    .col_expr(Column::Owner, Expr::value(claim.owner))
                    .col_expr(Column::LeaseUntil, Expr::value(claim.lease_until_ms))
                    .col_expr(Column::UpdatedAt, Expr::value(claim.now_ms))
                    .filter(Column::Id.eq(row.id.as_str()))
                    .filter(Column::Attempt.eq(row.attempt))
                    .filter(due(claim.now_ms))
                    .exec(connection()?)
                    .await
                    .map_err(failed)?
                    .rows_affected
                    == 1;
                if won {
                    let payload =
                        rahti::serde_json::from_str(&row.payload).map_err(JobStoreError::new)?;
                    return Ok(Some(ClaimedJob {
                        id: row.id,
                        name: row.name,
                        payload,
                        origin_request_id: row.origin_request_id,
                        attempt: u32::try_from(attempt).unwrap_or(u32::MAX),
                    }));
                }
            }
            Ok(None)
        })
    }

    fn renew<'a>(
        &'a self,
        id: &'a str,
        owner: &'a str,
        lease_until_ms: i64,
    ) -> JobStoreFuture<'a, bool> {
        Box::pin(async move {
            let renewed = Entity::update_many()
                .col_expr(Column::LeaseUntil, Expr::value(lease_until_ms))
                .filter(held(id, owner))
                .exec(connection()?)
                .await
                .map_err(failed)?;
            Ok(renewed.rows_affected == 1)
        })
    }

    fn complete<'a>(&'a self, id: &'a str, owner: &'a str) -> JobStoreFuture<'a, ()> {
        Box::pin(async move {
            Entity::delete_many()
                .filter(held(id, owner))
                .exec(connection()?)
                .await
                .map_err(failed)?;
            Ok(())
        })
    }

    fn reschedule<'a>(
        &'a self,
        id: &'a str,
        owner: &'a str,
        run_at_ms: i64,
        error: &'a str,
    ) -> JobStoreFuture<'a, ()> {
        Box::pin(async move {
            Entity::update_many()
                .col_expr(Column::Status, Expr::value(PENDING))
                .col_expr(Column::RunAt, Expr::value(run_at_ms))
                .col_expr(Column::LastError, Expr::value(error))
                .col_expr(Column::Owner, Expr::value(Option::<String>::None))
                .col_expr(Column::LeaseUntil, Expr::value(Option::<i64>::None))
                .filter(held(id, owner))
                .exec(connection()?)
                .await
                .map_err(failed)?;
            Ok(())
        })
    }

    fn finish<'a>(
        &'a self,
        id: &'a str,
        owner: &'a str,
        outcome: Terminal,
        error: &'a str,
        now_ms: i64,
    ) -> JobStoreFuture<'a, ()> {
        Box::pin(async move {
            Entity::update_many()
                .col_expr(Column::Status, Expr::value(outcome.as_str()))
                .col_expr(Column::LastError, Expr::value(error))
                .col_expr(Column::UniqueKey, Expr::value(Option::<String>::None))
                .col_expr(Column::Owner, Expr::value(Option::<String>::None))
                .col_expr(Column::LeaseUntil, Expr::value(Option::<i64>::None))
                .col_expr(Column::UpdatedAt, Expr::value(now_ms))
                .filter(held(id, owner))
                .exec(connection()?)
                .await
                .map_err(failed)?;
            Ok(())
        })
    }

    fn release<'a>(&'a self, id: &'a str, owner: &'a str) -> JobStoreFuture<'a, ()> {
        Box::pin(async move {
            Entity::update_many()
                .col_expr(Column::Status, Expr::value(PENDING))
                .col_expr(Column::Attempt, Expr::col(Column::Attempt).sub(1))
                .col_expr(Column::Owner, Expr::value(Option::<String>::None))
                .col_expr(Column::LeaseUntil, Expr::value(Option::<i64>::None))
                .filter(held(id, owner))
                .exec(connection()?)
                .await
                .map_err(failed)?;
            Ok(())
        })
    }

    fn counts(&self) -> JobStoreFuture<'_, JobCounts> {
        Box::pin(async move {
            let count = async |status: &str| {
                Entity::find()
                    .filter(Column::Status.eq(status))
                    .count(connection()?)
                    .await
                    .map_err(failed)
            };
            Ok(JobCounts {
                pending: count(PENDING).await?,
                running: count(RUNNING).await?,
                failed: count(Terminal::Failed.as_str()).await?,
                discarded: count(Terminal::Discarded.as_str()).await?,
            })
        })
    }

    fn failures(&self, limit: u32) -> JobStoreFuture<'_, Vec<FailedJob>> {
        Box::pin(async move {
            let rows = Entity::find()
                .filter(
                    Column::Status.is_in([Terminal::Failed.as_str(), Terminal::Discarded.as_str()]),
                )
                .order_by_desc(Column::UpdatedAt)
                .limit(u64::from(limit))
                .all(connection()?)
                .await
                .map_err(failed)?;
            Ok(rows
                .into_iter()
                .map(|row| FailedJob {
                    outcome: if row.status == Terminal::Failed.as_str() {
                        Terminal::Failed
                    } else {
                        Terminal::Discarded
                    },
                    id: row.id,
                    name: row.name,
                    attempt: u32::try_from(row.attempt).unwrap_or(0),
                    error: row.last_error.unwrap_or_default(),
                    at_ms: row.updated_at,
                })
                .collect())
        })
    }

    fn purge(&self, before_ms: i64) -> JobStoreFuture<'_, u64> {
        Box::pin(async move {
            let deleted = Entity::delete_many()
                .filter(
                    Column::Status.is_in([Terminal::Failed.as_str(), Terminal::Discarded.as_str()]),
                )
                .filter(Column::UpdatedAt.lt(before_ms))
                .exec(connection()?)
                .await
                .map_err(failed)?;
            Ok(deleted.rows_affected)
        })
    }
}
