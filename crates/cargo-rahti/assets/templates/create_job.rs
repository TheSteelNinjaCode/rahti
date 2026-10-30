//! Create the `job` table, written by `cargo rahti generate job-store`.

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Job::Table)
                    .if_not_exists()
                    .col(string_len(Job::Id, 64).primary_key())
                    .col(string_len(Job::Name, 64))
                    .col(text(Job::Payload))
                    .col(string_len_null(Job::OriginRequestId, 128))
                    .col(string_len_null(Job::UniqueKey, 128).unique_key())
                    .col(string_len(Job::Status, 16))
                    .col(integer(Job::Attempt).default(0))
                    .col(big_integer(Job::RunAt))
                    .col(big_integer_null(Job::LeaseUntil))
                    .col(string_len_null(Job::Owner, 96))
                    .col(text_null(Job::LastError))
                    .col(big_integer(Job::CreatedAt))
                    .col(big_integer(Job::UpdatedAt))
                    .to_owned(),
            )
            .await?;

        // What every worker asks on every poll.
        manager
            .create_index(
                Index::create()
                    .name("idx_job_claim")
                    .table(Job::Table)
                    .col(Job::Status)
                    .col(Job::RunAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Job::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Job {
    Table,
    Id,
    Name,
    Payload,
    OriginRequestId,
    UniqueKey,
    Status,
    Attempt,
    RunAt,
    LeaseUntil,
    Owner,
    LastError,
    CreatedAt,
    UpdatedAt,
}
