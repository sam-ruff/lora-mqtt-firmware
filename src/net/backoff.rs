//! Exponential reconnect backoff, in whole seconds so it stays clock-free
//! and host-testable.

const INITIAL_SECS: u32 = 1;
const MAX_SECS: u32 = 60;

pub struct Backoff {
    next_secs: u32,
}

impl Backoff {
    pub const fn new() -> Self {
        Self { next_secs: INITIAL_SECS }
    }

    /// The delay to wait now; each call doubles the next one up to the cap.
    pub fn next_secs(&mut self) -> u32 {
        let current = self.next_secs;
        self.next_secs = (self.next_secs * 2).min(MAX_SECS);
        current
    }

    /// Call after a successful connection so the next failure starts small.
    pub fn reset(&mut self) {
        self.next_secs = INITIAL_SECS;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_to_cap() {
        let mut backoff = Backoff::new();
        let delays: [u32; 8] = core::array::from_fn(|_| backoff.next_secs());
        assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60]);
    }

    #[test]
    fn reset_restarts_from_initial() {
        let mut backoff = Backoff::new();
        for _ in 0..5 {
            backoff.next_secs();
        }
        backoff.reset();
        assert_eq!(backoff.next_secs(), 1);
        assert_eq!(backoff.next_secs(), 2);
    }
}
