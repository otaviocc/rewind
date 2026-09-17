//! Shared inputs that would otherwise ripple through every signature: right now, just the clock.

use jiff::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ctx {
    pub now: Timestamp,
}
