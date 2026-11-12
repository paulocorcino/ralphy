//! `Debug` for the PTY types: the environment shows only its keys, because a
//! value can be a secret that the child needs.

use std::fmt;

use crate::{PtyCommand, PtySession};

impl fmt::Debug for PtyCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env_keys: Vec<&std::ffi::OsString> = self.env.iter().map(|(key, _)| key).collect();
        f.debug_struct("PtyCommand")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("cwd", &self.cwd)
            .field("env_keys", &env_keys)
            .field("env_remove", &self.env_remove)
            .field("rows", &self.rows)
            .field("cols", &self.cols)
            .finish()
    }
}

impl fmt::Debug for PtySession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PtySession")
            .field("process_id", &self.process_id())
            .finish_non_exhaustive()
    }
}
