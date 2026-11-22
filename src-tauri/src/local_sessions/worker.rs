//! One bounded worker for local PTY drainage and active frame publication.
use super::LocalSessions;
use std::{
    sync::{Mutex, TryLockError, Weak},
    thread::{self, Thread},
    time::Duration,
};

const TICK: Duration = Duration::from_millis(25);

pub(super) struct LocalWorker(Thread);

impl LocalWorker {
    pub(super) fn start(owner: Weak<Mutex<LocalSessions>>) -> Result<Self, String> {
        let worker = thread::Builder::new()
            .name("aether-local-terminal".into())
            .spawn(move || loop {
                // Do not keep the app owner alive across a sleep, nor queue
                // for the lock behind a foreground IPC operation.
                let (check_again, delivery) = {
                    let Some(owner) = owner.upgrade() else { break };
                    let result = match owner.try_lock() {
                        Ok(mut local) => local.pump(),
                        Err(TryLockError::WouldBlock) => (true, None),
                        Err(TryLockError::Poisoned(_)) => break,
                    };
                    result
                };
                if let Some(delivery) = delivery {
                    delivery.send();
                }
                if check_again {
                    thread::park_timeout(TICK);
                } else {
                    thread::park();
                }
            })
            .map_err(|_| "local terminal worker unavailable".to_owned())?;
        Ok(Self(worker.thread().clone()))
    }

    pub(super) fn wake(&self) {
        self.0.unpark();
    }
}

impl Drop for LocalWorker {
    fn drop(&mut self) {
        // A parked thread must see its Weak owner disappear without waiting
        // for another IPC call or retaining any PTY lifecycle authority.
        self.wake();
    }
}
