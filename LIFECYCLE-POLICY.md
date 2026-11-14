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

Independent code review found no blocking issue in this batch. Required embedding
contract: no other host code may reap these owned children via waitpid(-1) or change
SIGCHLD disposition concurrently. The read-only default-disposition check cannot
prove that contract for an arbitrary future host. Short mutex contention remains;
“no blocking child wait” is not “zero waiting of any kind.”
