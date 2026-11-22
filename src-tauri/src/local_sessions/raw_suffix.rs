//! Owns an explicit local command's unwritten suffix, never schedules a write.

const MAX_RAW_BYTES: usize = 4096;

#[derive(Default)]
pub(super) enum RawSuffix<Owner> {
    #[default]
    None,
    Pending {
        owner: Owner,
        unsent: Box<[u8]>,
        prefix_accepted: bool,
    },
    Interrupted,
}

impl<Owner: PartialEq> RawSuffix<Owner> {
    pub(super) fn write(
        &mut self,
        owner: Owner,
        bytes: Vec<u8>,
        send: impl FnOnce(&[u8]) -> Result<usize, String>,
    ) -> Result<usize, String> {
        if bytes.len() > MAX_RAW_BYTES {
            return Err("input byte limit".into());
        }
        let prefix_accepted = match self {
            Self::None => false,
            Self::Pending {
                owner: expected,
                unsent,
                prefix_accepted,
            } => {
                if *expected != owner || &**unsent != bytes.as_slice() {
                    return Err(
                        "local command continuation does not match its pending suffix".into(),
                    );
                }
                *prefix_accepted
            }
            Self::Interrupted => {
                return Err("local command input interrupted; close the shell".into())
            }
        };
        let accepted = match send(&bytes) {
            Ok(count) if count <= bytes.len() => count,
            Ok(_) => {
                *self = Self::Interrupted;
                return Err("invalid write count".into());
            }
            Err(error) => {
                *self = Self::Interrupted;
                return Err(error);
            }
        };
        *self = if accepted == bytes.len() {
            Self::None
        } else {
            Self::Pending {
                owner,
                unsent: bytes[accepted..].into(),
                prefix_accepted: prefix_accepted || accepted > 0,
            }
        };
        Ok(accepted)
    }

    pub(super) fn retire_view(&mut self) {
        if let Self::Pending {
            prefix_accepted, ..
        } = self
        {
            *self = if *prefix_accepted {
                Self::Interrupted
            } else {
                Self::None
            };
        }
    }

    pub(super) fn retain_only_current(&mut self, current: &Owner) {
        if matches!(self, Self::Pending { owner, .. } if owner != current) {
            self.retire_view();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_count_and_owner_gate_zero_and_positive_short_writes() {
        let mut q = RawSuffix::default();
        let mut wire = vec![];
        let line = b"\xc3\xa9\n";
        let first = (1, 2, 3);
        let wrong = (1, 2, 4);
        assert_eq!(q.write(first, line.to_vec(), |_| Ok(0)), Ok(0));
        assert!(q
            .write(wrong, line.to_vec(), |_| panic!("foreign generation"))
            .is_err());
        assert!(q
            .write(first, b"different\n".to_vec(), |_| panic!(
                "replacement bytes"
            ))
            .is_err());
        assert_eq!(
            q.write(first, line.to_vec(), |offered| {
                assert_eq!(offered, line);
                wire.extend_from_slice(&offered[..1]);
                Ok(1)
            }),
            Ok(1)
        );
        assert!(q
            .write(first, line.to_vec(), |_| panic!("cannot replay prefix"))
            .is_err());
        q.retain_only_current(&first);
        assert_eq!(
            q.write(first, line[1..].to_vec(), |offered| {
                wire.extend_from_slice(offered);
                Ok(offered.len())
            }),
            Ok(2)
        );
        assert_eq!(wire, line);
        assert!(matches!(q, RawSuffix::None));
    }

    #[test]
    fn retirement_distinguishes_zero_from_started_and_never_sends_a_suffix() {
        let mut q = RawSuffix::default();
        assert_eq!(q.write(1, b"a\n".to_vec(), |_| Ok(0)), Ok(0));
        q.retire_view();
        assert!(matches!(q, RawSuffix::None));
        assert_eq!(q.write(2, b"b\n".to_vec(), |_| Ok(1)), Ok(1));
        q.retain_only_current(&3);
        assert!(matches!(q, RawSuffix::Interrupted));
        assert!(q
            .write(3, b"\n".to_vec(), |_| panic!("retired suffix"))
            .is_err());
    }

    #[test]
    fn invalid_results_stop_input_and_bounds_match_existing_form() {
        let mut q = RawSuffix::default();
        assert!(q
            .write(1, vec![0; MAX_RAW_BYTES + 1], |_| panic!("oversized"))
            .is_err());
        assert_eq!(
            q.write(1, vec![b'a'; MAX_RAW_BYTES], |b| Ok(b.len())),
            Ok(MAX_RAW_BYTES)
        );
        assert!(q.write(1, b"x\n".to_vec(), |_| Ok(3)).is_err());
        assert!(q
            .write(1, b"x\n".to_vec(), |_| panic!("unknown accepted count"))
            .is_err());
        let mut q = RawSuffix::default();
        assert_eq!(
            q.write(1, b"x\n".to_vec(), |_| Err("lost acknowledgement".into())),
            Err("lost acknowledgement".into())
        );
        q.retire_view();
        assert!(q
            .write(2, b"x\n".to_vec(), |_| panic!("unknown acceptance"))
            .is_err());
    }
}
