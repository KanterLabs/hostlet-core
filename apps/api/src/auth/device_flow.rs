//! Re-export shim: GitHub device-flow persistence now lives in
//! [`crate::device_flow`] so the Cloud auth overlay can call core instead of
//! forking. Kept so `crate::auth`'s existing `device_flow::*` paths resolve.
pub(super) use crate::device_flow::{
    delete_device_flow, load_device_flow, store_device_flow, StoredDeviceFlow,
};
