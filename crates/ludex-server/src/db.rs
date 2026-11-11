//! Database connection, migrations and small shared helpers.
//!
//! Ludex supports two backends (#46):
//!
//! * SQLite — the default, a single file, no external service;
//! * PostgreSQL — for operators who already run a database server.
//!
//! The `Database` type selects the backend at startup and carries the matching
//! pool. Queries go through the small [`DbQuery`] wrappers below so a single
//! SQL text can run on both engines: `?` placeholders are written once and
//! rewritten to `$1`, `$2`, … for PostgreSQL at execution time. The few places
//! where the dialects really differ (JSON table functions, scalar `MAX`) branch
//! on [`Database::is_postgres`] instead of concatenating user input.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgRow};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
    SqliteSynchronous,
};
use sqlx::types::Type;
use sqlx::{ColumnIndex, Decode, FromRow, Postgres, Row, Sqlite};

/// Default pool size when `LUDEX_DATABASE_MAX_CONNECTIONS` is not set.
pub const DEFAULT_MAX_CONNECTIONS: u32 = 8;

/// Which SQL engine a [`Database`] talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    /// Embedded SQLite file (default).
    Sqlite,
    /// PostgreSQL server.
    Postgres,
}

impl Backend {
    /// Stable lowercase identifier used in logs, health details and docs.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Sqlite => "sqlite",
            Backend::Postgres => "postgres",
        }
    }

    /// Parses the value of `LUDEX_DATABASE_BACKEND`.
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sqlite" | "sqlite3" => Ok(Backend::Sqlite),
            "postgres" | "postgresql" | "pg" => Ok(Backend::Postgres),
            other => {
                bail!("unknown database backend {other:?}: expected \"sqlite\" or \"postgres\"")
            }
        }
    }

    /// Infers the backend from a connection URL.
    pub fn from_url(database_url: &str) -> Result<Self> {
        let trimmed = database_url.trim();
        if trimmed.is_empty() {
            bail!("database URL is empty");
        }
        let scheme = trimmed
            .split_once(':')
            .map(|(scheme, _)| scheme.to_ascii_lowercase());
        match scheme.as_deref() {
            Some("postgres" | "postgresql") => Ok(Backend::Postgres),
            Some("sqlite" | "sqlite3") => Ok(Backend::Sqlite),
            Some(other) => {
                bail!("unsupported database URL scheme {other:?}: use sqlite:// or postgres://")
            }
            // A bare path is only valid when LUDEX_DATABASE_BACKEND says sqlite.
            None => Ok(Backend::Sqlite),
        }
    }
}

/// Pool counters exposed by `/api/health/details`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PoolStats {
    /// Live connections (including idle ones).
    pub size: u32,
    /// Idle connections ready for reuse.
    pub idle: u32,
    /// Configured ceiling.
    pub max_connections: u32,
}

#[derive(Clone)]
enum DbInner {
    Sqlite(SqlitePool),
    Postgres(PgPool),
}

/// A live database handle: one pool for the configured backend.
#[derive(Clone)]
pub struct Database {
    inner: DbInner,
    max_connections: u32,
}

impl fmt::Debug for Database {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Database")
            .field("backend", &self.backend().as_str())
            .field("max_connections", &self.max_connections)
            .finish()
    }
}

impl Database {
    /// Which engine this handle talks to.
    #[must_use]
    pub fn backend(&self) -> Backend {
        match self.inner {
            DbInner::Sqlite(_) => Backend::Sqlite,
            DbInner::Postgres(_) => Backend::Postgres,
        }
    }

    /// Convenience predicate for the handful of dialect-specific queries.
    #[must_use]
    pub fn is_postgres(&self) -> bool {
        self.backend() == Backend::Postgres
    }

    /// Pool counters for health details and metrics.
    #[must_use]
    pub fn pool_stats(&self) -> PoolStats {
        match &self.inner {
            DbInner::Sqlite(pool) => PoolStats {
                size: pool.size(),
                idle: u32::try_from(pool.num_idle()).unwrap_or(0),
                max_connections: self.max_connections,
            },
            DbInner::Postgres(pool) => PoolStats {
                size: pool.size(),
                idle: u32::try_from(pool.num_idle()).unwrap_or(0),
                max_connections: self.max_connections,
            },
        }
    }

    /// Runs `SELECT 1`; used by health and metrics.
    pub async fn ping(&self) -> bool {
        self.query_scalar::<i64>("SELECT CAST(1 AS BIGINT)")
            .fetch_one(self)
            .await
            .is_ok()
    }

