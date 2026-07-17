#!/usr/bin/env sh
set -eu

workflow=${1:-.github/workflows/release.yml}

python3 - "$workflow" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()


def need(condition, message):
    if not condition:
        print(f"release workflow missing: {message}", file=sys.stderr)
        sys.exit(1)


need(
    re.search(
        r"(?ms)^on:\n(?:  [^\n]+:\n(?:    [^\n]*\n)*)*  push:\n    tags:\n      - 'v\*'",
        text,
    ),
    "v* push tag trigger",
)
need("  release:\n    types: [published]" in text, "published release trigger")
need(
    re.search(
        r"(?ms)workflow_dispatch:\n    inputs:\n      push:\n(?:        [^\n]*\n)*        default: false\n        type: boolean",
        text,
    ),
    "manual push input defaulting to dry-run",
)
need("permissions:\n  contents: read\n  packages: write" in text, "GHCR package permission")
need("IMAGE_NAME: ghcr.io/${{ github.repository }}" in text, "GHCR image name")
need("docker/login-action@v3" in text, "GHCR login step")
need("docker/build-push-action@v6" in text, "Docker build/push step")
need(
    "if: ${{ github.event_name != 'release' || startsWith(github.event.release.tag_name, 'v') }}"
    in text,
    "release job v* guard",
)
dry_run_guard = "${{ github.event_name != 'workflow_dispatch' || inputs.push == true }}"
need(f"if: {dry_run_guard}" in text, "manual dry-run login guard")
need(f"push: {dry_run_guard}" in text, "manual dry-run push guard")

print("release workflow check passed")
PY
