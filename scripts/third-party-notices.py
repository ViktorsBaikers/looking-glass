#!/usr/bin/env python3
"""Regenerate THIRD_PARTY_NOTICES.md from local sources.

    python3 scripts/third-party-notices.py          # rewrite THIRD_PARTY_NOTICES.md
    python3 scripts/third-party-notices.py --check  # exit 1 if it is out of date

Needs `cargo` with the crate sources in CARGO_HOME (any cargo build or fetch of
this workspace puts them there), an installed `frontend/node_modules`
(`npm ci`), and the pinned Rust toolchain RUST_TOOLCHAIN with its `rust-src`
component plus the standard library's crates.io sources:

    rustup component add rust-src --toolchain 1.96
    RUSTC_BOOTSTRAP=1 cargo +1.96 fetch --locked --manifest-path \
        "$(rustc +1.96 --print sysroot)/lib/rustlib/src/rust/library/Cargo.toml"

(RUSTC_BOOTSTRAP lets stable cargo read the library manifest, which uses a
nightly cargo feature.) The script runs rustc and cargo with
RUSTUP_TOOLCHAIN=RUST_TOOLCHAIN and stops if the toolchain, rust-src or a
source is missing, or if the Dockerfile, ci.yml or release.yml pin another
toolchain. The output depends only on Cargo.lock, package-lock.json, the pinned
toolchain and those sources: no paths, dates or hosts, so two runs produce
identical bytes. A new patch release of the pinned toolchain changes the output.

What counts as shipped:
- Rust crates: every crate that `cargo tree -e normal,build` resolves for the
  `central` and `agent` binaries on each release target in TARGETS. That is the
  feature-resolved build graph, so crates that are locked only for other
  platforms (core-foundation, windows-sys) or reachable only through
  dev-dependency features (x509-parser) are left out. Build scripts and proc
  macros stay in: their generated code is compiled into the binaries.
- The Rust standard library, which is compiled into both binaries: the Rust
  project's copyright statement for it (the toolchain's
  share/doc/rust/COPYRIGHT-library.html, in-tree part), the license texts that
  statement names (share/doc/rust/licenses/), every license file under rust-src
  library/, and every crates.io package in rust-src library/Cargo.lock (all
  targets, a superset of what TARGETS link). Their license files come from the
  notices in COPYRIGHT-library.html; a package that page omits takes them from
  its source in CARGO_HOME. Every one of them is also scanned per file (below),
  so the script stops if any of their sources is missing.
- ring's C, assembly and PerlAsm sources carry their own copyright lines. The
  files its build.rs compiles for TARGETS (RING_SRCS, the pregenerated
  assembly, and the headers they include) are scanned and every line containing
  "Copyright" is listed with its file.
- npm packages: the SPA's runtime roots (package.json `dependencies` plus
  BUNDLED_DEV) and everything they reach through `dependencies`. This is a
  superset of what Vite keeps after tree-shaking. EMITTED lists packages whose
  code or data a build tool writes into the bundle; they are listed without
  their own dependencies.
- Fonts: the font files each @fontsource package's index.css references.

License text comes from the files each package ships (LICENSE*, COPYING*,
NOTICE*, COPYRIGHT*, UNLICENSE*) anywhere in the package, such as vendored code
under third_party/ (nested node_modules excepted), copied verbatim with every
copyright line. A package that ships none borrows the files of a package in the
same set that has the same repository and declared license; failing that, it is
listed under "Packages without a license file" with its declared license and
authors.

Per-file notices: every file of every package above whose source is on disk
(each Rust crate, the rust-src library/ tree, the standard-library packages
from CARGO_HOME, and each npm package; nested node_modules excepted) is read,
not only the files a build compiles or bundles, so the result is a superset.
Files that are not UTF-8 text or contain a NUL byte are skipped as binary, and
license files because they are reproduced whole. Every line matching NOTICE
(copyright, ©, "(c) <digit>", all rights reserved, public domain, licensed
under, SPDX license identifiers, permission-notice wording, "Author:", "Written
by <name> <email>", in any case) is recorded verbatim with its context: the
whole comment block when the line is in one (a run of lines starting with a
comment marker, or inside a /* */ or <!-- --> comment), otherwise its paragraph
(the run of non-blank lines around it, at most PARAGRAPH lines on each side, so
data files stay bounded). So a license header written out in a file (a BSD or
ISC permission notice, musl's notice with its authors) and an attribution
paragraph in a README are kept in full. A line longer than LONG_LINE characters
(minified code, source maps) is cut to WINDOW characters on each side of the
match. Blocks are deduplicated per package and listed with the files they
appear in.

Crate sources are checked before they are read, so a damaged CARGO_HOME cannot
silently drop a notice: each crate's .crate archive in CARGO_HOME/registry/cache
must match the checksum in Cargo.lock (library/Cargo.lock for the standard
library's packages), and the extracted source must hold exactly the archive's
files with the same bytes (cargo's .cargo-ok marker aside). Otherwise the
script stops and names the crate. npm packages and rust-src are read as
installed.
"""

