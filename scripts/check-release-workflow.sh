#!/usr/bin/env sh
set -eu

workflow=${1:-.github/workflows/release.yml}
readme=${2:-README.md}

python3 - "$workflow" "$readme" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
readme = Path(sys.argv[2]).read_text()


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
need(
    re.search(
        r"(?ms)workflow_dispatch:\n    inputs:\n      push:\n(?:        [^\n]*\n)*        default: false\n        type: boolean",
        text,
    ),
    "manual push input defaulting to dry-run",
)
need("permissions:\n  contents: write\n  packages: write" in text, "release and GHCR permissions")
need("IMAGE_NAME: ghcr.io/${{ github.repository }}" in text, "GHCR image name")
need("release-assets:" in text, "release asset job")
need(
    "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')" in text,
    "tag-only release asset guard",
)
need("rust:1.96-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663" in text, "pinned release asset builder")
need("cargo build --locked --release --package agent" in text, "agent release build")
need("assets/install-agent.sh" in text, "installer release asset")
need("assets/lg-agent-x86_64-unknown-linux-gnu" in text, "agent release asset")
for variable, asset in (
    ("LG_INSTALLER_SHA256", "assets/install-agent.sh"),
    ("LG_AGENT_SHA256", "assets/lg-agent-x86_64-unknown-linux-gnu"),
):
    match = re.search(rf"^{variable}=([0-9a-f]{{64}})$", readme, re.MULTILINE)
    need(match is not None, f"README {variable} pin")
    need(f"{match.group(1)} {asset}" in text, f"workflow {variable} pin equality")
need('gh release create "$GITHUB_REF_NAME" --verify-tag --generate-notes' in text, "GitHub Release asset publication")
need("needs: image" in text, "GHCR publication before release assets")
need(text.index("needs: image") < text.index('gh release create "$GITHUB_REF_NAME"'), "release ordering")
need("docker/login-action@v3" in text, "GHCR login step")
need("docker/build-push-action@v6" in text, "Docker build/push step")
dry_run_guard = "${{ github.event_name != 'workflow_dispatch' || inputs.push == true }}"
need(f"if: {dry_run_guard}" in text, "manual dry-run login guard")
need(f"push: {dry_run_guard}" in text, "manual dry-run push guard")

print("release workflow check passed")
PY
