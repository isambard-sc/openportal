// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Replay protection for ongoing message traffic - see
//! `docs/plans/replay-protection-design.md` for the full design and
//! rationale. Deliberately the textbook IPsec/WireGuard-style anti-replay
//! window (a monotonically increasing per-sender nonce, and a receiver-side
//! high-water-mark plus a fixed-size bitmap of recently-accepted values),
//! not a bespoke scheme - named `anti_replay` rather than `replay` to avoid
//! reading as a typo of `paddington::relay`.

use serde::{Deserialize, Serialize};

/// Number of trailing nonces the receiver can still accept out of order -
/// i.e. how far behind the highest nonce seen so far a message can still
/// arrive and be checked, rather than being rejected outright as too old.
/// 128 bytes of state per peer relationship at this size - tunable if real
/// operational experience ever calls for it, but not currently exposed as
/// a config option (see the design doc §2).
const WINDOW_BITS: u64 = 1024;
const WINDOW_WORDS: usize = (WINDOW_BITS / 64) as usize;

/// An IPsec/WireGuard-style sliding anti-replay window. Tracks the highest
/// nonce accepted so far, plus a fixed-size bitmap recording which of the
/// `WINDOW_BITS` nonces immediately behind it have already been seen.
///
/// One instance per *direction* of one peer relationship (a connection or
/// relayed session has one for verifying what it receives; the nonce it
/// generates for its own outgoing messages is a separate, simple counter -
/// see `next_nonce` fields on `ConnectionState`/`RelayedSession`). Reset by
/// simply constructing a fresh one - see the design doc §4.3 for why every
/// call site that owns one already reconstructs it from scratch on
/// reconnect/re-bootstrap, so no explicit "reset" method is needed.
#[derive(Debug, Clone)]
pub(crate) struct ReplayWindow {
    initialized: bool,
    highest: u64,
    bitmap: [u64; WINDOW_WORDS],
}

impl Default for ReplayWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayWindow {
    pub(crate) fn new() -> Self {
        ReplayWindow {
            initialized: false,
            highest: 0,
            bitmap: [0; WINDOW_WORDS],
        }
    }

    /// Convenience for a payload that might not carry a nonce at all - a
    /// not-yet-upgraded peer, see `NoncedPayload` - which is always
    /// accepted, since there is nothing to check it against.
    pub(crate) fn check_and_record_optional(&mut self, nonce: Option<u64>) -> bool {
        match nonce {
            Some(nonce) => self.check_and_record(nonce),
            None => true,
        }
    }

    /// Check `nonce` against everything seen so far, recording it if it's
    /// new. Returns `true` if `nonce` should be accepted (never seen
    /// before, and not too old to tell), `false` if it's a replay (or a
    /// nonce so old the window can no longer distinguish it from one).
    pub(crate) fn check_and_record(&mut self, nonce: u64) -> bool {
        if !self.initialized {
            self.initialized = true;
            self.highest = nonce;
            self.set_bit(0);
            return true;
        }

        if nonce > self.highest {
            let advance = nonce - self.highest;
            self.shift_left(advance);
            self.highest = nonce;
            self.set_bit(0);
            return true;
        }

        let age = self.highest - nonce;

        if age >= WINDOW_BITS {
            // too old for the window to say anything about - treat as a
            // replay rather than silently trusting it.
            return false;
        }

        if self.test_bit(age) {
            return false;
        }

        self.set_bit(age);
        true
    }

    fn set_bit(&mut self, bit: u64) {
        let bit = bit as usize;
        self.bitmap[bit / 64] |= 1u64 << (bit % 64);
    }

    fn test_bit(&self, bit: u64) -> bool {
        let bit = bit as usize;
        (self.bitmap[bit / 64] >> (bit % 64)) & 1 != 0
    }

