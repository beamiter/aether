# Direct-child lifecycle policy

The prototype must not claim descendant containment. The available cloud host has
no writable delegated cgroup and no effective capabilities. A job can fork or
create another session; signalling a guessed process group or enumerating matching
PIDs does not establish safe ownership. No process-tree kill is implemented.

The reviewed design uses one fixed-capacity supervisor, reserves capacity before
spawning, and never creates a thread per child. Session close transfers direct-child
ownership rather than blocking in wait. A Linux pidfd, not a reusable numeric PID,
is required for any signal. Reaping uses nonblocking try_wait; an error is not
proof that a child remains owned/alive. Unresolved children retain capacity rather
than being forgotten or allowing unbounded new sessions.

Close acknowledgment distinguishes transfer from confirmed reaping. Output recovery
does not implicitly terminate processes, replay input, or release an uncertain
input safety lock. Production descendant containment needs a separate reviewed
platform facility (such as delegated cgroup ownership), not a stronger UI claim.

Signals/children may finish only after the host gets CPU time; this is a resource
bound, not a real-time shutdown promise. A quarantined supervisor rejects new
sessions and retains uncertain ownership. App process exit can interrupt pending
cleanup. No kernel cgroup delegation, namespace setup or system signal setting
change was performed to conceal these remaining limits.

The adapter reads the host SIGCHLD disposition before each spawn and rejects
SIG_IGN or SA_NOCLDWAIT, which make child exit status unavailable. GTK can install
a handler for its own child watches. GLib waits for the specific children it
registered, and the adapter waits for its own shell. The adapter leaves signal
handling to the host. [GLib child watch contract](https://docs.gtk.org/glib/func.child_watch_source_new.html)

Embedding hosts must keep PTY children waitable and reserve their waits for the
adapter. A catch-all wait or a second watcher on the same PTY child violates that
contract. If a child wait fails, the adapter discards signalling authority and
keeps the slot under observation. Short mutex contention remains possible.
