//! The Observe and Write byte-op paths over the real `/ws/command` wire: reads,
//! run snapshots, file encodings, confined writes and the refusal of a
//! malformed run. No module sets an env var, so they share one test binary.

mod command_refusal;
mod file_encoding;
mod observe_read;
mod runs_list;
mod workspace_write;
