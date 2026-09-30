#!/usr/bin/env sh
set -eu

workflow=${1:-.github/workflows/release.yml}
ci=${2:-.github/workflows/ci.yml}

python3 - "$workflow" "$ci" <<'PY'
import copy
import re
import sys
from pathlib import Path


def scalar(value):
    if value == "{}":
        return {}
    return value


def indent(line):
    return len(line) - len(line.lstrip(" "))


def parse(lines, level=0):
    while lines and not lines[0].strip():
        lines.pop(0)
    if not lines:
        return {}
    if indent(lines[0]) != level:
        raise ValueError("unexpected indentation")
    if lines[0][level:].startswith("- "):
        values = []
        while lines:
            if not lines[0].strip():
                lines.pop(0)
                continue
            if indent(lines[0]) != level:
                break
            if not lines[0][level:].startswith("- "):
                raise ValueError("mixed YAML collection")
            first = lines.pop(0)[level + 2:]
            if ":" not in first:
                if lines and lines[0].strip() and indent(lines[0]) > level:
                    raise ValueError("nested scalar list item")
                values.append(scalar(first))
                continue
            entry = [" " * (level + 2) + first]
            while lines and (not lines[0].strip() or indent(lines[0]) > level):
                entry.append(lines.pop(0))
            values.append(parse(entry, level + 2))
        return values

    values = {}
    while lines:
        if not lines[0].strip():
            lines.pop(0)
            continue
        if indent(lines[0]) != level:
            break
        line = lines.pop(0)[level:]
        if line.startswith("#") or ":" not in line:
            raise ValueError("unsupported YAML syntax")
        key, value = line.split(":", 1)
        if not key or key in values:
            raise ValueError("invalid or duplicate mapping key")
        value = value.lstrip(" ")
        if value == "|":
            body = []
            while lines and (not lines[0].strip() or indent(lines[0]) > level):
                body.append(lines.pop(0))
            if not body:
                raise ValueError("empty block scalar")
            minimum = min(indent(line) for line in body if line.strip())
            values[key] = "".join(line[minimum:] + "\n" for line in body).rstrip("\n") + "\n"
        elif value:
            values[key] = scalar(value)
        else:
            if not lines or indent(lines[0]) <= level:
                raise ValueError("empty mapping value")
            values[key] = parse(lines, indent(lines[0]))
    return values


def fail(message):
    raise ValueError(message)


def mapping(value, message):
    if not isinstance(value, dict):
        fail(message)
    return value


def steps(value, names):
    if not isinstance(value, list) or [step.get("name") for step in value] != names:
        fail("exact ordered steps")
    if any(not isinstance(step, dict) for step in value):
        fail("step mapping")
    return value


def expect(value, expected, message):
    if value != expected:
        fail(message)


def scalar_values(value):
    if isinstance(value, dict):
        for child in value.values():
            yield from scalar_values(child)
    elif isinstance(value, list):
        for child in value:
            yield from scalar_values(child)
    elif isinstance(value, str):
        yield value


def pinned(step, name, action, message):
    # Actions are pinned to a full commit SHA with the release tag as a comment.
    uses = step.get("uses", "")
    if step.get("name") != name or not re.fullmatch(re.escape(action) + r"@[0-9a-f]{40} # v[0-9]+(\.[0-9]+)*", uses):
        fail(message)
    return {key: value for key, value in step.items() if key not in {"name", "uses"}}


def publish_run():
    return verify_run() + (
        '/usr/bin/gh release create "$GITHUB_REF_NAME" --verify-tag --generate-notes'
        " assets/install-agent.sh assets/lg-agent-x86_64-unknown-linux-gnu THIRD_PARTY_NOTICES.md\n"
    )


