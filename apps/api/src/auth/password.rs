//! Re-export shim: control-plane password primitives now live in
//! [`crate::password`] so the Cloud auth overlay can call core instead of
//! forking. Kept so `crate::auth`'s existing `password::*` paths resolve.
pub(super) use crate::password::{hash_password, valid_control_plane_password, verify_password};
