# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

- **Visitors (public diagnostics page).** Two equally weighted groups: network engineers (peers, customers, NOC staff) checking reachability, latency, and routing toward an operator's network, often mid-incident; and prospective customers evaluating network quality (speed test, iperf endpoints, test IPs) before buying.
- **Administrators.** The operator's staff who publish Locations, enroll remote Agents, choose Offered methods, and set branding and execution limits. All Administrators are equal peers (ADR 0001).

## Product Purpose

A self-hosted network diagnostics console. Visitors run read-only diagnostics (ping, mtr, traceroute, BGP, and IPv6 counterparts) from operator-published Locations, streamed live, plus a browser-measured Speed test and copy-paste iperf3 commands. Administrators manage those Locations and the site. Success: a visitor gets trustworthy raw output from the right Location within seconds, and an operator can stand up and brand a deployment without touching code.

## Positioning

One central container plus outbound-tunnel Agents; every Node runs only fixed, capability-scoped commands. No third-party speed-test module, no curl/whois, no managed iperf3 server (ADR 0002). Output is the real tool output, not a summary.

## Operating Context

- Deployed by the operator behind their own TLS proxy under their own domain; visitors arrive from the operator's website, status page, or peering documentation.
- Engineers compare output across Locations and copy it into tickets or chats.
- Prospects look at Location list, ASN, facility, test IPs, and throughput.

## Capabilities and Constraints

- Terminology follows `CONTEXT.md` (Location, Node, Agent, Enrollment, Revoke, Method, Offered method, Run, Test IP, iperf endpoint, Test file, Speed test, Administrator, Pending administrator, Activation link, Global settings).
- White-label: operators set site title, logo URL, terms-of-service URL, a custom content block, and a default theme (light, dark, or follow system). Visitors may toggle theme; their choice persists.
- Both light and dark themes are required.
- Frontend: SvelteKit SPA (Svelte 5), Panda CSS + Ark UI, embedded into the Rust `central` binary. Existing unit and Playwright e2e tests pin accessible labels and roles; functionality must not change during visual work.

## Brand Commitments

- Product name: Looking Glass. The UI carries its own identity but stays brand-tolerant: any operator logo and title must sit well on it.

## Evidence on Hand

- Hermetic fixture API with sample Locations (Frankfurt, Vienna, London, New York, Singapore, San Francisco Hub) in `frontend/tests/fixture-server.mjs`; sample data only, not real deployments.
- No customers, testimonials, benchmarks, or throughput claims exist; never fabricate them.

## Product Principles

1. Raw output is the product: never hide, summarize away, or decorate the real tool output.
2. Speed under stress: an engineer mid-incident reaches a running diagnostic in the fewest possible steps.
3. The operator's network is the subject, Looking Glass is the instrument: identity lives in precision, not in competing with the operator's brand.
4. Read-only and fail-closed: the UI never implies capabilities the Node does not offer.

## Accessibility & Inclusion

Keyboard operable and screen-reader labelled throughout (existing e2e tests rely on accessible names and roles); respect `prefers-reduced-motion`; readable contrast in both themes.
