// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Replay protection for ongoing message traffic - see
//! `docs/plans/replay-protection-design.md` for the full design and
//! rationale. Deliberately the textbook IPsec/WireGuard-style anti-replay
//! window (a monotonically increasing per-sender nonce, and a receiver-side
//! high-water-mark plus a fixed-size bitmap of recently-accepted values),
//! not a bespoke scheme - named `anti_replay` rather than `replay` to avoid
//! reading as a typo of `paddington::relay`.

use once_cell::sync::Lazy;
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

    /// `bit` is always `< WINDOW_BITS` at every call site (the `age >=
    /// WINDOW_BITS` guard in `check_and_record` is what establishes that), so
    /// the word index is always in range. Accessed via `get_mut`/`get` anyway
    /// so that a future caller which loses that invariant degrades to a
    /// dropped bit rather than a panic - the release profile sets
    /// `panic = "abort"`, so an out-of-range index here would kill the
    /// process. See docs/specifications/security-review-2.md (finding R1).
    fn set_bit(&mut self, bit: u64) {
        let bit = bit as usize;
        if let Some(word) = self.bitmap.get_mut(bit / 64) {
            *word |= 1u64 << (bit % 64);
        }
    }

    fn test_bit(&self, bit: u64) -> bool {
        let bit = bit as usize;
        self.bitmap
            .get(bit / 64)
            .map(|word| (word >> (bit % 64)) & 1 != 0)
            .unwrap_or(false)
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

        // Both loops are bounded by `WINDOW_WORDS`, i.e. by the length of
        // `bitmap` itself, so every index below is in range by construction.
        // Written with `get`/`get_mut` rather than `[]` so that this stays
        // true even if the bounds are ever edited - see
        // docs/specifications/security-review-2.md (finding R1).
        if word_shift > 0 {
            for i in (word_shift..WINDOW_WORDS).rev() {
                let carried = self.bitmap.get(i - word_shift).copied().unwrap_or(0);
                if let Some(word) = self.bitmap.get_mut(i) {
                    *word = carried;
                }
            }
            for word in self.bitmap.iter_mut().take(word_shift) {
                *word = 0;
            }
        }

        if bit_shift > 0 {
            for i in (1..WINDOW_WORDS).rev() {
                let lower = self.bitmap.get(i - 1).copied().unwrap_or(0);
                if let Some(word) = self.bitmap.get_mut(i) {
                    *word = (*word << bit_shift) | (lower >> (64 - bit_shift));
                }
            }
            if let Some(word) = self.bitmap.first_mut() {
                *word <<= bit_shift;
            }
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

/// How many distinct sender incarnations (epochs) a receiver tracks per peer
/// before evicting the least recently used.
///
/// Sized against the two things that legitimately produce several live epochs
/// for one peer identity: process restarts, and client HA, where several
/// physical processes present the *same* `name@zone`
/// ([highavailability.md](../../docs/specifications/highavailability.md) §2) and
/// so each contribute an epoch. HA arbitration already caps concurrent standby
/// connections at 16, and 8 comfortably covers "the current primary plus a few
/// replicas plus a couple of recent restarts". Evicting a window means nonces
/// from that (long-superseded) incarnation become replayable again - which
/// requires 8 subsequent restarts/replicas after the capture, and buys an
/// attacker only what §10.1 of the design doc already describes for a replayed
/// handshake: wasted work, not authentication. See
/// `docs/specifications/security-review-2.md` (finding R10).
const MAX_TRACKED_EPOCHS: usize = 8;

/// A per-process random value identifying *this* incarnation of the process,
/// sent alongside every handshake/bootstrap nonce so a receiver can tell
/// "the peer restarted and its counter legitimately went back to zero" from
/// "someone is replaying an old message".
///
/// Random rather than clock-derived on purpose. A monotonically increasing
/// epoch would let the receiver simply require "newer than the last one I
/// saw", but that breaks client HA: several processes share one peer identity,
/// and the replica with the lower epoch would be rejected as a replay and could
/// never reach standby. Because the epoch is random and therefore unordered,
/// freshness comes from keeping a *separate window per epoch* (see
/// `HandshakeNonceState`) rather than from ordering.
static PROCESS_EPOCH: Lazy<u64> = Lazy::new(|| {
    let mut bytes = [0u8; 8];

    match orion::util::secure_rand_bytes(&mut bytes) {
        Ok(()) => u64::from_le_bytes(bytes),
        Err(e) => {
            // The CSPRNG failing is not recoverable in any useful sense, but a
            // fixed fallback is still better than aborting the process: it
            // degrades this peer to the pre-epoch behaviour (one shared
            // window) rather than taking the agent down.
            tracing::error!(
                "Could not generate a process epoch from the CSPRNG ({}) - \
                 falling back to a fixed value, which disables per-incarnation \
                 replay-window separation for this process.",
                e
            );
            0
        }
    }
});

/// This process's epoch - see `PROCESS_EPOCH`.
pub(crate) fn process_epoch() -> u64 {
    *PROCESS_EPOCH
}

/// Per-peer nonce/replay state for handshake- and bootstrap-phase
/// messages (`Handshake`/`PeerDetails` for direct connections;
/// `StartRelayedConnection`/`RelayedConnectionAccepted`/`SessionUnknown`
/// for relayed bootstrap) - see
/// `docs/plans/replay-protection-design.md` §10.
///
/// Deliberately **not** reset on reconnect: `ConnectionState`/`RelayedSession`
/// protect session keys that are fresh every reconnect, so resetting alongside
/// them is correct. The messages this type protects are encrypted wholly or
/// partly under the *permanent* pre-shared key pair, which never changes across
/// reconnects - a window that reset per connection would accept nonce 0 all
/// over again on every replay attempt against a fresh connection, which is as
/// good as no window at all. Callers keep one of these per peer for as long as
/// the process runs, in a registry separate from the per-connection/per-session
/// state (see `connection.rs`'s and `relay.rs`'s own per-peer maps).
///
/// # One window per sender incarnation
///
/// The receive side keeps a *bounded, most-recently-used-first list of windows
/// keyed by the sender's epoch*, not a single window. That is what reconciles
/// three requirements that a single window cannot satisfy together:
///
/// - **A restart must not wedge the link.** The outgoing counter lives in
///   memory (agents deliberately keep no on-disk state), so it returns to zero
///   when a process restarts, while the peer's window remembered where it had
///   got to - and rejected every reconnect as a replay until the counter
///   climbed back past the old high-water mark, at 5 s per attempt. A new
///   epoch gets a fresh window, so the reconnect is accepted immediately.
/// - **A replay must still be rejected.** The old epoch's window is *retained*
///   rather than discarded, so a captured message replayed later lands on the
///   window that already recorded it. Discarding on epoch change - the obvious
///   reading of "reset the counter when the epoch changes" - would be *weaker*
///   than a single window, because an attacker could alternate a replay with
///   genuine traffic and have the window reset for them every time.
/// - **Client HA must keep working.** Several processes legitimately share one
///   peer identity, each with its own epoch; per-epoch windows let them coexist
///   instead of continually resetting each other.
///
/// A peer that sends no epoch at all (`None` - a pre-epoch build) gets its own
/// slot, and so behaves exactly as it did before this existed.
#[derive(Debug, Default)]
pub(crate) struct HandshakeNonceState {
    next_nonce: u64,
    /// One window per sender epoch, most recently used first. Bounded by
    /// `MAX_TRACKED_EPOCHS`; a linear scan is the right structure at that size.
    windows: Vec<(Option<u64>, ReplayWindow)>,
}

impl HandshakeNonceState {
    pub(crate) fn take_next_nonce(&mut self) -> u64 {
        let nonce = self.next_nonce;
        // 2^64 handshakes is unreachable; wrap explicitly rather than rely on
        // release-mode overflow behaviour differing from debug.
        self.next_nonce = self.next_nonce.wrapping_add(1);
        nonce
    }

    /// Check `nonce`, sent by the peer incarnation identified by `epoch`,
    /// against what that incarnation has sent before.
    pub(crate) fn check_replay(&mut self, epoch: Option<u64>, nonce: Option<u64>) -> bool {
        match self.windows.iter().position(|(e, _)| *e == epoch) {
            Some(index) => {
                // Known incarnation - check against its own window, and move it
                // to the front so the least recently used epoch is the one
                // evicted below.
                let mut entry = self.windows.remove(index);
                let accepted = entry.1.check_and_record_optional(nonce);
                self.windows.insert(0, entry);
                accepted
            }
            None => {
                // A previously unseen incarnation: a restart, a new HA replica,
                // or a peer we have not spoken to since our own start. It gets
                // a fresh window, so its first nonce is accepted whatever its
                // value - exactly as `ReplayWindow` already treats the first
                // nonce it ever sees.
                let mut window = ReplayWindow::new();
                let accepted = window.check_and_record_optional(nonce);

                self.windows.insert(0, (epoch, window));
                self.windows.truncate(MAX_TRACKED_EPOCHS);

                accepted
            }
        }
    }

    /// How many sender incarnations are currently tracked. Test/diagnostic
    /// helper - the eviction bound is a security-relevant property, so it is
    /// asserted directly.
    #[cfg(test)]
    pub(crate) fn tracked_epochs(&self) -> usize {
        self.windows.len()
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

    #[test]
    fn test_handshake_nonce_state_take_next_nonce_increments() {
        let mut state = HandshakeNonceState::default();
        assert_eq!(state.take_next_nonce(), 0);
        assert_eq!(state.take_next_nonce(), 1);
        assert_eq!(state.take_next_nonce(), 2);
    }

    #[test]
    fn test_handshake_nonce_state_rejects_duplicate() {
        let mut state = HandshakeNonceState::default();
        let epoch = Some(42);
        assert!(state.check_replay(epoch, Some(0)));
        assert!(state.check_replay(epoch, Some(1)));
        assert!(
            !state.check_replay(epoch, Some(0)),
            "duplicate must be rejected"
        );
    }

    #[test]
    fn test_handshake_nonce_state_none_always_accepted() {
        // a not-yet-upgraded peer's `Handshake`/`PeerDetails` has no
        // `nonce` field at all, so deserialises as `None` - there is
        // nothing to check it against, so it must always be accepted.
        let mut state = HandshakeNonceState::default();
        assert!(state.check_replay(None, None));
        assert!(state.check_replay(None, None));
        assert!(state.check_replay(None, None));
    }

    #[test]
    fn test_process_epoch_is_stable_and_not_the_fallback() {
        // The epoch must be constant for the life of the process (it identifies
        // *this* incarnation) and must come from the CSPRNG rather than the
        // degraded fallback.
        assert_eq!(process_epoch(), process_epoch());
        assert_ne!(
            process_epoch(),
            0,
            "epoch should not be the CSPRNG fallback"
        );
    }

    #[test]
    fn test_new_epoch_accepts_a_restarted_counter() {
        // Finding R10, the case that motivated all of this. A peer runs for a
        // while, so our window for it has a high water mark; the peer then
        // restarts, its in-memory counter returns to zero, and it reconnects.
        // Under a single shared window every one of those low nonces was
        // rejected as a replay until the counter climbed past the old mark, at
        // 5 s per attempt. A new epoch must be accepted immediately.
        let mut state = HandshakeNonceState::default();
        let before = Some(1);

        for nonce in 0..500u64 {
            assert!(state.check_replay(before, Some(nonce)));
        }

        let after_restart = Some(2);
        assert!(
            state.check_replay(after_restart, Some(0)),
            "a restarted peer's nonce 0 must be accepted under its new epoch"
        );
        assert!(state.check_replay(after_restart, Some(1)));
    }

    #[test]
    fn test_old_epoch_window_is_retained_so_replays_still_fail() {
        // The other half of R10, and the reason the old window is *retained*
        // rather than discarded on an epoch change: discarding would be weaker
        // than a single window, because an attacker could alternate a replay
        // with genuine traffic and have the window cleared for them each time.
        let mut state = HandshakeNonceState::default();
        let old = Some(1);
        let new = Some(2);

        assert!(state.check_replay(old, Some(10)));
        assert!(state.check_replay(new, Some(0)));

        // The captured message from the old incarnation must still be refused.
        assert!(
            !state.check_replay(old, Some(10)),
            "a replay under a superseded epoch must still be rejected"
        );

        // ...and repeatedly, however much genuine traffic is interleaved.
        for nonce in 1..50u64 {
            assert!(state.check_replay(new, Some(nonce)));
            assert!(
                !state.check_replay(old, Some(10)),
                "interleaving genuine traffic must not re-arm the replay"
            );
        }
    }

    #[test]
    fn test_concurrent_epochs_do_not_reset_each_other() {
        // Client HA: several physical processes present the same identity, each
        // with its own epoch (highavailability.md §2). They must coexist -
        // under a strictly-increasing epoch rule the lower-epoch replica would
        // be rejected as a replay and could never reach standby.
        let mut state = HandshakeNonceState::default();
        let replica_a = Some(100);
        let replica_b = Some(7); // deliberately *lower* than A

        for nonce in 0..200u64 {
            assert!(state.check_replay(replica_a, Some(nonce)));
            assert!(
                state.check_replay(replica_b, Some(nonce)),
                "a second HA replica must not be treated as a replay of the first"
            );
        }

        // Each still detects its own duplicates.
        assert!(!state.check_replay(replica_a, Some(5)));
        assert!(!state.check_replay(replica_b, Some(5)));
    }

    #[test]
    fn test_tracked_epochs_are_bounded_and_evict_least_recently_used() {
        let mut state = HandshakeNonceState::default();

        for epoch in 0..(MAX_TRACKED_EPOCHS as u64) * 3 {
            assert!(state.check_replay(Some(epoch), Some(0)));
            assert!(
                state.tracked_epochs() <= MAX_TRACKED_EPOCHS,
                "tracked epochs must stay bounded (was {})",
                state.tracked_epochs()
            );
        }

        assert_eq!(state.tracked_epochs(), MAX_TRACKED_EPOCHS);

        // The most recent epochs are still remembered...
        let newest = (MAX_TRACKED_EPOCHS as u64) * 3 - 1;
        assert!(!state.check_replay(Some(newest), Some(0)));

        // ...while a long-superseded one has been evicted, so its window is
        // fresh again. This is the accepted cost of the bound, and it takes
        // MAX_TRACKED_EPOCHS further incarnations to reach.
        assert!(state.check_replay(Some(0), Some(0)));
    }

    #[test]
    fn test_legacy_no_epoch_peer_keeps_its_own_window() {
        // A peer built before the epoch field sends `None`. It must get its own
        // slot, so it behaves exactly as it did before - and must not have its
        // window reset by an unrelated epoch-bearing peer sharing the state.
        let mut state = HandshakeNonceState::default();

        assert!(state.check_replay(None, Some(5)));
        assert!(state.check_replay(Some(1), Some(5)));
        assert!(
            !state.check_replay(None, Some(5)),
            "the legacy slot must still detect its own duplicates"
        );
    }
}
