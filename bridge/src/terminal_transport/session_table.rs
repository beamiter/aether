//! The standalone owner alone stores and disposes each registered bridge.
use std::{
    collections::{hash_map::Entry, HashMap},
    time::{Duration, Instant},
};

pub(super) struct SessionTable<T, const LIMIT: usize> {
    sessions: HashMap<String, Owned<T>>,
}

struct Owned<T> {
    value: T,
    last_used: Instant,
}

impl<T, const LIMIT: usize> Default for SessionTable<T, LIMIT> {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }
}

impl<T, const LIMIT: usize> SessionTable<T, LIMIT> {
    // No owner is constructed at capacity. The stable ID is created by the
    // bridge, never taken from a request or an attachment generation.
    pub(super) fn admit<R>(
        &mut self,
        create: impl FnOnce() -> Result<(String, T, R), String>,
    ) -> Result<R, String> {
        if self.sessions.len() >= LIMIT {
            return Err("terminal host session limit reached".into());
        }
        let (id, value, reply) = create()?;
        match self.sessions.entry(id) {
            Entry::Vacant(slot) => {
                slot.insert(Owned {
                    value,
                    last_used: Instant::now(),
                });
                Ok(reply)
            }
            // Value has not entered the table and only it is dropped.
            Entry::Occupied(_) => Err("terminal session ID collision".into()),
        }
    }

    pub(super) fn get_mut(&mut self, id: &str) -> Option<&mut T> {
        self.sessions.get_mut(id).map(|owned| &mut owned.value)
    }

    pub(super) fn activity_mut(&mut self) -> impl Iterator<Item = (&mut T, &mut Instant)> {
        self.sessions
            .values_mut()
            .map(|owned| (&mut owned.value, &mut owned.last_used))
    }

    pub(super) fn remove(&mut self, id: &str) {
        self.sessions.remove(id);
    }

    pub(super) fn touch(&mut self, id: &str, now: Instant) {
        // Only called after a successful operation on an existing entry.
        self.sessions.get_mut(id).unwrap().last_used = now;
    }

    pub(super) fn expire(&mut self, now: Instant, inactivity: Duration) {
        self.sessions
            .retain(|_, owned| now.duration_since(owned.last_used) < inactivity);
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[derive(Default)]
    struct Disposal(Rc<Cell<usize>>);

    impl Drop for Disposal {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn admission_checks_capacity_before_creation_and_keeps_the_old_owner_on_collision() {
        let mut sessions = SessionTable::<Disposal, 2>::default();
        let original = Disposal::default();
        let original_drops = original.0.clone();
        sessions.admit(|| Ok(("a".into(), original, ()))).unwrap();
        let collision = Disposal::default();
        let collision_drops = collision.0.clone();
        assert_eq!(
            sessions
                .admit(|| Ok(("a".into(), collision, ())))
                .unwrap_err(),
            "terminal session ID collision"
        );
        assert_eq!(collision_drops.get(), 1);
        assert_eq!(original_drops.get(), 0);
        sessions
            .admit(|| Ok(("b".into(), Disposal::default(), ())))
            .unwrap();
        assert_eq!(
            sessions
                .admit::<()>(|| panic!("must reject before creating a new owner"))
                .unwrap_err(),
            "terminal host session limit reached"
        );
        sessions.remove("a");
        assert_eq!(original_drops.get(), 1);
        assert!(sessions.get_mut("a").is_none());
        assert!(sessions.get_mut("b").is_some());
        sessions
            .admit(|| Ok(("c".into(), Disposal::default(), ())))
            .unwrap();
    }

    #[test]
    fn failed_creation_keeps_capacity_and_expiry_is_per_session() {
        let mut sessions = SessionTable::<u8, 2>::default();
        assert_eq!(
            sessions.admit::<()>(|| Err("test error".into())),
            Err("test error".into())
        );
        assert!(sessions.is_empty());
        sessions.admit(|| Ok(("a".into(), 1, ()))).unwrap();
        sessions.admit(|| Ok(("b".into(), 2, ()))).unwrap();
        let inactivity = Duration::from_secs(600);
        let now = Instant::now();
        sessions.touch("a", now);
        sessions.touch("b", now + Duration::from_secs(1));
        sessions.expire(now + inactivity, inactivity);
        assert!(sessions.get_mut("a").is_none());
        assert_eq!(sessions.get_mut("b"), Some(&mut 2));
    }
}