import hashlib
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import tomllib
from html import unescape
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FRONTEND = ROOT / "frontend"
OUTPUT = ROOT / "THIRD_PARTY_NOTICES.md"

# The central image is built for linux/amd64 and the agent release asset is
# lg-agent-x86_64-unknown-linux-gnu (see .github/workflows/release.yml).
TARGETS = ["x86_64-unknown-linux-gnu"]
BINARIES = ["central", "agent"]
# The toolchain the Dockerfile, ci.yml and release.yml build with. Its standard
# library is what the release binaries contain.
RUST_TOOLCHAIN = "1.96"
TOOLCHAIN_PINS = {
    "Dockerfile": f"FROM rust:{RUST_TOOLCHAIN}-",
    ".github/workflows/ci.yml": f"toolchain: '{RUST_TOOLCHAIN}'",
    ".github/workflows/release.yml": f"rust:{RUST_TOOLCHAIN}-",
}
# devDependencies whose runtime code Vite bundles into the SPA.
BUNDLED_DEV = ["svelte", "@sveltejs/kit"]
# Code or data a build tool writes into the bundle: Panda's generated css
# runtime (styled-system/), the Material Symbols icon data and the component
# code unplugin-icons compiles around it, Vite's module preload helper
# (__vite__mapDeps, vite:preloadError) and Rollup's namespace objects
# (Symbol.toStringTag "Module").
EMITTED = ["@pandacss/generator", "@pandacss/shared", "@iconify-json/material-symbols",
           "unplugin-icons", "vite", "rollup"]

LICENSE_FILE = re.compile(r"(?i)^(licen[cs]e|copying|notice|copyright|unlicense)")
NOTICE = re.compile(r"(?i)copyright|©|\(c\)\s*\d|all rights reserved|public domain|licensed under|"
                    r"license, version|spdx-license-identifier|permission notice|permission is hereby granted|"
                    r"permission to use, copy|redistribution and use|\bauthors?:|"
                    r"\bwritten by [^<\n]*<[^<>@\s]+@[^<>\s]+>")
COMMENT = re.compile(r"\s*(//|/\*|\*|#|;|--|%|@|<!--)")
LONG_LINE = 1000
WINDOW = 300
PARAGRAPH = 10
ENV = {**os.environ, "RUSTUP_TOOLCHAIN": RUST_TOOLCHAIN}


def run(*args, cwd=ROOT):
    return subprocess.run(args, cwd=cwd, check=True, stdout=subprocess.PIPE, text=True, env=ENV).stdout


def normalize(text):
    return text.replace("\r\n", "\n").replace("\r", "\n").rstrip() + "\n"


def license_files(directory):
    files = []
    for parent, dirs, names in os.walk(directory):
        dirs[:] = sorted(d for d in dirs if d != "node_modules")
        for name in sorted(names):
            path = Path(parent, name)
            if path.is_file() and LICENSE_FILE.match(name):
                text = path.read_bytes().decode("utf-8", errors="replace")
                files.append((path.relative_to(directory).as_posix(), normalize(text)))
    return files


