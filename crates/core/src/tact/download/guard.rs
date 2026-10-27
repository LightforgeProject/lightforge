use super::Cancel;

/// The downloads this process has running, keyed by product: the real
/// [`crate::preflight::DownloadGuard`].
///
/// Agent and Lightforge writing the same `data.NNN` and `.idx` files is
/// corruption, so a launch pauses any download first. "Resume" re-plans
/// against the checkpointed store, which needs the network, so
/// [`Downloads::resume`] only reports *that* a product needs restarting.
#[derive(Debug, Clone, Default)]
pub struct Downloads {
    running: std::collections::HashMap<crate::ProductCode, Cancel>,
    paused: std::collections::HashSet<crate::ProductCode>,
}

impl Downloads {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a download as running, so a launch can pause it.
    pub fn register(&mut self, code: crate::ProductCode, cancel: Cancel) {
        self.paused.remove(&code);
        self.running.insert(code, cancel);
    }

    /// Drop a download that finished on its own. Not the same as pausing one:
    /// nothing is owed afterwards.
    pub fn finished(&mut self, code: &crate::ProductCode) {
        self.running.remove(code);
        self.paused.remove(code);
    }

    /// Products paused by a launch and not yet restarted.
    pub fn pending(&self) -> impl Iterator<Item = &crate::ProductCode> {
        self.paused.iter()
    }
}

impl crate::preflight::DownloadGuard for Downloads {
    fn pause(&mut self, code: &crate::ProductCode) -> Result<bool, crate::preflight::PreflightError> {
        match self.running.remove(code) {
            Some(cancel) => {
                cancel.cancel();
                self.paused.insert(code.clone());
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn resume(&mut self, code: &crate::ProductCode) -> Result<bool, crate::preflight::PreflightError> {
        Ok(self.paused.remove(code))
    }
}