    /// Starts a transaction on the active backend.
    pub async fn begin(&self) -> Result<DbTransaction<'_>, sqlx::Error> {
        Ok(match &self.inner {
            DbInner::Sqlite(pool) => DbTransaction::Sqlite(pool.begin().await?),
            DbInner::Postgres(pool) => DbTransaction::Postgres(pool.begin().await?),
        })
    }

    /// Closes every pooled connection.
    pub async fn close(self) {
        match self.inner {
            DbInner::Sqlite(pool) => pool.close().await,
            DbInner::Postgres(pool) => pool.close().await,
        }
    }

    /// Bytes used by the database, when the backend can report it cheaply.
    ///
    /// SQLite returns the size of the file plus its WAL sidecar; PostgreSQL
    /// asks the server (`pg_database_size`) and returns 0 when the query is not
    /// permitted.
    pub async fn size_bytes(&self) -> u64 {
        match &self.inner {
            DbInner::Sqlite(_) => {
                // The URL is not stored here; callers use `database_size`.
                0
            }
            DbInner::Postgres(_) => self
                .query_scalar::<i64>("SELECT pg_database_size(current_database())")
                .fetch_one(self)
                .await
                .ok()
                .and_then(|value| u64::try_from(value).ok())
                .unwrap_or(0),
        }
    }
}

/// Creates a query builder.
#[must_use]
pub fn query(sql: impl AsRef<str>) -> DbQuery {
    DbQuery::new(sql.as_ref())
}

/// Creates a typed query builder.
#[must_use]
pub fn query_as<O>(sql: impl AsRef<str>) -> DbQueryAs<O> {
    DbQueryAs::new(sql.as_ref())
}

/// Creates a scalar query builder.
#[must_use]
pub fn query_scalar<O>(sql: impl AsRef<str>) -> DbQueryScalar<O> {
    DbQueryScalar::new(sql.as_ref())
}

impl Database {
    /// Creates a query builder.
    #[must_use]
    pub fn query(&self, sql: impl AsRef<str>) -> DbQuery {
        DbQuery::new(sql.as_ref())
    }

    /// Creates a typed query builder.
    #[must_use]
    pub fn query_as<O>(&self, sql: impl AsRef<str>) -> DbQueryAs<O> {
        DbQueryAs::new(sql.as_ref())
    }

    /// Creates a scalar query builder.
    #[must_use]
    pub fn query_scalar<O>(&self, sql: impl AsRef<str>) -> DbQueryScalar<O> {
        DbQueryScalar::new(sql.as_ref())
    }
}

/// Result of a statement that does not return rows.
#[derive(Debug, Clone, Copy, Default)]
pub struct DbQueryResult {
    rows_affected: u64,
}

impl DbQueryResult {
    /// Number of rows inserted, updated or deleted.
    #[must_use]
    pub fn rows_affected(&self) -> u64 {
        self.rows_affected
    }
}

/// Marker for row structs that can be decoded from either backend.
pub trait RowType:
    for<'r> FromRow<'r, SqliteRow> + for<'r> FromRow<'r, PgRow> + Send + Unpin
{
}

impl<T> RowType for T where
    T: for<'r> FromRow<'r, SqliteRow> + for<'r> FromRow<'r, PgRow> + Send + Unpin
{
}

/// Marker for scalar types decodable from either backend.
pub trait ScalarType:
    for<'r> Decode<'r, Sqlite>
    + Type<Sqlite>
    + for<'r> Decode<'r, Postgres>
    + Type<Postgres>
    + Send
    + Unpin
{
}

impl<T> ScalarType for T where
    T: for<'r> Decode<'r, Sqlite>
        + Type<Sqlite>
        + for<'r> Decode<'r, Postgres>
        + Type<Postgres>
        + Send
        + Unpin
{
}

/// One dynamically bound SQL value.
#[derive(Debug, Clone)]
pub enum DbValue {
    /// Non-null integer.
    Int(i64),
    /// Non-null float.
    Float(f64),
    /// Non-null text.
    Text(String),
    /// Non-null blob.
    Blob(Vec<u8>),
    /// Nullable integer.
    OptInt(Option<i64>),
    /// Nullable float.
    OptFloat(Option<f64>),
    /// Nullable text.
    OptText(Option<String>),
    /// Nullable blob.
    OptBlob(Option<Vec<u8>>),
}

impl From<i64> for DbValue {
    fn from(value: i64) -> Self {
        DbValue::Int(value)
    }
}

impl From<i32> for DbValue {
    fn from(value: i32) -> Self {
        DbValue::Int(i64::from(value))
    }
}

impl From<u32> for DbValue {
    fn from(value: u32) -> Self {
        DbValue::Int(i64::from(value))
    }
}

impl From<f64> for DbValue {
    fn from(value: f64) -> Self {
        DbValue::Float(value)
    }
}

impl From<String> for DbValue {
    fn from(value: String) -> Self {
        DbValue::Text(value)
    }
}

impl From<&str> for DbValue {
    fn from(value: &str) -> Self {
        DbValue::Text(value.to_string())
    }
}

impl From<&String> for DbValue {
    fn from(value: &String) -> Self {
        DbValue::Text(value.clone())
    }
}

impl From<Vec<u8>> for DbValue {
    fn from(value: Vec<u8>) -> Self {
        DbValue::Blob(value)
    }
}

impl From<&[u8]> for DbValue {
    fn from(value: &[u8]) -> Self {
        DbValue::Blob(value.to_vec())
    }
}

impl From<Option<i64>> for DbValue {
    fn from(value: Option<i64>) -> Self {
        DbValue::OptInt(value)
    }
}

impl From<Option<i32>> for DbValue {
    fn from(value: Option<i32>) -> Self {
        DbValue::OptInt(value.map(i64::from))
    }
}

