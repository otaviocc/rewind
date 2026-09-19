//! One cancellation mechanism, shared by the conversation loader and the scan pool: a
//! generation counter a sender bumps and a worker polls, never a thread that gets killed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Default)]
pub struct Gate(Arc<AtomicU64>);

impl Gate {
    pub fn current(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    pub fn bump(&self) -> u64 {
        self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    pub fn token(&self) -> Cancel {
        Cancel { gate: self.clone(), generation: self.current() }
    }

    pub fn token_at(&self, generation: u64) -> Cancel {
        Cancel { gate: self.clone(), generation }
    }
}

#[derive(Debug, Clone)]
pub struct Cancel {
    gate: Gate,
    generation: u64,
}

impl Cancel {
    pub fn never() -> Self {
        Self { gate: Gate::default(), generation: 0 }
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn cancelled(&self) -> bool {
        self.gate.current() != self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_token_is_not_cancelled() {
        let gate = Gate::default();
        let token = gate.token();
        assert!(!token.cancelled());
    }

    #[test]
    fn bumping_the_gate_cancels_an_outstanding_token() {
        let gate = Gate::default();
        let token = gate.token();
        gate.bump();
        assert!(token.cancelled());
    }

    #[test]
    fn a_token_taken_after_the_bump_is_not_cancelled() {
        let gate = Gate::default();
        gate.bump();
        let token = gate.token();
        assert!(!token.cancelled());
    }

    #[test]
    fn never_is_never_cancelled_even_after_other_gates_move() {
        let unrelated = Gate::default();
        unrelated.bump();
        let token = Cancel::never();
        assert!(!token.cancelled());
    }

    #[test]
    fn bump_returns_the_new_generation() {
        let gate = Gate::default();
        assert_eq!(gate.bump(), 1);
        assert_eq!(gate.bump(), 2);
    }

    #[test]
    fn token_at_pins_a_specific_generation() {
        let gate = Gate::default();
        gate.bump();
        let stale = gate.token_at(0);
        let current = gate.token_at(gate.current());
        assert!(stale.cancelled());
        assert!(!current.cancelled());
    }
}
