#![cfg(target_os = "linux")]

use jterm_headless_session_spike::Session;
use std::{
    mem::MaybeUninit,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
    thread,
    time::{Duration, Instant},
};

static WATCHED_PID: AtomicI32 = AtomicI32::new(0);
static WATCHED_REAPED: AtomicBool = AtomicBool::new(false);

extern "C" fn watch_child(_: libc::c_int) {
    let pid = WATCHED_PID.load(Ordering::Relaxed);
    if pid > 0 && !WATCHED_REAPED.load(Ordering::Relaxed) {
        unsafe {
            let errno = libc::__errno_location();
            let saved = *errno;
            if libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) == pid {
                WATCHED_REAPED.store(true, Ordering::Relaxed);
            }
            *errno = saved;
        }
    }
}

struct RestoreAction(libc::sigaction);

impl Drop for RestoreAction {
    fn drop(&mut self) {
        unsafe { libc::sigaction(libc::SIGCHLD, &self.0, std::ptr::null_mut()) };
    }
}

fn install_action(handler: libc::sighandler_t, flags: libc::c_int) -> RestoreAction {
    let mut previous = MaybeUninit::uninit();
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handler;
        action.sa_flags = flags;
        assert_eq!(libc::sigemptyset(&mut action.sa_mask), 0);
        assert_eq!(
            libc::sigaction(libc::SIGCHLD, &action, previous.as_mut_ptr()),
            0
        );
        RestoreAction(previous.assume_init())
    }
}

fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

// This separate integration-test process has no other tests that could spawn
// children while this test installs process-wide signal dispositions.
#[test]
fn scoped_sigchld_watch_and_pty_wait_for_their_own_children() {
    let handler = watch_child as *const () as libc::sighandler_t;
    {
        let _restore = install_action(handler, libc::SA_RESTART | libc::SA_NOCLDSTOP);
        let mut watched = Command::new("/bin/sh")
            .args(["-c", "read release"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        WATCHED_PID.store(watched.id() as libc::pid_t, Ordering::Relaxed);

        let mut session = Session::spawn(4, 48).unwrap();
        let command = b"printf 'PTY_%s\\n' WAIT_OK; exit\n";
        let mut sent = 0;
        until("PTY child to be drained", || {
            if sent < command.len() {
                sent += session.write(&command[sent..]).unwrap();
            }
            session.poll().unwrap();
            session.is_drained()
        });
        assert!(session.terminal().visible_text().contains("PTY_WAIT_OK"));
        session.close().unwrap();
        until("PTY child cleanup", || !session.cleanup_pending());

        drop(watched.stdin.take());
        until("registered child to be reaped by its watcher", || {
            WATCHED_REAPED.load(Ordering::Relaxed)
        });
        let error = watched.try_wait().unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    }

    for (handler, flags) in [(libc::SIG_IGN, 0), (handler, libc::SA_NOCLDWAIT)] {
        let _restore = install_action(handler, flags);
        assert_eq!(
            Session::spawn(4, 48).err().as_deref(),
            Some("exclusive child wait ownership requires waitable children")
        );
    }
}
