//! Runtime accounting for generated input size.

/// Non-recoverable failure while walking a case: either the user interrupted
/// generation, or the case grew past the input-size safety ceiling. Constraint
/// misses are NOT errors — they flow back as `Ok(false)` so the caller can
/// resample the case.
#[derive(Debug)]
pub(super) enum GenError {
    Interrupted,
    Oversize(String),
}

pub(super) struct Budget {
    used: u128,
    limit: u128,
}

impl Budget {
    pub(super) fn new(limit: u128) -> Self {
        Self { used: 0, limit }
    }

    pub(super) fn add(&mut self, n: u128) -> Result<(), GenError> {
        self.used = self.used.checked_add(n).ok_or_else(|| {
            GenError::Oversize(
                "input too large: generated case element count overflows 128-bit range"
                    .to_owned(),
            )
        })?;
        if self.used > self.limit {
            return Err(GenError::Oversize(format!(
                "input too large: generated case has at least {} elements, exceeding the safety \
                 ceiling {}; narrow the range in the yml `vars:` section",
                self.used, self.limit
            )));
        }
        Ok(())
    }

    /// Snapshot the current usage so a failed sub-walk can be rolled back.
    pub(super) fn mark(&self) -> u128 {
        self.used
    }

    /// Discard usage charged since `mark`. `mark` must come from `Self::mark`.
    pub(super) fn rollback_to(&mut self, mark: u128) {
        debug_assert!(mark <= self.used);
        self.used = mark;
    }
}
