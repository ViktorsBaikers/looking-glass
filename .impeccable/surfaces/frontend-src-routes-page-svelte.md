---
version: 1
slug: "frontend-src-routes-page-svelte"
primary_target: "frontend/src/routes/+page.svelte"
related_targets: ["frontend/src/routes/admin","frontend/src/routes/login","frontend/src/routes/install","frontend/src/routes/activate"]
---

## Scope

Whole SPA: public diagnostics (`/`), admin (locations, location editor tabs, administrators, settings), auth (install, login, activate). Light + dark. Mode: Operate. Functionality, accessible names and roles unchanged.

## Audience and task

Engineers mid-incident plus prospects evaluating the network. Task: pick a Location, run a Method against a target, read raw output. Compact Location selector (user decision). User rejects both terminal cosplay and SaaS gloss.

## Direction contract

THESIS: Diagnostics as a backbone map. Each Location is a line, each hop a station. Refuses navy-plus-neon terminal and the rounded SaaS card stack.

OWN-WORLD: Neutral map paper (light) or charcoal night map (dark). Ink chrome, hairline rules, square corners. Saturated line colours only on data: Location roundel bullets, route lines, stations. Overpass + Overpass Mono, tabular numerals. Status follows transit conventions: solid = Online, grey = Offline, dashed = Not enrolled. Primary buttons are solid ink.

STORY: The visitor sees which Locations run, picks one by its bullet, runs a Method, and watches the route draw hop by hop. They copy the raw output.

FIRST VIEWPORT: Title strip. A single row of bullet selectors. The command line (method, target, ink Run) beside the Location station card. Console below, full width.

FORM: Transit/backbone diagram, position 6 of 7, seed a02161ec.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Memorable moment

mtr/traceroute hops arrive as stations on the Location's coloured vertical line. A live marker sits on the newest hop, lossy hops get a warning ring, and unresolved hops get a dashed segment.