def verify_run():
    return (
        "lg_installer_sha256=$(/usr/bin/grep -E '^LG_INSTALLER_SHA256=[0-9a-f]{64}$' README.md | /usr/bin/cut -d= -f2)\n"
        "lg_agent_sha256=$(/usr/bin/grep -E '^LG_AGENT_SHA256=[0-9a-f]{64}$' README.md | /usr/bin/cut -d= -f2)\n"
        "[ \"$(/usr/bin/printf '%s\\n' \"$lg_installer_sha256\" | /usr/bin/wc -l)\" -eq 1 ]\n"
        "[ \"$(/usr/bin/printf '%s\\n' \"$lg_agent_sha256\" | /usr/bin/wc -l)\" -eq 1 ]\n"
        "/usr/bin/printf '%s  %s\\n' \\\n"
        "  \"$lg_installer_sha256\" assets/install-agent.sh \\\n"
        "  \"$lg_agent_sha256\" assets/lg-agent-x86_64-unknown-linux-gnu \\\n"
        "  | /usr/bin/sha256sum --check --strict\n"
    )


def pins_run():
    # The verified pins, plus download URLs for this tag's assets, become the image's build args.
    return verify_run() + (
        'base="$GITHUB_SERVER_URL/$GITHUB_REPOSITORY/releases/download/$GITHUB_REF_NAME"\n'
        "/usr/bin/printf '%s\\n' \\\n"
        '  "agent_url=$base/lg-agent-x86_64-unknown-linux-gnu" \\\n'
        '  "agent_sha256=$lg_agent_sha256" \\\n'
        '  "installer_url=$base/install-agent.sh" \\\n'
        '  "installer_sha256=$lg_installer_sha256" \\\n'
        '  >> "$GITHUB_OUTPUT"\n'
    )


PIN_OUTPUTS = ("agent_url", "agent_sha256", "installer_url", "installer_sha256")