    /// Ages every currently-tracked bit by `k` positions (bit `j`, meaning
    /// "nonce `highest - j` has been seen", becomes bit `j + k` - the same
    /// nonce, now further behind the *new* `highest`). Implemented as a
    /// plain multi-word left shift of the bitmap treated as one big
    /// unsigned integer (`bitmap[0]` least significant) - the standard
    /// technique for a fixed-width bitset shift, not anything specific to
    /// replay windows.
    fn shift_left(&mut self, k: u64) {
        if k >= WINDOW_BITS {
            self.bitmap = [0; WINDOW_WORDS];
            return;
        }

        let word_shift = (k / 64) as usize;
        let bit_shift = (k % 64) as u32;

        if word_shift > 0 {
            for i in (word_shift..WINDOW_WORDS).rev() {
                self.bitmap[i] = self.bitmap[i - word_shift];
            }
            for i in 0..word_shift {
                self.bitmap[i] = 0;
            }
        }

        if bit_shift > 0 {
            for i in (1..WINDOW_WORDS).rev() {
                self.bitmap[i] =
                    (self.bitmap[i] << bit_shift) | (self.bitmap[i - 1] >> (64 - bit_shift));
            }
            self.bitmap[0] <<= bit_shift;
        }
    }
}

/// Wraps an ongoing-traffic payload with its sender-assigned nonce before
/// encryption, in place of the bare string sent today. `#[serde(untagged)]`
/// so a payload from a not-yet-upgraded peer (a bare JSON string, the
/// current wire shape) is still recognised as `Legacy` rather than failing
/// to deserialise outright - see the design doc §4.1/§5 for why this is
/// *not* a compatibility bridge (an upgraded sender's `Nonced` shape still
/// cannot be parsed by old code), just defensive handling of whatever
/// reaches this deserialisation step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum NoncedPayload {
    Nonced { nonce: u64, payload: String },
    Legacy(String),
}

impl NoncedPayload {
    pub(crate) fn new(nonce: u64, payload: String) -> Self {
        NoncedPayload::Nonced { nonce, payload }
    }

    /// Wraps `payload` for sending to a specific peer, given whether *that
    /// peer* has confirmed (via `PeerDetails::supports_nonce` for a direct
    /// connection, or `StartRelayedConnection`/`RelayedConnectionAccepted`
    /// for a relayed one) that it understands the `Nonced` shape. Sending
    /// `Nonced` to a peer that never confirmed support would break it
    /// outright rather than degrade gracefully, so a peer that hasn't
    /// confirmed gets `Legacy` instead - which serialises as exactly the
    /// bare string it already expects. See
    /// `docs/plans/replay-protection-design.md` §9.
    pub(crate) fn for_peer(nonce: u64, payload: String, peer_supports_nonce: bool) -> Self {
        if peer_supports_nonce {
            Self::new(nonce, payload)
        } else {
            NoncedPayload::Legacy(payload)
        }
    }