impl From<Option<f64>> for DbValue {
    fn from(value: Option<f64>) -> Self {
        DbValue::OptFloat(value)
    }
}

impl From<Option<String>> for DbValue {
    fn from(value: Option<String>) -> Self {
        DbValue::OptText(value)
    }
}

impl From<Option<&str>> for DbValue {
    fn from(value: Option<&str>) -> Self {
        DbValue::OptText(value.map(str::to_string))
    }
}

impl From<Option<Vec<u8>>> for DbValue {
    fn from(value: Option<Vec<u8>>) -> Self {
        DbValue::OptBlob(value)
    }
}

impl From<&i64> for DbValue {
    fn from(value: &i64) -> Self {
        DbValue::Int(*value)
    }
}

impl From<&f64> for DbValue {
    fn from(value: &f64) -> Self {
        DbValue::Float(*value)
    }
}

impl From<&Option<i64>> for DbValue {
    fn from(value: &Option<i64>) -> Self {
        DbValue::OptInt(*value)
    }
}

impl From<&Option<f64>> for DbValue {
    fn from(value: &Option<f64>) -> Self {
        DbValue::OptFloat(*value)
    }
}

impl From<&Option<String>> for DbValue {
    fn from(value: &Option<String>) -> Self {
        DbValue::OptText(value.clone())
    }
}

impl From<&Option<Vec<u8>>> for DbValue {
    fn from(value: &Option<Vec<u8>>) -> Self {
        DbValue::OptBlob(value.clone())
    }
}

/// Executes `query` with the dynamic bindings in `binds`.
macro_rules! bind_all {
    ($query:expr, $binds:expr) => {{
        let mut query = $query;
        for bind in $binds {
            query = match bind {
                DbValue::Int(value) => query.bind(*value),
                DbValue::Float(value) => query.bind(*value),
                DbValue::Text(value) => query.bind(value.as_str()),
                DbValue::Blob(value) => query.bind(value.as_slice()),
                DbValue::OptInt(value) => query.bind(*value),
                DbValue::OptFloat(value) => query.bind(*value),
                DbValue::OptText(value) => query.bind(value.as_deref()),
                DbValue::OptBlob(value) => query.bind(value.as_deref()),
            };
        }
        query
    }};
}

/// Where a query runs: the pool or an open transaction.
pub enum DbExec<'a, 'c> {
    /// The connection pool.
    Pool(&'a Database),
    /// An open transaction.
    Transaction(&'a mut DbTransaction<'c>),
}

impl<'a, 'c> From<&'a Database> for DbExec<'a, 'c> {
    fn from(database: &'a Database) -> Self {
        DbExec::Pool(database)
    }
}

impl<'a, 'c: 'a> From<&'a mut DbTransaction<'c>> for DbExec<'a, 'c> {
    fn from(transaction: &'a mut DbTransaction<'c>) -> Self {
        DbExec::Transaction(transaction)
    }
}

fn placeholder_sql(backend: Backend, sql: &str) -> String {
    match backend {
        Backend::Sqlite => sql.to_string(),
        Backend::Postgres => pg_placeholders(sql),
    }
}

