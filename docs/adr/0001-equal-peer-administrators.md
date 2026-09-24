# Administrators are equal peers, onboarded by one-time activation links

Looking Glass originally had a single administrator created at install. The redesign adds multiple Administrators with no roles or permission tiers: any peer can create a Pending administrator (who sets their own password through a 24-hour, single-use Activation link) or remove another peer. We chose flat peers over role-based access because the site has one trust level (full control of locations and settings) and roles would add authorization surface with no real separation to enforce.

## Consequences

- Guard rails replace roles: a peer cannot remove themselves, the last active Administrator cannot be removed, and removal immediately deletes that peer's sessions.
- The creating peer never learns the new Administrator's password; the Activation link is shown once and not retrievable afterwards (regenerate instead).
