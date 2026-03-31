//! Modifier validation hook for the router.
//!
//! The router calls the validator (if set) for each modifier in a
//! ModifierResponse before forwarding. The validator decides: accept or reject.
//! The router does not interpret the verdict beyond forward/drop.

/// Result of validating a single modifier from a ModifierResponse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierVerdict {
    /// Forward this modifier to the requester.
    Accept,
    /// Drop this modifier silently. Do not forward.
    Reject,
}

/// Hook for external validation of modifiers before forwarding.
///
/// The router calls `validate` for each modifier in a ModifierResponse.
/// Implementations can parse, verify, track, or do whatever they need.
/// The router only cares about the verdict.
///
/// # Contract
/// - Must never panic. Return `Reject` on any internal error.
/// - Called with the raw modifier bytes — parsing is the implementor's job.
/// - A `Reject` verdict means the modifier is dropped before the request
///   tracker or latency tracker see it (as if it never arrived).
pub trait ModifierValidator: Send {
    /// Validate a single modifier.
    ///
    /// - `modifier_type`: the type byte (1=Header, 2=Tx, 3=BlockTransactions, etc.)
    /// - `id`: the 32-byte modifier ID
    /// - `data`: the raw modifier bytes
    fn validate(&mut self, modifier_type: u8, id: &[u8; 32], data: &[u8]) -> ModifierVerdict;
}
