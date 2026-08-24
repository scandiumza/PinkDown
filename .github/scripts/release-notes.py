#!/usr/bin/env python3
"""Build GitHub Release notes from conventional commits between tags."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys

SECTION_ORDER = ("Added", "Fixed", "Changed", "Docs", "Other")
PREFIX_TO_SECTION = {
    "feat": "Added",
    "fix": "Fixed",
    "docs": "Docs",
    "perf": "Changed",
    "refactor": "Changed",
    "chore": "Other",
    "ci": "Other",
    "test": "Other",
    "style": "Other",
    "build": "Other",
}
SKIP = re.compile(r"^release:\s*prepare\s", re.I)
CONVENTIONAL = re.compile(
    r"^(?P<type>feat|fix|docs|perf|refactor|chore|ci|test|style|build)"
    r"(?:\([^)]+\))?!?:\s*(?P<msg>.+)$"
)


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return ""
    return result.stdout


def previous_tag(tag: str) -> str:
    return git("describe", "--tags", "--abbrev=0", f"{tag}^").strip()


def commit_subjects(previous: str, tag: str) -> list[str]:
    log = git("log", "--pretty=format:%s", f"{previous}..{tag}")
    return [line.strip() for line in log.splitlines() if line.strip()]


def group_subjects(subjects: list[str]) -> dict[str, list[str]]:
    sections = {name: [] for name in SECTION_ORDER}
    for subject in subjects:
        if SKIP.match(subject):
            continue
        match = CONVENTIONAL.match(subject)
        if match:
            section = PREFIX_TO_SECTION[match.group("type")]
            sections[section].append(match.group("msg").strip())
        else:
            sections["Other"].append(subject)
    return sections


def render_notes(previous: str, tag: str, repo: str, subjects: list[str]) -> str:
    if not previous:
        return "Initial release.\n"

    sections = group_subjects(subjects)
    lines = ["## Changes", ""]
    has_items = False
    for title in SECTION_ORDER:
        items = sections[title]
        if not items:
            continue
        has_items = True
        lines.append(f"### {title}")
        lines.append("")
        lines.extend(f"- {item}" for item in items)
        lines.append("")
    if not has_items:
        lines.append(f"No user-facing changes since {previous}.")
        lines.append("")
    lines.append(f"**Full Changelog**: https://github.com/{repo}/compare/{previous}...{tag}")
    lines.append("")
    return "\n".join(lines)


def render(tag: str, repo: str) -> str:
    previous = previous_tag(tag)
    subjects = commit_subjects(previous, tag) if previous else []
    return render_notes(previous, tag, repo, subjects)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag", help="Git tag being released, e.g. v1.8.7")
    parser.add_argument("repo", help="GitHub owner/name, e.g. 3xian/PinkDown")
    args = parser.parse_args()
    sys.stdout.write(render(args.tag, args.repo))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
