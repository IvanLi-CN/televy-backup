#!/usr/bin/env python3
"""Publish a trusted workflow result as a check run on an exact PR head."""

from __future__ import annotations

import argparse
import json
import os
import re
from urllib import error, request


SHA_RE = re.compile(r"^[0-9a-fA-F]{40}$")
CONCLUSIONS = {"action_required", "cancelled", "failure", "neutral", "success", "skipped", "timed_out"}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--head-sha", required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--conclusion", required=True, choices=sorted(CONCLUSIONS))
    parser.add_argument("--title", required=True)
    parser.add_argument("--summary", required=True)
    parser.add_argument("--details-url", default="")
    parser.add_argument("--api-root", default=os.environ.get("GITHUB_API_URL", "https://api.github.com"))
    args = parser.parse_args(argv)

    if not SHA_RE.fullmatch(args.head_sha):
        raise SystemExit("check head SHA must be a full commit SHA")
    token = os.environ.get("GH_TOKEN", "")
    if not token:
        raise SystemExit("GH_TOKEN is required")

    payload = {
        "name": args.name,
        "head_sha": args.head_sha.lower(),
        "status": "completed",
        "conclusion": args.conclusion,
        "output": {"title": args.title, "summary": args.summary},
    }
    if args.details_url:
        payload["details_url"] = args.details_url
    url = f"{args.api_root.rstrip('/')}/repos/{args.repository}/check-runs"
    req = request.Request(
        url,
        data=json.dumps(payload).encode("utf-8"),
        method="POST",
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "Content-Type": "application/json",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "televy-backup-release-gate",
        },
    )
    try:
        with request.urlopen(req) as response:
            if response.status not in {200, 201}:
                raise SystemExit(f"GitHub check-run API returned HTTP {response.status}")
    except error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise SystemExit(f"GitHub check-run API returned HTTP {exc.code}: {body[:400]}") from exc
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
