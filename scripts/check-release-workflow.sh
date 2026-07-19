#!/usr/bin/env sh
set -eu

workflow=${1:-.github/workflows/release.yml}

python3 - "$workflow" <<'PY'
import copy
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
    expect(set(prepare), {"if", "runs-on", "permissions", "defaults", "env", "steps"}, "prepare allowlist")
    expect(prepare["if"], "${{ github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v') }}", "prepare tag guard")
    expect(prepare["runs-on"], "ubuntu-latest", "prepare runner")
    expect(prepare["defaults"], {"run": {"shell": "/usr/bin/bash --noprofile --norc -e -o pipefail {0}"}}, "prepare shell")
    expect(prepare["permissions"], {"contents": "read"}, "prepare permissions")
    expect(prepare["env"], {"BASH_ENV": "/dev/null"}, "prepare environment")
    prepare_steps = steps(prepare["steps"], [
        "Checkout tag tree", "Build release assets", "Verify release assets against README pins",
        "Upload verified release assets",
    ])
    expect(prepare_steps[0], {"name": "Checkout tag tree", "uses": "actions/checkout@v4"}, "prepare checkout")
    expect(set(prepare_steps[1]), {"name", "run"}, "prepare builder")
    if "cargo build --locked --release --package agent" not in prepare_steps[1]["run"]:
        fail("prepare agent build")
    expect(set(prepare_steps[2]), {"name", "run"}, "prepare verification")
    expect(prepare_steps[2]["run"], verify_run(), "prepare strict README verification")
    expect(prepare_steps[3], {
        "name": "Upload verified release assets", "uses": "actions/upload-artifact@v4",
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
    expect(publisher_steps[0], {"name": "Checkout tag tree", "uses": "actions/checkout@v4"}, "publisher checkout")
    expect(publisher_steps[1], {
        "name": "Download verified release assets", "uses": "actions/download-artifact@v4",
        "with": {"name": "release-assets", "path": "assets"},
    }, "publisher artifact download")
    expect(publisher_steps[2], {
        "name": "Verify and publish GitHub Release assets",
        "env": {"GH_TOKEN": "${{ github.token }}"},
        "run": verify_run() + '/usr/bin/gh release create "$GITHUB_REF_NAME" --verify-tag --generate-notes assets/install-agent.sh assets/lg-agent-x86_64-unknown-linux-gnu\n',
    }, "publisher strict verification and fixed gh release")

    image = mapping(jobs["image"], "image mapping")
    expect(set(image), {"runs-on", "permissions", "steps"}, "image allowlist")
    expect(image["permissions"], {"contents": "read", "packages": "write"}, "image permissions")

    for job_name, job in jobs.items():
        for value in scalar_values(job):
            if "GITHUB_ENV" in value or "PATH" in value:
                fail("environment-file or PATH mutation")
            if job_name != "release-assets" and ("gh release" in value or "/releases" in value):
                fail("alternate publication path")


try:
    workflow = parse(Path(sys.argv[1]).read_text().splitlines())
    validate(workflow)
    mutations = {
        "builder-in-publisher": lambda value: value["jobs"]["release-assets"]["steps"].insert(
            1, {"name": "Build release assets", "run": "cargo build"}
        ),
        "privilege-leakage": lambda value: value["jobs"]["prepare-release-assets"]["permissions"].update({"contents": "write"}),
        "artifact-tampering": lambda value: value["jobs"]["release-assets"]["steps"][2].update(
            {"run": verify_run() + 'printf tampered > assets/install-agent.sh\n/usr/bin/gh release create "$GITHUB_REF_NAME" --verify-tag --generate-notes assets/install-agent.sh assets/lg-agent-x86_64-unknown-linux-gnu\n'}
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
except (ValueError, IndexError, TypeError) as error:
    print(f"release workflow missing: {error}", file=sys.stderr)
    sys.exit(1)

print("release workflow check passed")
PY