def source_notices(directory, label):
    if not Path(directory).is_dir():
        raise SystemExit(f"third-party-notices: no source directory for {label}: {directory}")
    blocks = {}
    for parent, dirs, names in os.walk(directory):
        dirs[:] = sorted(d for d in dirs if d != "node_modules")
        for name in sorted(names):
            path = Path(parent, name)
            if LICENSE_FILE.match(name) or not path.is_file():
                continue  # license files are reproduced whole by license_files
            data = path.read_bytes()
            try:
                text = data.decode("utf-8")
            except UnicodeDecodeError:
                continue  # binary
            if "\0" in text:
                continue
            lines = text.replace("\r\n", "\n").replace("\r", "\n").split("\n")
            comment, closing = [], None
            for line in lines:
                if closing:
                    comment.append(True)
                    if closing in line:
                        closing = None
                    continue
                comment.append(bool(COMMENT.match(line)))
                stripped = line.lstrip()
                for start, end in (("/*", "*/"), ("<!--", "-->")):
                    if stripped.startswith(start) and end not in stripped[len(start):]:
                        closing = end
            prose = [bool(line.strip()) and len(line) <= LONG_LINE for line in lines]
            rel = path.relative_to(directory).as_posix()
            for i, line in enumerate(lines):
                if not NOTICE.search(line):
                    continue
                if len(line) > LONG_LINE:
                    found = [line[max(0, m.start() - WINDOW): m.end() + WINDOW] for m in NOTICE.finditer(line)]
                else:
                    # the comment block around a comment line, else the paragraph around the line
                    same = comment if comment[i] else prose
                    reach = len(lines) if comment[i] else PARAGRAPH
                    first = last = i
                    while first > 0 and i - first < reach and same[first - 1]:
                        first -= 1
                    while last + 1 < len(lines) and last - i < reach and same[last + 1]:
                        last += 1
                    found = ["\n".join(lines[first: last + 1])]
                for block in found:
                    files = blocks.setdefault(block, [])
                    if rel not in files:
                        files.append(rel)
    if not blocks:
        return []
    body = [f"Copyright and license notices in the source files of {label}, verbatim, "
            "each with the files it appears in:"]
    for block, files in blocks.items():
        body += ["", f"In {', '.join(files)}:", "", block]
    return [("notices in source files", normalize("\n".join(body)))]


def verify_crate(directory, checksum):
    # directory is CARGO_HOME/registry/src/<index>/<name>-<version>; cargo keeps
    # the downloaded archive at CARGO_HOME/registry/cache/<index>/<name>-<version>.crate.
    directory = Path(directory)
    crate = directory.parents[2] / "cache" / directory.parent.name / f"{directory.name}.crate"
    if not crate.is_file():
        raise SystemExit(f"third-party-notices: {crate} is missing; delete {directory} and run cargo fetch"
                         " --locked to download and extract it again")
    data = crate.read_bytes()
    if hashlib.sha256(data).hexdigest() != checksum:
        raise SystemExit(f"third-party-notices: {crate} does not match its Cargo.lock checksum; delete it and"
                         f" {directory}, then run cargo fetch --locked")
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        expected = {m.name.split("/", 1)[1]: archive.extractfile(m).read() for m in archive if m.isfile()}
    actual = {p.relative_to(directory).as_posix() for p in directory.rglob("*") if p.is_file()} - {".cargo-ok"}
    if actual != expected.keys() or any((directory / name).read_bytes() != body for name, body in expected.items()):
        raise SystemExit(f"third-party-notices: {directory} does not hold exactly the files of {crate.name};"
                         " delete it and run cargo fetch --locked to extract it again")


def package(kind, name, version, license, repository, authors, directory):
    return {
        "kind": kind, "name": name, "version": version, "license": license or "unspecified",
        "repository": repository or "", "authors": authors, "files": license_files(directory),
        "notices": source_notices(directory, f"{name} {version}"), "directory": directory, "via": None,
    }


