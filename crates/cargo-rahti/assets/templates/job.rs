//! The `job` table: accepted background work.
//!
//! Written by `cargo rahti generate job-store`, and read and written only by
//! `src/job_store.rs`. Times are Unix milliseconds, so every backend compares
//! the same integers. A successful job is deleted; a failed or discarded one
//! stays for inspection until it is purged.

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "job")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub name: String,
    /// JSON. Never logged and never shown by tooling.
    pub payload: String,
    pub origin_request_id: Option<String>,
    /// Held while the job is pending or running, and cleared when it ends.
    #[sea_orm(unique)]
    pub unique_key: Option<String>,
    /// `pending`, `running`, `failed` or `discarded`.
    pub status: String,
    pub attempt: i32,
    pub run_at: i64,
    pub lease_until: Option<i64>,
    pub owner: Option<String>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
