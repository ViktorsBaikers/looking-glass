// Formats a remote agent's last-heartbeat timestamp for the admin locations list
// (Slice 8b). The central API returns `last_seen` as unix seconds; a location that
// has never enrolled/beaten reports it as null. Kept as a pure function so the
// relative-time edges (never-seen, just-now, minutes/hours/days) are unit-testable
// without a DOM.

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** A short human relative-time label for a last-seen timestamp.
 *  @param lastSeen unix seconds of the last heartbeat, or null if never seen.
 *  @param nowMs the current time in ms (injected so the result is deterministic). */
export function formatLastSeen(lastSeen: number | null | undefined, nowMs: number): string {
	if (lastSeen == null) return 'Never';
	const elapsed = Math.floor(nowMs / 1000) - lastSeen;
	if (elapsed < 0) return 'Just now'; // clock skew: treat a future stamp as current
	if (elapsed < MINUTE) return 'Just now';
	if (elapsed < HOUR) return `${Math.floor(elapsed / MINUTE)}m ago`;
	if (elapsed < DAY) return `${Math.floor(elapsed / HOUR)}h ago`;
	return `${Math.floor(elapsed / DAY)}d ago`;
}