def validate(document):
    root = mapping(document, "workflow mapping")
    expect(set(root), {"name", "on", "permissions", "concurrency", "env", "jobs"}, "root allowlist")
    expect(root["on"], {
        "push": {"tags": ["'v*'"]},
        "workflow_dispatch": {"inputs": {"push": {
            "description": "Push the image to GHCR", "required": "true", "default": "false", "type": "boolean",
        }}},
    }, "release triggers")
    expect(root["permissions"], {}, "root permissions")
    expect(root["env"], {"IMAGE_NAME": "ghcr.io/${{ github.repository }}"}, "root environment")
    jobs = mapping(root["jobs"], "jobs mapping")
    expect(set(jobs), {"prepare-release-assets", "release-assets", "image"}, "exact jobs")

    prepare = mapping(jobs["prepare-release-assets"], "prepare mapping")
    expect(set(prepare), {"if", "runs-on", "outputs", "permissions", "defaults", "env", "steps"}, "prepare allowlist")
    expect(prepare["outputs"], {name: f"${{{{ steps.pins.outputs.{name} }}}}" for name in PIN_OUTPUTS}, "prepare pin outputs")
    expect(prepare["if"], "${{ github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v') }}", "prepare tag guard")
    expect(prepare["runs-on"], "ubuntu-latest", "prepare runner")
    expect(prepare["defaults"], {"run": {"shell": "/usr/bin/bash --noprofile --norc -e -o pipefail {0}"}}, "prepare shell")
    expect(prepare["permissions"], {"contents": "read"}, "prepare permissions")
    expect(prepare["env"], {"BASH_ENV": "/dev/null"}, "prepare environment")
    prepare_steps = steps(prepare["steps"], [
        "Checkout tag tree", "Build release assets", "Verify release assets against README pins",
        "Upload verified release assets",
    ])
    expect(pinned(prepare_steps[0], "Checkout tag tree", "actions/checkout", "prepare checkout"), {}, "prepare checkout")
    expect(set(prepare_steps[1]), {"name", "run"}, "prepare builder")
    if "cargo build --locked --release --package agent" not in prepare_steps[1]["run"]:
        fail("prepare agent build")
    expect(set(prepare_steps[2]), {"name", "id", "run"}, "prepare verification")
    expect(prepare_steps[2]["id"], "pins", "prepare verification id")
    expect(prepare_steps[2]["run"], pins_run(), "prepare strict README verification")
    expect(pinned(prepare_steps[3], "Upload verified release assets", "actions/upload-artifact", "prepare artifact"), {
        "with": {
            "name": "release-assets",
            "path": "assets/install-agent.sh\nassets/lg-agent-x86_64-unknown-linux-gnu\n",
            "if-no-files-found": "error", "compression-level": "0",
        },
    }, "prepare artifact")

    publisher = mapping(jobs["release-assets"], "publisher mapping")
    expect(set(publisher), {"if", "needs", "runs-on", "permissions", "defaults", "env", "steps"}, "publisher allowlist")
    expect(publisher["if"], "${{ github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v') }}", "publisher tag guard")
    expect(publisher["runs-on"], "ubuntu-latest", "publisher runner")
    expect(publisher["defaults"], {"run": {"shell": "/usr/bin/bash --noprofile --norc -e -o pipefail {0}"}}, "publisher shell")
    expect(publisher["permissions"], {"contents": "write"}, "publisher permissions")
    expect(publisher["env"], {"BASH_ENV": "/dev/null"}, "publisher environment")
    expect(publisher["needs"], "[image, prepare-release-assets]", "publisher dependencies")
    publisher_steps = steps(publisher["steps"], [
        "Checkout tag tree", "Download verified release assets", "Verify and publish GitHub Release assets",
    ])
    expect(pinned(publisher_steps[0], "Checkout tag tree", "actions/checkout", "publisher checkout"), {}, "publisher checkout")
    expect(pinned(publisher_steps[1], "Download verified release assets", "actions/download-artifact", "publisher artifact download"), {
        "with": {"name": "release-assets", "path": "assets"},
    }, "publisher artifact download")
    expect(publisher_steps[2], {
        "name": "Verify and publish GitHub Release assets",
        "env": {"GH_TOKEN": "${{ github.token }}"},
        "run": publish_run(),
    }, "publisher strict verification and fixed gh release")

    image = mapping(jobs["image"], "image mapping")
    expect(set(image), {"needs", "if", "runs-on", "permissions", "steps"}, "image allowlist")
    expect(image["permissions"], {"contents": "read", "packages": "write"}, "image permissions")
    # The image carries the pins, so it is never built for a tag whose assets failed verification.
    expect(image["needs"], "prepare-release-assets", "image dependencies")
    expect(image["if"], "${{ !cancelled() && (needs.prepare-release-assets.result == 'success'"
           " || github.event_name == 'workflow_dispatch') }}", "image verification guard")
    builders = [step for step in image["steps"] if step.get("uses", "").startswith("docker/build-push-action@")]
    if len(builders) != 1 or not isinstance(builders[0].get("with"), dict):
        fail("image build step")
    expect(builders[0]["with"].get("build-args"), "".join(
        f"{arg}=${{{{ needs.prepare-release-assets.outputs.{name} }}}}\n"
        for arg, name in zip(
            ("LG_AGENT_URL", "LG_AGENT_SHA256", "LG_AGENT_INSTALL_SCRIPT_URL", "LG_AGENT_INSTALL_SCRIPT_SHA256"),
            PIN_OUTPUTS,
        )
    ), "image pin build args")
    # A manual run skips the pin job, so on a tag ref it must never push over the release tags.
    push_guard = ("${{ github.event_name != 'workflow_dispatch'"
                  " || (inputs.push == true && !startsWith(github.ref, 'refs/tags/')) }}")
    expect(builders[0]["with"].get("push"), push_guard, "image push guard")
    logins = [step for step in image["steps"] if step.get("uses", "").startswith("docker/login-action@")]
    if len(logins) != 1 or logins[0].get("if") != push_guard:
        fail("image login guard")

    for job_name, job in jobs.items():
        for step in job["steps"]:
            if "uses" in step:
                pinned(step, step.get("name"), step["uses"].split("@", 1)[0], "action not pinned to a commit SHA")
        for value in scalar_values(job):
            if "GITHUB_ENV" in value or "PATH" in value:
                fail("environment-file or PATH mutation")
            # Only the exact pins script may name a release download URL outside the publisher.
            if job_name != "release-assets" and ("gh release" in value or ("/releases" in value and value != pins_run())):
                fail("alternate publication path")


def validate_ci(text):
    # CI pins every action to a full commit SHA and builds against Cargo.lock.
    for line in text.splitlines():
        uses = re.match(r"\s*(?:-\s+)?uses:\s*(\S+)", line)
        if uses and not re.fullmatch(r"[\w.-]+/[\w./-]+@[0-9a-f]{40}", uses.group(1)):
            fail(f"ci action not pinned to a commit SHA: {uses.group(1)}")
        if re.search(r"\bcargo (build|test|clippy|fetch)\b", line) and "--locked" not in line:
            fail(f"ci cargo command without --locked: {line.strip()}")


