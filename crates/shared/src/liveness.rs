//! The heartbeat liveness window, shared by the agent (which beats) and central
//! (which derives online/offline). One definition of the interval, the window, and
//! the [`is_online`] predicate so the beat rate and the offline window can never
//! drift apart.
//!
//! Compute-on-read (decisions.md "Slice 8b liveness design"): central persists only
//! each agent's `last_seen` timestamp; whether a location is online is *derived* at
//! every point status is read (the public selector, the admin list), never stored.
//! That makes liveness restart-safe — it re-derives from the persisted timestamp —
//! and revoke-safe: a revoked agent can never compute back online, however fresh its
//! last beat.

use std::time::Duration;

/// How often an agent sends a `Heartbeat` frame up the authenticated tunnel.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// How long central tolerates silence before a location is derived offline. Three
/// missed beats at [`HEARTBEAT_INTERVAL`], so one dropped heartbeat does not flap a
/// healthy agent, yet a genuinely down agent reflects within the window.
pub const OFFLINE_AFTER: Duration = Duration::from_secs(30);

/// Whether an agent is online *right now*, derived from its persisted `last_seen`
/// (unix seconds, the store's `unix_now`). Online iff it has beaten within
/// [`OFFLINE_AFTER`] **and** is not revoked. The two guards are independent:
///
/// - a revoked agent is never online, however recent its beat — the resurrection-hole
///   guard (drift.md: last-seen must not resurrect a revoked agent);
/// - an agent that has never beaten (`None`) is offline.
///
/// The window is a half-open bound: silence strictly under [`OFFLINE_AFTER`] seconds
/// is online, exactly at or past it is offline. `now` earlier than `last_seen` (clock
/// skew) saturates to zero elapsed and reads online.
pub fn is_online(last_seen: Option<u64>, now: u64, revoked: bool) -> bool {
    if revoked {
        return false;
    }
    match last_seen {
        Some(seen) => now.saturating_sub(seen) < OFFLINE_AFTER.as_secs(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected outcomes are reasoned from the spec's 30s window, not read back from
    // OFFLINE_AFTER, so a wrong constant would fail these rather than move with them.
    const BEAT: u64 = 1_000_000; // an arbitrary "last beat" instant, unix seconds.

    #[test]
    fn online_at_the_moment_of_the_beat() {
        assert!(is_online(Some(BEAT), BEAT, false));
    }

    #[test]
    fn online_just_inside_the_window() {
        // 29s of silence: within the 30s window, still online.
        assert!(is_online(Some(BEAT), BEAT + 29, false));
    }

    #[test]
    fn offline_at_the_window_boundary() {
        // Exactly 30s of silence flips offline (half-open bound).
        assert!(!is_online(Some(BEAT), BEAT + 30, false));
    }

    #[test]
    fn offline_past_the_window() {
        // 31s of silence: past the 30s window, offline.
        assert!(!is_online(Some(BEAT), BEAT + 31, false));
    }

    #[test]
    fn revoked_is_offline_however_recent_the_beat() {
        // A fresh beat must not resurrect a revoked agent as online.
        assert!(!is_online(Some(BEAT), BEAT, true));
        assert!(!is_online(Some(BEAT), BEAT + 1, true));
    }

    #[test]
    fn never_seen_is_offline() {
        assert!(!is_online(None, BEAT, false));
        assert!(!is_online(None, BEAT, true));
    }

    #[test]
    fn clock_skew_reads_online_not_underflow() {
        // now before last_seen must not panic on subtraction; it reads as online.
        assert!(is_online(Some(BEAT), BEAT - 5, false));
    }

    #[test]
    fn the_window_is_three_missed_beats() {
        // Documents the locked design: the offline window is three heartbeat
        // intervals, so a single dropped beat never flaps a healthy agent.
        assert_eq!(OFFLINE_AFTER, HEARTBEAT_INTERVAL * 3);
    }
}