/// Rewrites `?` placeholders to `$1`, `$2`, … for PostgreSQL.
///
/// String literals, quoted identifiers, line comments and block comments are
/// left untouched, so a `?` in data is never treated as a parameter.
#[must_use]
pub fn pg_placeholders(sql: &str) -> String {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        Single,
        Double,
        LineComment,
        BlockComment,
    }

    let mut out = String::with_capacity(sql.len() + 8);
    let mut state = State::Normal;
    let mut count = 0_usize;
    let mut chars = sql.chars().peekable();
    while let Some(character) = chars.next() {
        match state {
            State::Normal => match character {
                '\'' => {
                    out.push(character);
                    state = State::Single;
                }
                '"' => {
                    out.push(character);
                    state = State::Double;
                }
                '-' if chars.peek() == Some(&'-') => {
                    out.push(character);
                    out.push(chars.next().unwrap_or('-'));
                    state = State::LineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    out.push(character);
                    out.push(chars.next().unwrap_or('*'));
                    state = State::BlockComment;
                }
                '?' => {
                    count += 1;
                    out.push('$');
                    out.push_str(&count.to_string());
                }
                _ => out.push(character),
            },
            State::Single => {
                out.push(character);
                if character == '\'' {
                    if chars.peek() == Some(&'\'') {
                        out.push(chars.next().unwrap_or('\''));
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::Double => {
                out.push(character);
                if character == '"' {
                    if chars.peek() == Some(&'"') {
                        out.push(chars.next().unwrap_or('"'));
                    } else {
                        state = State::Normal;
                    }
                }
            }
            State::LineComment => {
                out.push(character);
                if character == '\n' {
                    state = State::Normal;
                }
            }
            State::BlockComment => {
                out.push(character);
                if character == '*' && chars.peek() == Some(&'/') {
                    out.push(chars.next().unwrap_or('/'));
                    state = State::Normal;
                }
            }
        }
    }
    out
}

/// Builds a query wrapper.
macro_rules! db_query_type {
    ($name:ident, $output:ident) => {
        /// Query builder bound to one SQL text and its dynamic values.
        pub struct $name<$output = ()> {
            sql: String,
            binds: Vec<DbValue>,
            #[allow(dead_code)]
            marker: std::marker::PhantomData<fn() -> $output>,
        }

        impl<$output> $name<$output> {
            fn new(sql: &str) -> Self {
                Self {
                    sql: sql.to_string(),
                    binds: Vec::new(),
                    marker: std::marker::PhantomData,
                }
            }

            /// Binds one value.
            #[must_use]
            pub fn bind(mut self, value: impl Into<DbValue>) -> Self {
                self.binds.push(value.into());
                self
            }

            /// Binds an already-built dynamic value.
            #[must_use]
            pub fn dyn_bind(mut self, value: DbValue) -> Self {
                self.binds.push(value);
                self
            }

            fn sql_for(&self, backend: Backend) -> String {
                placeholder_sql(backend, &self.sql)
            }

            fn binds(&self) -> &[DbValue] {
                &self.binds
            }
        }
    };
}

db_query_type!(DbQuery, O);
db_query_type!(DbQueryAs, O);
db_query_type!(DbQueryScalar, O);

impl DbQuery {
    /// Executes an `INSERT` and returns the generated `id`.
    ///
    /// Works on both engines: SQLite (3.35+) and PostgreSQL both support
    /// `RETURNING`, which is portable where `last_insert_rowid()` is not.
    pub async fn returning_id<'a, 'c, X>(self, executor: X) -> Result<i64, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
    {
        self.returning_id_optional(executor)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    /// Like [`DbQuery::returning_id`] but tolerates `ON CONFLICT DO NOTHING`.
    pub async fn returning_id_optional<'a, 'c, X>(
        self,
        executor: X,
    ) -> Result<Option<i64>, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
    {
        let sql = format!("{} RETURNING id", self.sql.trim_end().trim_end_matches(';'));
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let query = bind_all!(sqlx::query_scalar::<Sqlite, i64>(&sql), self.binds());
                    query.fetch_optional(pool).await
                }
                DbInner::Postgres(pool) => {
                    let sql = pg_placeholders(&sql);
                    let query = bind_all!(sqlx::query_scalar::<Postgres, i64>(&sql), self.binds());
                    query.fetch_optional(pool).await
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let query = bind_all!(sqlx::query_scalar::<Sqlite, i64>(&sql), self.binds());
                    query.fetch_optional(&mut **inner).await
                }
                DbTransaction::Postgres(inner) => {
                    let sql = pg_placeholders(&sql);
                    let query = bind_all!(sqlx::query_scalar::<Postgres, i64>(&sql), self.binds());
                    query.fetch_optional(&mut **inner).await
                }
            },
        }
    }

    /// Executes the statement and returns the affected-row count.
    pub async fn execute<'a, 'c, X>(self, executor: X) -> Result<DbQueryResult, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query::<Sqlite>(&sql), self.binds());
                    query.execute(pool).await.map(|result| DbQueryResult {
                        rows_affected: result.rows_affected(),
                    })
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query::<Postgres>(&sql), self.binds());
                    query.execute(pool).await.map(|result| DbQueryResult {
                        rows_affected: result.rows_affected(),
                    })
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query::<Sqlite>(&sql), self.binds());
                    query
                        .execute(&mut **inner)
                        .await
                        .map(|result| DbQueryResult {
                            rows_affected: result.rows_affected(),
                        })
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query::<Postgres>(&sql), self.binds());
                    query
                        .execute(&mut **inner)
                        .await
                        .map(|result| DbQueryResult {
                            rows_affected: result.rows_affected(),
                        })
                }
            },
        }
    }

    /// Fetches every row as a backend-specific row.
    pub async fn fetch_all<'a, 'c, X>(self, executor: X) -> Result<Vec<DbRow>, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query::<Sqlite>(&sql), self.binds());
                    let rows = query.fetch_all(pool).await?;
                    Ok(rows.into_iter().map(DbRow::Sqlite).collect())
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query::<Postgres>(&sql), self.binds());
                    let rows = query.fetch_all(pool).await?;
                    Ok(rows.into_iter().map(DbRow::Postgres).collect())
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query::<Sqlite>(&sql), self.binds());
                    let rows = query.fetch_all(&mut **inner).await?;
                    Ok(rows.into_iter().map(DbRow::Sqlite).collect())
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query::<Postgres>(&sql), self.binds());
                    let rows = query.fetch_all(&mut **inner).await?;
                    Ok(rows.into_iter().map(DbRow::Postgres).collect())
                }
            },
        }
    }

    /// Fetches exactly one row.
    pub async fn fetch_one<'a, 'c, X>(self, executor: X) -> Result<DbRow, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
    {
        self.fetch_optional(executor)
            .await?
            .ok_or(sqlx::Error::RowNotFound)
    }

    /// Fetches zero or one row.
    pub async fn fetch_optional<'a, 'c, X>(self, executor: X) -> Result<Option<DbRow>, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query::<Sqlite>(&sql), self.binds());
                    Ok(query.fetch_optional(pool).await?.map(DbRow::Sqlite))
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query::<Postgres>(&sql), self.binds());
                    Ok(query.fetch_optional(pool).await?.map(DbRow::Postgres))
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query::<Sqlite>(&sql), self.binds());
                    Ok(query.fetch_optional(&mut **inner).await?.map(DbRow::Sqlite))
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query::<Postgres>(&sql), self.binds());
                    Ok(query
                        .fetch_optional(&mut **inner)
                        .await?
                        .map(DbRow::Postgres))
                }
            },
        }
    }
}