try:
    ci = Path(sys.argv[2]).read_text()
    validate_ci(ci)
    ci_mutations = {
        "ci-tag-pinned-action": lambda text: re.sub(r"@[0-9a-f]{40}", "@v4", text, count=1),
        "ci-unlocked-cargo": lambda text: text.replace(" --locked", "", 1),
    }
    for name, mutate in ci_mutations.items():
        try:
            validate_ci(mutate(ci))
        except ValueError:
            print(f"ci workflow mutation rejected: {name}")
            continue
        fail(f"mutation passed: {name}")
    workflow = parse(Path(sys.argv[1]).read_text().splitlines())
    validate(workflow)
    mutations = {
        "builder-in-publisher": lambda value: value["jobs"]["release-assets"]["steps"].insert(
            1, {"name": "Build release assets", "run": "cargo build"}
        ),
        "privilege-leakage": lambda value: value["jobs"]["prepare-release-assets"]["permissions"].update({"contents": "write"}),
        "artifact-tampering": lambda value: value["jobs"]["release-assets"]["steps"][2].update(
            {"run": publish_run().replace("/usr/bin/gh release", "printf tampered > assets/install-agent.sh\n/usr/bin/gh release")}
        ),
        "environment-export": lambda value: value["jobs"]["release-assets"]["steps"][2].update(
            {"run": value["jobs"]["release-assets"]["steps"][2]["run"] + "printf x >> $GITHUB_ENV\n"}
        ),
        "alternate-publication-path": lambda value: value["jobs"].update(
            {"shadow-release": {"runs-on": "ubuntu-latest", "steps": [{"name": "Publish", "run": "/usr/bin/gh release create shadow"}]}}
        ),
        "branch-push-trigger": lambda value: value["on"]["push"]["tags"].append("main"),
        "permissive-guard": lambda value: value["jobs"]["prepare-release-assets"].update({"if": "${{ true }}"}),
        "altered-runner": lambda value: value["jobs"]["release-assets"].update({"runs-on": "windows-latest"}),
        "altered-default-shell": lambda value: value["jobs"]["prepare-release-assets"].update({"defaults": {"run": {"shell": "bash {0}"}}}),
        "tag-pinned-checkout": lambda value: value["jobs"]["release-assets"]["steps"][0].update({"uses": "actions/checkout@v4"}),
        "tag-pinned-image-action": lambda value: value["jobs"]["image"]["steps"][0].update({"uses": "actions/checkout@v4"}),
        "image-before-verification": lambda value: value["jobs"]["image"].pop("needs"),
        "image-guard-bypass": lambda value: value["jobs"]["image"].update({"if": "${{ always() }}"}),
        "image-unpinned-agent": lambda value: value["jobs"]["image"]["steps"][-1]["with"].update(
            {"build-args": "LG_AGENT_URL=https://example.invalid/agent\n"}
        ),
        "manual-push-over-release-tag": lambda value: value["jobs"]["image"]["steps"][-1]["with"].update(
            {"push": "${{ github.event_name != 'workflow_dispatch' || inputs.push == true }}"}
        ),
        "release-api-in-pins": lambda value: value["jobs"]["prepare-release-assets"]["steps"][2].update(
            {"run": pins_run() + "/usr/bin/curl -X POST https://api.github.com/repos/x/y/releases\n"}
        ),
        "notices-not-published": lambda value: value["jobs"]["release-assets"]["steps"][2].update(
            {"run": publish_run().replace(" THIRD_PARTY_NOTICES.md", "")}
        ),
    }
    for name, mutate in mutations.items():
        mutant = copy.deepcopy(workflow)
        mutate(mutant)
        try:
            validate(mutant)
        except ValueError:
            print(f"release workflow mutation rejected: {name}")
            continue
        fail(f"mutation passed: {name}")
except (ValueError, IndexError, TypeError, OSError) as error:
    print(f"release workflow missing: {error}", file=sys.stderr)
    sys.exit(1)

print("release workflow check passed")
PY
