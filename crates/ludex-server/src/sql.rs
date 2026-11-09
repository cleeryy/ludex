//! Small helpers for dynamic SQL with optional filters.

use sqlx::query::{Query, QueryAs, QueryScalar};
use sqlx::sqlite::SqliteArguments;
use sqlx::Sqlite;

/// One dynamically bound SQL value.
#[derive(Debug, Clone)]
pub enum Bind {
    /// Non-null integer.
    Int(i64),
    /// Non-null float.
    Float(f64),
    /// Non-null text.
    Text(String),
    /// Nullable integer.
    OptInt(Option<i64>),
    /// Nullable float.
    OptFloat(Option<f64>),
    /// Nullable text.
    OptText(Option<String>),
}

/// Applies dynamic bindings to a `query`.
pub fn bind_query<'q>(
    mut query: Query<'q, Sqlite, SqliteArguments<'q>>,
    binds: &'q [Bind],
) -> Query<'q, Sqlite, SqliteArguments<'q>> {
    for bind in binds {
        query = match bind {
            Bind::Int(value) => query.bind(*value),
            Bind::Float(value) => query.bind(*value),
            Bind::Text(value) => query.bind(value.as_str()),
            Bind::OptInt(value) => query.bind(*value),
            Bind::OptFloat(value) => query.bind(*value),
            Bind::OptText(value) => query.bind(value.as_deref()),
        };
    }
    query
}

/// Applies dynamic bindings to a `query_scalar`.
pub fn bind_query_scalar<'q, O>(
    mut query: QueryScalar<'q, Sqlite, O, SqliteArguments<'q>>,
    binds: &'q [Bind],
) -> QueryScalar<'q, Sqlite, O, SqliteArguments<'q>> {
    for bind in binds {
        query = match bind {
            Bind::Int(value) => query.bind(*value),
            Bind::Float(value) => query.bind(*value),
            Bind::Text(value) => query.bind(value.as_str()),
            Bind::OptInt(value) => query.bind(*value),
            Bind::OptFloat(value) => query.bind(*value),
            Bind::OptText(value) => query.bind(value.as_deref()),
        };
    }
    query
}

/// Applies dynamic bindings to a `query_as`.
pub fn bind_query_as<'q, O>(
    mut query: QueryAs<'q, Sqlite, O, SqliteArguments<'q>>,
    binds: &'q [Bind],
) -> QueryAs<'q, Sqlite, O, SqliteArguments<'q>> {
    for bind in binds {
        query = match bind {
            Bind::Int(value) => query.bind(*value),
            Bind::Float(value) => query.bind(*value),
            Bind::Text(value) => query.bind(value.as_str()),
            Bind::OptInt(value) => query.bind(*value),
            Bind::OptFloat(value) => query.bind(*value),
            Bind::OptText(value) => query.bind(value.as_deref()),
        };
    }
    query
}