def crates():
    platforms = [arg for target in TARGETS for arg in ("--filter-platform", target)]
    metadata = json.loads(run("cargo", "metadata", "--locked", "--format-version", "1", *platforms))
    registry = {(p["name"], p["version"]): p for p in metadata["packages"] if p["source"]}
    checksums = {(p["name"], p["version"]): p["checksum"]
                 for p in tomllib.loads((ROOT / "Cargo.lock").read_text())["package"] if "checksum" in p}
    wanted = set()
    for target in TARGETS:
        args = ["cargo", "tree", "--locked", "-e", "normal,build", "--target", target,
                "--prefix", "none", "-f", "{p}"]
        for binary in BINARIES:
            args += ["-p", binary]
        for line in run(*args).splitlines():
            parts = line.split()
            if len(parts) >= 2 and (parts[0], parts[1].lstrip("v")) in registry:
                wanted.add((parts[0], parts[1].lstrip("v")))
    result = []
    for key in sorted(wanted):
        p = registry[key]
        verify_crate(os.path.dirname(p["manifest_path"]), checksums[key])
        result.append(package("crate", p["name"], p["version"], p["license"], p["repository"],
                              p["authors"], os.path.dirname(p["manifest_path"])))
        if p["name"] == "ring":
            result[-1]["files"].append(ring_source_notices(result[-1]))
    return result


def ring_source_notices(ring):
    directory = Path(ring["directory"])
    build = (directory / "build.rs").read_text()
    consts = dict(re.findall(r'const (\w+): &str = "([^"]*)";', build))
    table = build[build.index("const RING_SRCS"):]
    table = table[: table.index("\n];")]
    files = set()
    for target in TARGETS:
        arch = {name for name, value in consts.items() if value == target.split("-")[0]}
        formats = [f for a, f in re.findall(r'oss: LINUX_ABI,\s*arch: (\w+),\s*perlasm_format: "(\w+)"', build)
                   if a in arch]
        if "-linux-" not in target or not formats:
            raise SystemExit(f"third-party-notices: no ring assembly format for {target} in build.rs")
        sources = [consts.get(src, src.strip('"')) for archs, src in
                   re.findall(r'\(&\[([^\]]*)\],\s*("[^"]*"|\w+)\)', table)
                   if not archs.strip() or arch & {a.strip() for a in archs.split(",")}]
        # build.rs assembles each PerlAsm script from pregenerated/<stem>-<format>.S,
        # and runs sha512-x86_64.pl a second time as sha256-x86_64.
        perlasm = [src for src in sources if src.endswith(".pl")]
        for concrete, synthesized in re.findall(r"maybe_synthesize\((\w+), (\w+)\)", build):
            if consts[concrete] in perlasm:
                perlasm.append(consts[synthesized])
        files.update(sources)
        files.update(f"pregenerated/{Path(src).stem}-{formats[0]}.S" for src in perlasm)
    pending = sorted(files)
    while pending:
        source = pending.pop()
        path = directory / source
        if not path.is_file():
            raise SystemExit(f"third-party-notices: ring {ring['version']} has no {source}")
        for include in re.findall(r'^\s*#\s*include\s*[<"]([^>"]+)[>"]', path.read_text(), re.M):
            for candidate in (os.path.normpath(Path(source).parent / include), f"include/{include}"):
                if (directory / candidate).is_file():
                    if candidate not in files:
                        files.add(candidate)
                        pending.append(candidate)
                    break
    lines = []
    for source in sorted(files):
        for line in (directory / source).read_text().splitlines():
            if "Copyright" in line:
                notice = re.sub(r"^\s*(/\*+|\*+|//+|#+|;+)?\s*|\s*\*/\s*$", "", line)
                lines.append(f"{source}: {notice}")
    header = (f"Copyright lines of the {len(files)} C, assembly and PerlAsm source files that "
              f"ring {ring['version']} compiles for {', '.join(TARGETS)} (build.rs RING_SRCS, "
              "the pregenerated assembly and the headers they include), with the file each "
              "appears in:")
    return ("copyright lines of compiled sources", header + "\n\n" + "\n".join(lines) + "\n")


