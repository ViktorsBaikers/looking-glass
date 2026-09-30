# Looking Glass

Self-hosted network diagnostics console: visitors run read-only network tests from operator-published locations; administrators manage those locations and the site.

## Language

### Locations and nodes

**Location**:
A published point of presence visitors can run diagnostics from, identified by a display name, geographic label, and optional ASN.
_Avoid_: Site, PoP, server

**Node**:
The machine that actually executes a Location's diagnostics — either the built-in **Local node** inside central or a **Remote node** running an Agent.
_Avoid_: Host, box

**Agent**:
The process on a Remote node that holds the outbound tunnel to central and executes runs.
_Avoid_: Client, daemon

**Enrollment**:
Binding an Agent to a Location via a short-lived install command.
_Avoid_: Registration, pairing

**Revoke**:
Detaching an enrolled Agent from its Location so it can no longer run diagnostics.
_Avoid_: Unenroll, kick

**Location status**:
One of Online, Offline, or Not enrolled (a remote Location with no Agent yet).
_Avoid_: Health, state

**ASN**:
The autonomous system number a Location announces from, shown as `AS<n>`.

### Diagnostics

**Method**:
A diagnostic a Location can run (`ping`, `mtr`, `traceroute`, `bgp`, and their IPv6 counterparts).
_Avoid_: Tool, command, test

**Offered method**:
A Method an administrator has enabled at a Location; only offered methods can run there.
_Avoid_: Allowed method, capability

**Run**:
One execution of a Method against a target at a Location, streamed live to the visitor.
_Avoid_: Job, test, query

**Test IP**:
An address published by a Location for visitors to test toward.

**iperf endpoint**:
A published iperf3 server with copy-paste client commands; display-only, never executed by Looking Glass.

**Test file**:
A file served directly from a Node for download-throughput testing.
_Avoid_: Speedtest file, blob

**Speed test**:
A browser-measured throughput test against a Location's Node using its Test files.
_Avoid_: Speedtest module, Ookla

### Administration

**Administrator**:
A person who can sign in and manage the site; all administrators are equal peers with no roles.
_Avoid_: Admin user, owner, superuser

**Pending administrator**:
An Administrator created by a peer who has not yet set a password through their Activation link.

**Activation link**:
A one-time URL that lets a Pending administrator choose their password; shown once to the creating peer.
_Avoid_: Invite, magic link

**Global settings**:
Site-wide branding, default theme, and execution limits applied to every Run.
_Avoid_: Config, preferences