/// One row returned by a plain [`DbQuery`].
pub enum DbRow {
    /// SQLite row.
    Sqlite(SqliteRow),
    /// PostgreSQL row.
    Postgres(PgRow),
}

impl DbRow {
    /// Reads a column by name or position, panicking on a decode error.
    pub fn get<'r, T, I>(&'r self, index: I) -> T
    where
        T: RowValue<'r>,
        I: ColumnIndex<SqliteRow> + ColumnIndex<PgRow> + Copy + std::fmt::Debug,
    {
        self.try_get(index)
            .unwrap_or_else(|error| panic!("could not read column {index:?}: {error}"))
    }

    /// Reads a column by name or position.
    pub fn try_get<'r, T, I>(&'r self, index: I) -> Result<T, sqlx::Error>
    where
        T: RowValue<'r>,
        I: ColumnIndex<SqliteRow> + ColumnIndex<PgRow> + Copy,
    {
        match self {
            DbRow::Sqlite(row) => row.try_get(index),
            DbRow::Postgres(row) => row.try_get(index),
        }
    }
}

/// Column types readable from either backend.
pub trait RowValue<'r>:
    Decode<'r, Sqlite> + Type<Sqlite> + Decode<'r, Postgres> + Type<Postgres>
{
}

impl<'r, T> RowValue<'r> for T where
    T: Decode<'r, Sqlite> + Type<Sqlite> + Decode<'r, Postgres> + Type<Postgres>
{
}

impl<O: RowType> DbQueryAs<O> {
    /// Executes the statement and returns the affected-row count.
    pub async fn execute<'a, 'c, X>(self, executor: X) -> Result<DbQueryResult, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query::<Sqlite>(&sql), self.binds());
                    query.execute(pool).await.map(|result| DbQueryResult {
                        rows_affected: result.rows_affected(),
                    })
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query::<Postgres>(&sql), self.binds());
                    query.execute(pool).await.map(|result| DbQueryResult {
                        rows_affected: result.rows_affected(),
                    })
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query::<Sqlite>(&sql), self.binds());
                    query
                        .execute(&mut **inner)
                        .await
                        .map(|result| DbQueryResult {
                            rows_affected: result.rows_affected(),
                        })
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query::<Postgres>(&sql), self.binds());
                    query
                        .execute(&mut **inner)
                        .await
                        .map(|result| DbQueryResult {
                            rows_affected: result.rows_affected(),
                        })
                }
            },
        }
    }

    /// Fetches every row.
    pub async fn fetch_all<'a, 'c, X>(self, executor: X) -> Result<Vec<O>, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
        O: RowType,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_as::<Sqlite, O>(&sql), self.binds());
                    query.fetch_all(pool).await
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_as::<Postgres, O>(&sql), self.binds());
                    query.fetch_all(pool).await
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_as::<Sqlite, O>(&sql), self.binds());
                    query.fetch_all(&mut **inner).await
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_as::<Postgres, O>(&sql), self.binds());
                    query.fetch_all(&mut **inner).await
                }
            },
        }
    }

    /// Fetches exactly one row.
    pub async fn fetch_one<'a, 'c, X>(self, executor: X) -> Result<O, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
        O: RowType,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_as::<Sqlite, O>(&sql), self.binds());
                    query.fetch_one(pool).await
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_as::<Postgres, O>(&sql), self.binds());
                    query.fetch_one(pool).await
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_as::<Sqlite, O>(&sql), self.binds());
                    query.fetch_one(&mut **inner).await
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_as::<Postgres, O>(&sql), self.binds());
                    query.fetch_one(&mut **inner).await
                }
            },
        }
    }

    /// Fetches zero or one row.
    pub async fn fetch_optional<'a, 'c, X>(self, executor: X) -> Result<Option<O>, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
        O: RowType,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_as::<Sqlite, O>(&sql), self.binds());
                    query.fetch_optional(pool).await
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_as::<Postgres, O>(&sql), self.binds());
                    query.fetch_optional(pool).await
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_as::<Sqlite, O>(&sql), self.binds());
                    query.fetch_optional(&mut **inner).await
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_as::<Postgres, O>(&sql), self.binds());
                    query.fetch_optional(&mut **inner).await
                }
            },
        }
    }
}