def standard_library():
    release = re.search(r"^release: (\S+)$", run("rustc", "-vV"), re.M).group(1)
    if not release.startswith(RUST_TOOLCHAIN + "."):
        raise SystemExit(f"third-party-notices: rustc {release} is not the pinned toolchain {RUST_TOOLCHAIN}"
                         f" (rustup toolchain install {RUST_TOOLCHAIN})")
    for path, pin in TOOLCHAIN_PINS.items():
        if pin not in (ROOT / path).read_text():
            raise SystemExit(f"third-party-notices: {path} does not pin {pin!r}; update RUST_TOOLCHAIN")
    sysroot = Path(run("rustc", "--print", "sysroot").strip())
    docs = sysroot / "share/doc/rust"
    library = sysroot / "lib/rustlib/src/rust/library"
    if not (library / "Cargo.lock").is_file():
        raise SystemExit(f"third-party-notices: rust-src is missing for Rust {release}"
                         f" (rustup component add rust-src --toolchain {RUST_TOOLCHAIN})")
    page = (docs / "COPYRIGHT-library.html").read_text()
    head, _, dependencies = page.partition('<h2 id="out-of-tree-dependencies">')
    statement = unescape(re.sub(r"<[^>]+>", "", head[head.index("<h1>"):]))
    statement = re.sub(r"\n{3,}", "\n\n", "\n".join(line.strip() for line in statement.splitlines()))
    files = [("COPYRIGHT-library.html", normalize(statement.strip()))]
    expressions = " ".join(re.findall(r"^License: (.+)$", statement, re.M))
    for spdx in sorted(license_ids(expressions)):
        files.append((f"licenses/{spdx}.txt", normalize((docs / "licenses" / f"{spdx}.txt").read_text())))
    files += [(f"library/{name}", text) for name, text in license_files(library)]
    result = [{
        "kind": "std", "name": "Rust standard library (std, core, alloc and their in-tree crates)",
        "version": release, "license": "Apache-2.0 OR MIT, with the exceptions in COPYRIGHT-library.html",
        "repository": "https://github.com/rust-lang/rust", "authors": [], "files": files,
        "notices": source_notices(library, f"the Rust {release} standard library (rust-src library/)"),
        "directory": library, "via": None,
    }]

    notices = {}
    for block in dependencies.split("<h3>📦 ")[1:]:
        texts = re.findall(r"<summary><code>([^<]+)</code></summary>\s*<pre>(.*?)</pre>", block, re.S)
        notices[block[: block.index("</h3>")]] = {
            "license": unescape(re.search(r"<b>License:</b> ([^<]*)</p>", block).group(1)),
            "authors": [unescape(re.search(r"<b>Authors:</b> ([^<]*)</p>", block).group(1))],
            "files": [(name, normalize(unescape(body).removeprefix("\n"))) for name, body in texts],
        }
    cargo_home = Path(os.environ.get("CARGO_HOME") or Path.home() / ".cargo")
    lock = tomllib.loads((library / "Cargo.lock").read_text())
    for entry in lock["package"]:
        if not entry.get("source", "").startswith("registry+"):
            continue
        name, version = entry["name"], entry["version"]
        repository = f"https://crates.io/crates/{name}/{version}"
        sources = sorted((cargo_home / "registry" / "src").glob(f"*/{name}-{version}/Cargo.toml"))
        if not sources:
            raise SystemExit(f"third-party-notices: {name} {version} (Rust {release} library/Cargo.lock) is not"
                             f" in {cargo_home}/registry/src; run: RUSTC_BOOTSTRAP=1 cargo +{RUST_TOOLCHAIN}"
                             f" fetch --locked --manifest-path {library}/Cargo.toml")
        verify_crate(sources[0].parent, entry["checksum"])
        manifest = tomllib.loads(sources[0].read_text())["package"]
        result.append(package("std-crate", name, version, manifest.get("license"), repository,
                              manifest.get("authors", []), sources[0].parent))
        if f"{name}-{version}" in notices:
            result[-1].update(notices[f"{name}-{version}"])
    return result


def npm_resolve(name, parent):
    directory = parent
    while True:
        candidate = directory / "node_modules" / name
        if candidate.is_dir():
            return candidate
        if directory == FRONTEND:
            raise SystemExit(f"third-party-notices: {name} is not installed (run npm ci in frontend/)")
        text = str(directory)
        directory = Path(text[: text.rindex("/node_modules/")])


