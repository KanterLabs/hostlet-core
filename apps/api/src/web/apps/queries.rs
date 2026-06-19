//! Shared SQL for the app read endpoints.
//!
//! `list_apps` and `get_app` return the exact same projected app shape and
//! differ only in their final `WHERE` clause. `APP_SELECT_BODY` is sourced from
//! [`crate::apps::serialization`] — the single source of truth shared with the
//! Cloud overlay — and the format strings below append the caller-specific
//! trailing clause.

use crate::apps::serialization::APP_SELECT_BODY;

/// All apps owned by `$1`, newest first.
pub(super) fn list_query() -> String {
    format!(
        "{APP_SELECT_BODY}        WHERE a.user_id=$1\n        ORDER BY a.created_at DESC\n        "
    )
}

/// A single app `$1` owned by `$2`.
pub(super) fn get_query() -> String {
    format!("{APP_SELECT_BODY}        WHERE a.id=$1 AND a.user_id=$2\n        ")
}