impl<O: ScalarType> DbQueryScalar<O> {
    /// Executes the statement and returns the affected-row count.
    pub async fn execute<'a, 'c, X>(self, executor: X) -> Result<DbQueryResult, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query::<Sqlite>(&sql), self.binds());
                    query.execute(pool).await.map(|result| DbQueryResult {
                        rows_affected: result.rows_affected(),
                    })
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query::<Postgres>(&sql), self.binds());
                    query.execute(pool).await.map(|result| DbQueryResult {
                        rows_affected: result.rows_affected(),
                    })
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query::<Sqlite>(&sql), self.binds());
                    query
                        .execute(&mut **inner)
                        .await
                        .map(|result| DbQueryResult {
                            rows_affected: result.rows_affected(),
                        })
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query::<Postgres>(&sql), self.binds());
                    query
                        .execute(&mut **inner)
                        .await
                        .map(|result| DbQueryResult {
                            rows_affected: result.rows_affected(),
                        })
                }
            },
        }
    }

    /// Fetches every row.
    pub async fn fetch_all<'a, 'c, X>(self, executor: X) -> Result<Vec<O>, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
        O: ScalarType,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_scalar::<Sqlite, O>(&sql), self.binds());
                    query.fetch_all(pool).await
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_scalar::<Postgres, O>(&sql), self.binds());
                    query.fetch_all(pool).await
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_scalar::<Sqlite, O>(&sql), self.binds());
                    query.fetch_all(&mut **inner).await
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_scalar::<Postgres, O>(&sql), self.binds());
                    query.fetch_all(&mut **inner).await
                }
            },
        }
    }

    /// Fetches exactly one row.
    pub async fn fetch_one<'a, 'c, X>(self, executor: X) -> Result<O, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
        O: ScalarType,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_scalar::<Sqlite, O>(&sql), self.binds());
                    query.fetch_one(pool).await
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_scalar::<Postgres, O>(&sql), self.binds());
                    query.fetch_one(pool).await
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_scalar::<Sqlite, O>(&sql), self.binds());
                    query.fetch_one(&mut **inner).await
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_scalar::<Postgres, O>(&sql), self.binds());
                    query.fetch_one(&mut **inner).await
                }
            },
        }
    }

    /// Fetches zero or one row.
    pub async fn fetch_optional<'a, 'c, X>(self, executor: X) -> Result<Option<O>, sqlx::Error>
    where
        'c: 'a,
        X: Into<DbExec<'a, 'c>>,
        O: ScalarType,
    {
        match executor.into() {
            DbExec::Pool(database) => match &database.inner {
                DbInner::Sqlite(pool) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_scalar::<Sqlite, O>(&sql), self.binds());
                    query.fetch_optional(pool).await
                }
                DbInner::Postgres(pool) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_scalar::<Postgres, O>(&sql), self.binds());
                    query.fetch_optional(pool).await
                }
            },
            DbExec::Transaction(transaction) => match transaction {
                DbTransaction::Sqlite(inner) => {
                    let sql = self.sql_for(Backend::Sqlite);
                    let query = bind_all!(sqlx::query_scalar::<Sqlite, O>(&sql), self.binds());
                    query.fetch_optional(&mut **inner).await
                }
                DbTransaction::Postgres(inner) => {
                    let sql = self.sql_for(Backend::Postgres);
                    let query = bind_all!(sqlx::query_scalar::<Postgres, O>(&sql), self.binds());
                    query.fetch_optional(&mut **inner).await
                }
            },
        }
    }
}

/// An open transaction on the active backend.
pub enum DbTransaction<'c> {
    /// SQLite transaction.
    Sqlite(sqlx::Transaction<'c, Sqlite>),
    /// PostgreSQL transaction.
    Postgres(sqlx::Transaction<'c, Postgres>),
}

impl DbTransaction<'_> {
    /// Commits the transaction.
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        match self {
            DbTransaction::Sqlite(transaction) => transaction.commit().await,
            DbTransaction::Postgres(transaction) => transaction.commit().await,
        }
    }

    /// Rolls the transaction back.
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        match self {
            DbTransaction::Sqlite(transaction) => transaction.rollback().await,
            DbTransaction::Postgres(transaction) => transaction.rollback().await,
        }
    }
}

/// Opens the configured database and applies migrations.
pub async fn connect(database_url: &str) -> Result<Database> {
    connect_with(database_url, DEFAULT_MAX_CONNECTIONS).await
}

/// Opens a smaller pool for tests. The suite starts hundreds of databases in
/// parallel; eight connections each exhausts file descriptors and makes
/// unrelated tests time out on slower machines.
#[cfg(test)]
pub async fn connect_for_tests(database_url: &str) -> Result<Database> {
    connect_with(database_url, 2).await
}

/// Opens the configured database with an explicit pool size.
pub async fn connect_with(database_url: &str, max_connections: u32) -> Result<Database> {
    let max_connections = max_connections.max(1);
    let backend = Backend::from_url(database_url)?;
    match backend {
        Backend::Sqlite => {
            let options = connect_options(database_url).with_context(|| {
                format!(
                    "invalid database URL: {}",
                    redact_database_url(database_url)
                )
            })?;
            let pool = SqlitePoolOptions::new()
                .max_connections(max_connections)
                .acquire_timeout(Duration::from_secs(30))
                .connect_with(options)
                .await
                .context("connect to SQLite database")?;

            sqlx::migrate!("./migrations")
                .run(&pool)
                .await
                .context("apply database migrations")?;

            Ok(Database {
                inner: DbInner::Sqlite(pool),
                max_connections,
            })
        }
        Backend::Postgres => {
            let options = PgConnectOptions::from_str(database_url).with_context(|| {
                format!(
                    "invalid PostgreSQL URL: {}",
                    redact_database_url(database_url)
                )
            })?;
            let pool = PgPoolOptions::new()
                .max_connections(max_connections)
                .acquire_timeout(Duration::from_secs(15))
                .connect_with(options)
                .await
                .context(
                    "connect to PostgreSQL database (check LUDEX_DATABASE_URL, the server is running and the role can create tables)",
                )?;

            sqlx::migrate!("./migrations_postgres")
                .run(&pool)
                .await
                .context("apply PostgreSQL migrations")?;

            Ok(Database {
                inner: DbInner::Postgres(pool),
                max_connections,
            })
        }
    }
}

