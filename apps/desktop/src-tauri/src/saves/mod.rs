//! Cloud saves (#107, #108, #146, #153): detection, the save-root registry,
//! automatic sync and the Ludusavi manifest updater.

pub(crate) mod detect;
pub(crate) mod manifest;
pub(crate) mod registry;
pub(crate) mod sync;
