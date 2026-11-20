//! Cloud saves (#107, #108, #146, #153): detection, the save-root registry,
//! automatic sync, the local restore-point history (#295) and the Ludusavi
//! manifest updater.

pub(crate) mod detect;
pub(crate) mod history;
pub(crate) mod identity;
pub(crate) mod manifest;
pub(crate) mod registry;
pub(crate) mod sync;