fn connect_options(database_url: &str) -> Result<SqliteConnectOptions> {
    let options = match sqlite_path_from_url(database_url) {
        Some(path) => SqliteConnectOptions::new().filename(path),
        None => SqliteConnectOptions::from_str(database_url)?,
    };
    Ok(options
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5)))
}

/// Extracts the filesystem path from a `sqlite://` URL, ignoring query options.
#[must_use]
pub fn sqlite_path_from_url(database_url: &str) -> Option<PathBuf> {
    let rest = database_url.strip_prefix("sqlite://")?;
    let path = rest.split_once('?').map_or(rest, |(path, _)| path);
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(percent_decode(path)))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Hides the password in a database URL before logging or returning it.
#[must_use]
pub fn redact_database_url(database_url: &str) -> String {
    if let Ok(mut url) = url::Url::parse(database_url.trim()) {
        if url.password().is_some() {
            let _ = url.set_password(Some("***"));
            return url.to_string();
        }
        return database_url.trim().to_string();
    }
    // Not a URL (bare SQLite path or a malformed value): avoid echoing pairs
    // that look like credentials.
    let mut redacted = database_url.to_string();
    if let Some((before, after)) = redacted.split_once("://") {
        if let Some((userinfo, rest)) = after.split_once('@') {
            if let Some((user, _secret)) = userinfo.split_once(':') {
                redacted = format!("{before}://{user}:***@{rest}");
            }
        }
    }
    redacted
}

/// Resolves the backend from `LUDEX_DATABASE_BACKEND` and a URL.
///
/// The URL scheme wins when both are present and disagree is an error: silently
/// opening the wrong engine would be worse than refusing to start.
pub fn resolve_backend(database_url: &str, configured: Option<&str>) -> Result<Backend> {
    let explicit_url = database_url.contains("://");
    match configured.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => {
            let requested = Backend::parse(value)?;
            if explicit_url {
                let inferred = Backend::from_url(database_url)?;
                if inferred != requested {
                    bail!(
                        "LUDEX_DATABASE_BACKEND={value} conflicts with the database URL scheme ({})",
                        inferred.as_str()
                    );
                }
            }
            Ok(requested)
        }
        None => Backend::from_url(database_url),
    }
}

/// Completes an explicitly configured backend-specific URL.
///
/// `LUDEX_DATABASE_BACKEND=sqlite` with a bare path becomes
/// `sqlite://<path>?mode=rwc`; a bare `host/db` value with the postgres
/// backend becomes `postgres://host/db`. Full URLs are used verbatim.
pub fn complete_database_url(database_url: &str, backend: Backend) -> String {
    let trimmed = database_url.trim();
    if trimmed.contains("://") {
        return trimmed.to_string();
    }
    match backend {
        Backend::Sqlite => format!("sqlite://{trimmed}?mode=rwc"),
        Backend::Postgres => format!("postgres://{trimmed}"),
    }
}

/// Current UTC timestamp.
#[must_use]
pub fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Formats a timestamp for storage as RFC 3339 UTC text.
#[must_use]
pub fn db_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Parses a timestamp read from the database.
#[must_use]
pub fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

/// Parses a user- or provider-supplied date (`YYYY-MM-DD` or RFC 3339).
#[must_use]
pub fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(parsed) = parse_time(trimmed) {
        return Some(parsed);
    }
    if trimmed.len() == 4 {
        if let Ok(year) = trimmed.parse::<i32>() {
            if (1900..=2100).contains(&year) {
                return Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single();
            }
        }
    }
    chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|naive| Utc.from_utc_datetime(&naive))
}

/// Builds a midnight-UTC date from a release year.
#[must_use]
pub fn date_from_year(year: u16) -> Option<DateTime<Utc>> {
    Utc.with_ymd_and_hms(i32::from(year), 1, 1, 0, 0, 0)
        .single()
}

/// Serialises a list of strings for a `*_json` column.
#[must_use]
pub fn dump_json_list(values: &[String]) -> String {
    serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
}