def npm_packages():
    manifest = json.loads((FRONTEND / "package.json").read_text())
    pending = [(name, FRONTEND, True) for name in sorted(manifest["dependencies"]) + BUNDLED_DEV]
    pending += [(name, FRONTEND, False) for name in EMITTED]
    seen = {}
    while pending:
        name, parent, follow = pending.pop()
        directory = npm_resolve(name, parent)
        if directory in seen:
            continue
        data = json.loads((directory / "package.json").read_text())
        seen[directory] = data
        if follow:
            pending += [(dep, directory, True) for dep in sorted(data.get("dependencies", {}))]
    result = []
    for directory, data in seen.items():
        repository = data.get("repository")
        if isinstance(repository, dict):
            repository = repository.get("url")
        author = data.get("author")
        if not author and (directory / "info.json").is_file():
            # Iconify icon sets name the icons' author in info.json.
            author = json.loads((directory / "info.json").read_text()).get("author")
        if isinstance(author, dict):
            author = " ".join(str(author[k]) for k in ("name", "email", "url") if author.get(k))
        license = data.get("license")
        if not isinstance(license, str):
            license = json.dumps(license, sort_keys=True) if license else None
        result.append(package("npm", data["name"], data["version"], license, repository,
                              [author] if author else [], directory))
    return sorted(result, key=lambda p: (p["name"], p["version"]))


def fonts(npm):
    result = []
    for p in npm:
        if not p["name"].startswith("@fontsource"):
            continue
        css = Path(p["directory"], "index.css").read_text()
        for file in sorted(set(re.findall(r"url\(\./files/([^)]+)\)", css))):
            result.append((file, p))
    return result


def normalize_repository(url):
    url = re.sub(r"^(git\+|git://)", "", url or "").removesuffix(".git").rstrip("/")
    return re.sub(r"^(https?://|ssh://git@|git@)", "", url).replace("github.com:", "github.com/")


def borrow_license_files(packages):
    for p in packages:
        if p["files"] or not p["repository"]:
            continue
        for donor in packages:
            if (donor["files"] and donor["license"] == p["license"]
                    and normalize_repository(donor["repository"]) == normalize_repository(p["repository"])):
                p["files"], p["via"] = donor["files"], donor
                break


def fence(text):
    longest = max((len(m) for m in re.findall(r"`+", text)), default=0)
    return "`" * max(3, longest + 1)


def license_ids(expression):
    return set(re.findall(r"[\w.+-]+", expression)) - {"AND", "OR", "WITH"}


def cell(value):
    return str(value).replace("|", "\\|")


