//! One notifier for the local owner, not an independent reader or session.
use super::LocalSessions;
use std::{
    sync::{Mutex, TryLockError, Weak},
    thread::{self, Thread},
    time::Duration,
};

const TICK: Duration = Duration::from_millis(25);

pub(super) struct DetachedWorker(Thread);

impl DetachedWorker {
    pub(super) fn start(owner: Weak<Mutex<LocalSessions>>) -> Result<Self, String> {
        let worker = thread::Builder::new()
            .name("aether-local-detached".into())
            .spawn(move || loop {
                // Do not keep the app owner alive across a sleep, nor queue
                // for the lock behind a foreground IPC operation.
                let check_again = {
                    let Some(owner) = owner.upgrade() else { break };
                    let result = match owner.try_lock() {
                        Ok(mut local) => local.pump_detached(),
                        Err(TryLockError::WouldBlock) => true,
                        Err(TryLockError::Poisoned(_)) => break,
                    };
                    result
                };
                if check_again {
                    thread::park_timeout(TICK);
                } else {
                    thread::park();
                }
            })
            .map_err(|_| "detached terminal worker unavailable".to_owned())?;
        Ok(Self(worker.thread().clone()))
    }

    pub(super) fn wake(&self) {
        self.0.unpark();
    }
}

impl Drop for DetachedWorker {
    fn drop(&mut self) {
        // A parked thread must see its Weak owner disappear without waiting
        // for another IPC call or retaining any PTY lifecycle authority.
        self.wake();
    }
}