/// Reads a `*_json` column holding a list of strings, tolerating old/bad data.
#[must_use]
pub fn parse_json_list(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

/// Reads a `settings` value.
pub async fn get_setting(pool: &Database, key: &str) -> Result<Option<String>> {
    let value = pool
        .query_scalar::<String>("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .context("read setting")?;
    Ok(value)
}

/// Writes a `settings` value, replacing any previous value.
pub async fn set_setting(pool: &Database, key: &str, value: &str) -> Result<()> {
    pool.query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await
    .context("write setting")?;
    Ok(())
}

/// Number of registered accounts.
pub async fn count_users(pool: &Database) -> Result<i64> {
    let count = pool
        .query_scalar::<i64>("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .context("count users")?;
    Ok(count)
}

/// Server display name: the value saved at setup, or the configured default.
pub async fn server_name(pool: &Database, fallback: &str) -> String {
    match get_setting(pool, "server_name").await {
        Ok(Some(value)) if !value.trim().is_empty() => value,
        _ => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_urls_become_paths_without_query() {
        let path =
            sqlite_path_from_url("sqlite:///srv/ludex/data/ludex.db?mode=rwc").expect("path");
        assert_eq!(path, PathBuf::from("/srv/ludex/data/ludex.db"));

        let path = sqlite_path_from_url("sqlite://./data/ludex.db?mode=rwc").expect("path");
        assert_eq!(path, PathBuf::from("./data/ludex.db"));
    }

    #[test]
    fn windows_style_sqlite_urls_are_preserved() {
        let path = sqlite_path_from_url(
            "sqlite://C:/Users/Test User/AppData/Roaming/Ludex/ludex.db?mode=rwc",
        )
        .expect("path");
        assert_eq!(
            path.to_string_lossy(),
            "C:/Users/Test User/AppData/Roaming/Ludex/ludex.db"
        );

        let path = sqlite_path_from_url(
            "sqlite://C:\\Users\\Test User\\AppData\\Roaming\\Ludex\\ludex.db?mode=rwc",
        )
        .expect("path");
        assert_eq!(
            path.to_string_lossy(),
            "C:\\Users\\Test User\\AppData\\Roaming\\Ludex\\ludex.db"
        );
    }

    #[test]
    fn percent_encoded_paths_are_decoded() {
        let path = sqlite_path_from_url("sqlite:///tmp/ludex%20data/ludex.db").expect("path");
        assert_eq!(path, PathBuf::from("/tmp/ludex data/ludex.db"));
    }

    #[test]
    fn backends_are_inferred_from_the_url() {
        assert_eq!(
            Backend::from_url("postgres://user:secret@db:5432/ludex").expect("pg"),
            Backend::Postgres
        );
        assert_eq!(
            Backend::from_url("postgresql://db/ludex").expect("pg"),
            Backend::Postgres
        );
        assert_eq!(
            Backend::from_url("sqlite:///srv/ludex/ludex.db").expect("sqlite"),
            Backend::Sqlite
        );
        assert!(Backend::from_url("mysql://db/ludex").is_err());
        assert!(Backend::from_url("").is_err());
    }

    #[test]
    fn configured_backend_must_match_the_url() {
        assert_eq!(
            resolve_backend("postgres://db/ludex", Some("postgres")).expect("ok"),
            Backend::Postgres
        );
        assert!(resolve_backend("sqlite:///tmp/ludex.db", Some("postgres")).is_err());
        assert!(resolve_backend("sqlite:///tmp/ludex.db", Some("oracle")).is_err());
    }

    #[test]
    fn bare_paths_are_completed_for_sqlite() {
        assert_eq!(
            complete_database_url("/srv/ludex/ludex.db", Backend::Sqlite),
            "sqlite:///srv/ludex/ludex.db?mode=rwc"
        );
        assert_eq!(
            complete_database_url("postgres://db/ludex", Backend::Postgres),
            "postgres://db/ludex"
        );
    }

    #[test]
    fn passwords_are_redacted() {
        assert_eq!(
            redact_database_url("postgres://ludex:s3cret@db:5432/ludex"),
            "postgres://ludex:***@db:5432/ludex"
        );
        assert_eq!(
            redact_database_url("sqlite:///srv/ludex/ludex.db"),
            "sqlite:///srv/ludex/ludex.db"
        );
    }

    #[test]
    fn placeholders_are_numbered_for_postgres() {
        assert_eq!(
            pg_placeholders("SELECT * FROM games WHERE id = ? AND title = ?"),
            "SELECT * FROM games WHERE id = $1 AND title = $2"
        );
    }

    #[test]
    fn placeholders_skip_literals_and_comments() {
        assert_eq!(
            pg_placeholders("SELECT '?' AS q, \"a?\" FROM t WHERE x = ? -- ?\n AND y = ? /* ? */"),
            "SELECT '?' AS q, \"a?\" FROM t WHERE x = $1 -- ?\n AND y = $2 /* ? */"
        );
    }

    #[test]
    fn placeholders_keep_escaped_quotes() {
        assert_eq!(
            pg_placeholders("SELECT 'it''s ?' WHERE id = ?"),
            "SELECT 'it''s ?' WHERE id = $1"
        );
    }

    #[tokio::test]
    async fn connects_to_a_data_directory_with_spaces() {
        let temp = tempfile::tempdir().expect("tempdir");
        let data_dir = temp.path().join("ludex data");
        std::fs::create_dir_all(&data_dir).expect("mkdir");
        let url = format!("sqlite://{}?mode=rwc", data_dir.join("ludex.db").display());
        let pool = connect(&url).await.expect("connect");
        let count = pool
            .query_scalar::<i64>("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .expect("query");
        assert_eq!(count, 0);
        pool.close().await;
    }
}