def render(rust, std, npm, font_files):
    texts = {}
    for p in rust + std + npm:
        p["texts"] = []
        for name, text in p["files"] + p["notices"]:
            number = texts.setdefault(text, len(texts) + 1)
            p["texts"].append(number)
    users = {}
    for p in rust + std + npm:
        for (name, _), number in zip(p["files"] + p["notices"], p["texts"]):
            users.setdefault(number, []).append(f"{p['name']} {p['version']} ({name})")

    out = [
        "# Third-party notices",
        "",
        "Looking Glass is licensed under the MIT License (see `LICENSE`). Its release",
        "artefacts (the central container image and binary, the agent binary, and the web",
        "interface central serves) also contain the third-party software listed below.",
        "",
        "Each package's license files are reproduced verbatim, every copyright line",
        "included, followed by the copyright and license notices found in its source",
        "files. Packages whose texts are byte-for-byte identical share one copy under",
        "\"License texts\"; the tables give the text numbers for each package.",
        "",
        "Generated by `scripts/third-party-notices.py`. Do not edit by hand: run",
        "`python3 scripts/third-party-notices.py` after changing `Cargo.lock`,",
        "`frontend/package-lock.json` or the pinned Rust toolchain. The script documents",
        "what counts as shipped.",
        "",
        f"## Rust crates ({len(rust)})",
        "",
        f"Compiled into `central` and `agent` for {', '.join(TARGETS)}.",
        "",
        "| Crate | Version | License | Texts |",
        "| --- | --- | --- | --- |",
    ]
    for p in rust + ["std"] + std + [None] + npm:
        if p == "std":
            out += [
                "",
                f"## Rust standard library ({len(std)})",
                "",
                f"`std`, `core` and `alloc` from Rust {std[0]['version']} are compiled into `central` and `agent`.",
                "The first row is the library itself: the Rust project's copyright statement for it",
                "(`COPYRIGHT-library.html` in the toolchain), the license texts that statement names,",
                "the license files in `rust-src` `library/` and the notices in its source files. The",
                "other rows are the crates.io packages in the library's `Cargo.lock`, for every target",
                f"(a superset of what {', '.join(TARGETS)} links).",
                "",
                "| Component | Version | License | Texts |",
                "| --- | --- | --- | --- |",
            ]
            continue
        if p is None:
            out += [
                "",
                f"## npm packages ({len(npm)})",
                "",
                "Bundled into the web interface that central embeds and serves.",
                "",
                "| Package | Version | License | Texts |",
                "| --- | --- | --- | --- |",
            ]
            continue
        numbers = ", ".join(str(n) for n in p["texts"]) or "none"
        out.append(f"| {cell(p['name'])} | {cell(p['version'])} | {cell(p['license'])} | {numbers} |")

    out += ["", f"## Fonts ({len(font_files)})", "", "Served with the web interface.", "",
            "| Font file | Package | License | Texts |", "| --- | --- | --- | --- |"]
    for file, p in font_files:
        numbers = ", ".join(str(n) for n in p["texts"])
        out.append(f"| {cell(file)} | {cell(p['name'])} {cell(p['version'])} | {cell(p['license'])} | {numbers} |")

    borrowed = [p for p in rust + std + npm if p["via"]]
    missing = [p for p in rust + std + npm if not p["files"]]
    out += ["", "## Packages without a license file", ""]
    if borrowed:
        out += ["These packages ship no license file. Each uses the license files of a package",
                "published from the same repository under the same license:", ""]
        out += [f"- {p['name']} {p['version']}: files of {p['via']['name']} {p['via']['version']}"
                f" ({p['repository']})" for p in borrowed]
        out.append("")
    if missing:
        shipped = {spdx for p in rust + std + npm if p["files"] for spdx in license_ids(p["license"])}
        out += ["These packages ship no license file. Their declared license and authors are",
                "given as published. A license named here is reproduced in full under",
                "\"License texts\" when another package that declares it ships its text;",
                "an entry says so when no package does.", ""]
        for p in missing:
            authors = "; ".join(p["authors"]) or "not stated"
            absent = sorted(license_ids(p["license"]) - shipped)
            note = (f" No package listed here ships the {', '.join(absent)} text, so it is not reproduced."
                    if absent else "")
            out.append(f"- {p['name']} {p['version']}: {p['license']}. Authors: {authors.removesuffix('.')}."
                       f" Repository: {p['repository'] or 'not stated'}.{note}")
        out.append("")
    if not borrowed and not missing:
        out += ["None.", ""]

    out += ["## License texts", ""]
    for text, number in texts.items():
        marker = fence(text)
        out += [f"### Text {number}", "", "Used by: " + ", ".join(users[number]) + ".", "",
                marker + "text", text.rstrip("\n"), marker, ""]
    return "\n".join(out).rstrip("\n") + "\n"


def main():
    check = sys.argv[1:] == ["--check"]
    if sys.argv[1:] not in ([], ["--check"]):
        raise SystemExit("usage: third-party-notices.py [--check]")
    rust, std, npm = crates(), standard_library(), npm_packages()
    borrow_license_files(rust)
    borrow_license_files(npm)
    notices = render(rust, std, npm, fonts(npm))
    if check:
        if not OUTPUT.exists() or OUTPUT.read_text() != notices:
            print("THIRD_PARTY_NOTICES.md is out of date: run python3 scripts/third-party-notices.py",
                  file=sys.stderr)
            sys.exit(1)
        print("third-party notices up to date")
        return
    OUTPUT.write_text(notices)
    print(f"wrote THIRD_PARTY_NOTICES.md: {len(rust)} crates, {len(std)} standard library rows,"
          f" {len(npm)} npm packages")


if __name__ == "__main__":
    main()
