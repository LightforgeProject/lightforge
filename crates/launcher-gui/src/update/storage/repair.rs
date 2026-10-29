//! Checking a store against its headers, and repairing what is damaged.

use crate::*;

impl Lightforge {
    pub(super) fn update_repair(&mut self, message: Message) -> Result<Task<Message>, Message> {
        self.update_quick_scan(message)
            .or_else(|m| self.update_deep_scan(m))
    }

    /// The scan against the stored headers, and repairing what it drops.
    fn update_quick_scan(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::ScanAndRepair => {
                self.gear_open = false;
                // Rewrites indices: nothing else may write or open the store.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Ok(Task::none());
                }
                if self.in_play() {
                    self.error = Some("close the game first".into());
                    return Ok(Task::none());
                }
                self.error = None;
                self.scanning = true;
                self.scan_for = Some(self.selected.code.clone());
                self.busy = true;
                self.scan_sample = Default::default();
                self.status = "checking stored files against their headers…".to_owned();

                let (tx, rx) = tokio::sync::watch::channel(Default::default());
                self.scan_rx = Some(rx);
                Task::perform(
                    tasks::scan_and_repair(self.layout.clone(), self.selected.code.clone(), tx),
                    |result| Message::Scanned(Box::new(result)),
                )
            }

            Message::Scanned(result) => {
                self.scanning = false;
                self.busy = false;
                self.scan_rx = None;
                let scanned = self.scan_for.take().unwrap_or_else(|| self.selected.code.clone());
                match *result {
                    Err(e) => {
                        self.error = Some(e.into());
                        self.pump()
                    }
                    Ok(scan) => {
                        // The download refetches what repair dropped.
                        self.status = match scan.damaged {
                            0 => format!(
                                "{} files verified — none damaged; checking for anything missing",
                                scan.checked
                            ),
                            n => format!(
                                "{n} damaged of {} — {} dropped, fetching them again",
                                scan.checked, scan.removed
                            ),
                        };
                        self.enqueue_for(scanned)
                    }
                }
            }
            other => return Err(other),
        })
    }

    /// The deep scan of every stored byte, and the repair it can lead to.
    fn update_deep_scan(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::DeepScan => {
                self.gear_open = false;
                if let Some(cancel) = &self.deep_cancel {
                    cancel.cancel();
                    self.status = "stopping the deep scan…".to_owned();
                    return Ok(Task::none());
                }
                // Every WoW flavour shares one store, so any transfer at all
                // is writing to what this would read.
                let transferring = self.winding_down.is_some()
                    || self
                        .queue
                        .iter()
                        .any(|d| matches!(d.stage, Stage::Running { .. } | Stage::Queued));
                if transferring {
                    self.error = Some(
                        "pause downloads first — they write to the files being checked".into(),
                    );
                    return Ok(Task::none());
                }
                if let Some(what) = self.maintenance() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Ok(Task::none());
                }
                self.scan_for = Some(self.selected.code.clone());

                self.error = None;
                self.scanning = true;
                self.owed_repair = None;
                self.scan_sample = Default::default();
                self.status = "checking every stored byte — the game can stay open".to_owned();

                let cancel = Cancel::new();
                self.deep_cancel = Some(cancel.clone());
                let (tx, rx) = tokio::sync::watch::channel(Default::default());
                self.scan_rx = Some(rx);
                Task::perform(tasks::deep_scan(self.layout.clone(), self.selected.code.clone(), cancel, tx), |result| {
                    Message::DeepScanned(Box::new(result))
                })
            }

            Message::DeepScanned(result) => {
                self.scanning = false;
                let scanned = self.scan_for.take().unwrap_or_else(|| self.selected.code.clone());
                self.scan_rx = None;
                let stopped = self.deep_cancel.take().is_some_and(|c| c.is_cancelled());
                match *result {
                    Err(_) if stopped => {
                        self.status = "deep scan stopped".to_owned();
                        self.pump()
                    }
                    Err(e) => {
                        self.error = Some(e.into());
                        self.pump()
                    }
                    Ok(report) if report.is_healthy() => {
                        self.status = format!(
                            "{} files checked down to the byte — none damaged",
                            report.checked
                        );
                        self.pump()
                    }
                    // Before the queue is released, or the findings go stale.
                    Ok(report) => {
                        self.status = format!(
                            "{} damaged of {} — repairing…",
                            report.damaged.len(),
                            report.checked
                        );
                        self.owed_repair = Some((scanned, Box::new(report)));
                        self.update(Message::RepairDamaged)
                    }
                }
            }

            Message::RepairDamaged => {
                self.gear_open = false;
                let Some((code, report)) = self.owed_repair.clone() else {
                    return Ok(Task::none());
                };
                // Kept owed, and said so, not started: the game has
                // the store open, or something else is writing it.
                let held = match self.store_writer() {
                    Some(what) => Some(format!("wait for {what} to finish")),
                    None => self.in_play().then(|| "close the game".to_owned()),
                };
                if let Some(held) = held {
                    self.status = format!(
                        "{} damaged files found — {held}, then choose Repair Damaged Files",
                        report.damaged.len()
                    );
                    return Ok(self.pump());
                }
                // Not `busy`, which belongs to a launch.
                self.repairing = Some(code.clone());
                Task::perform(tasks::repair(self.layout.clone(), code, *report), Message::Repaired)
            }

            Message::Repaired(result) => {
                let code = self.repairing.take().unwrap_or_else(|| self.selected.code.clone());
                match result {
                    Ok(removed) => {
                        self.owed_repair = None;
                        if code == self.selected.code {
                            self.status =
                                format!("{removed} damaged files dropped — fetching them again");
                        }
                        self.enqueue_for(code)
                    }
                    Err(tasks::Unrepaired::InUse) => {
                        let damaged = self.owed_repair.as_ref().map_or(0, |(_, r)| r.damaged.len());
                        self.status = format!(
                            "{damaged} damaged files found — close the game and Battle.net, \
                             then choose Repair Damaged Files"
                        );
                        self.pump()
                    }
                    Err(tasks::Unrepaired::Failed(e)) => {
                        self.owed_repair = None;
                        self.error = Some(e.into());
                        self.pump()
                    }
                }
            }
            other => return Err(other),
        })
    }
}
