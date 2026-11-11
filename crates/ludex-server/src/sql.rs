//! Small helpers for dynamic SQL with optional filters.

use crate::db::{DbQuery, DbQueryAs, DbQueryScalar, DbValue};

/// One dynamically bound SQL value.
///
/// The variants mirror [`DbValue`]; the alias keeps the call sites short.
pub type Bind = DbValue;

/// Applies dynamic bindings to a query.
#[must_use]
pub fn bind_query(mut query: DbQuery, binds: &[Bind]) -> DbQuery {
    for bind in binds {
        query = query.dyn_bind(bind.clone());
    }
    query
}

/// Applies dynamic bindings to a typed query.
#[must_use]
pub fn bind_query_as<O>(mut query: DbQueryAs<O>, binds: &[Bind]) -> DbQueryAs<O> {
    for bind in binds {
        query = query.dyn_bind(bind.clone());
    }
    query
}

/// Applies dynamic bindings to a scalar query.
#[must_use]
pub fn bind_query_scalar<O>(mut query: DbQueryScalar<O>, binds: &[Bind]) -> DbQueryScalar<O> {
    for bind in binds {
        query = query.dyn_bind(bind.clone());
    }
    query
}