    /// The nonce, if this payload carries one (i.e. came from an upgraded
    /// peer), and the inner payload string either way.
    pub(crate) fn into_parts(self) -> (Option<u64>, String) {
        match self {
            NoncedPayload::Nonced { nonce, payload } => (Some(nonce), payload),
            NoncedPayload::Legacy(payload) => (None, payload),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_nonce_always_accepted() {
        let mut window = ReplayWindow::new();
        assert!(window.check_and_record(0));

        // and a fresh window with a non-zero first nonce is fine too -
        // there's nothing special about starting at 0.
        let mut window = ReplayWindow::new();
        assert!(window.check_and_record(12345));
    }

    #[test]
    fn test_strictly_increasing_sequence_all_accepted() {
        let mut window = ReplayWindow::new();
        for nonce in 0..2000u64 {
            assert!(window.check_and_record(nonce), "nonce {} rejected", nonce);
        }
    }

    #[test]
    fn test_exact_duplicate_rejected() {
        let mut window = ReplayWindow::new();
        assert!(window.check_and_record(10));
        assert!(window.check_and_record(11));
        assert!(
            !window.check_and_record(10),
            "duplicate of 10 must be rejected"
        );
        assert!(
            !window.check_and_record(11),
            "duplicate of 11 must be rejected"
        );
    }

    #[test]
    fn test_out_of_order_within_window_accepted_once() {
        let mut window = ReplayWindow::new();
        assert!(window.check_and_record(100));
        assert!(window.check_and_record(105)); // jump ahead
        assert!(window.check_and_record(102)); // arrives late, still in window
        assert!(!window.check_and_record(102)); // replay of the late one
        assert!(window.check_and_record(103));
        assert!(window.check_and_record(104));
        assert!(!window.check_and_record(100)); // replay of the very first
    }

    #[test]
    fn test_nonce_older_than_window_rejected() {
        let mut window = ReplayWindow::new();
        assert!(window.check_and_record(0));
        assert!(window.check_and_record(WINDOW_BITS + 500));
        // nonce 0 is now far more than WINDOW_BITS behind the new highest
        assert!(!window.check_and_record(0));
        // a nonce right at the edge of the window is still checkable
        let edge = (WINDOW_BITS + 500) - (WINDOW_BITS - 1);
        assert!(window.check_and_record(edge));
        assert!(!window.check_and_record(edge));
    }

    #[test]
    fn test_huge_jump_forward_clears_window_without_panicking() {
        let mut window = ReplayWindow::new();
        assert!(window.check_and_record(5));
        assert!(window.check_and_record(u64::MAX));
        // the huge jump must not leave stale bits reachable
        assert!(!window.check_and_record(u64::MAX));
        assert!(!window.check_and_record(5));
    }

    #[test]
    fn test_nonced_payload_roundtrip() {
        let wrapped = NoncedPayload::new(42, "hello".to_string());
        let json = serde_json::to_string(&wrapped).unwrap_or_else(|e| {
            unreachable!("serialise: {:?}", e);
        });

        let parsed: NoncedPayload = serde_json::from_str(&json).unwrap_or_else(|e| {
            unreachable!("deserialise: {:?}", e);
        });
        let (nonce, payload) = parsed.into_parts();
        assert_eq!(nonce, Some(42));
        assert_eq!(payload, "hello");
    }

    #[test]
    fn test_for_peer_wraps_only_when_peer_supports_nonce() {
        let wrapped = NoncedPayload::for_peer(7, "hello".to_string(), true);
        assert!(matches!(wrapped, NoncedPayload::Nonced { nonce: 7, .. }));

        let wrapped = NoncedPayload::for_peer(7, "hello".to_string(), false);
        assert!(matches!(wrapped, NoncedPayload::Legacy(ref s) if s == "hello"));
    }

    #[test]
    fn test_for_peer_legacy_serialises_identically_to_a_bare_string() {
        // this is the entire backward-compatibility mechanism: sending
        // `Legacy` to a peer that hasn't confirmed `supports_nonce` must
        // produce byte-identical wire output to the pre-nonce bare-string
        // format, or an old peer's deserialiser would choke on it.
        let wrapped = NoncedPayload::for_peer(7, "hello".to_string(), false);
        let wrapped_json = serde_json::to_string(&wrapped).unwrap_or_else(|e| {
            unreachable!("serialise wrapped: {:?}", e);
        });
        let bare_json = serde_json::to_string("hello").unwrap_or_else(|e| {
            unreachable!("serialise bare: {:?}", e);
        });
        assert_eq!(wrapped_json, bare_json);
    }

    #[test]
    fn test_nonced_payload_accepts_legacy_bare_string() {
        // what a not-yet-upgraded peer's payload looks like once
        // decrypted: just a plain JSON string, no wrapper object at all.
        let json = serde_json::to_string("a bare payload string").unwrap_or_else(|e| {
            unreachable!("serialise: {:?}", e);
        });

        let parsed: NoncedPayload = serde_json::from_str(&json).unwrap_or_else(|e| {
            unreachable!("deserialise: {:?}", e);
        });
        let (nonce, payload) = parsed.into_parts();
        assert_eq!(nonce, None);
        assert_eq!(payload, "a bare payload string");
    }
}
