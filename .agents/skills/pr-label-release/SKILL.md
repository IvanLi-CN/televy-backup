---
name: pr-label-release
description: "Project a verified release impact into PR labels and drive deterministic version and channel selection."
schema_version: 1
kind: policy-skill
slug: pr-label-release
primary_topic: pr-label-release
policy_dependencies:
  - semver-change-governance
visibility: public
public_url_hosts: []
---

# PR label release

Use this project policy when a repository uses pull-request labels to declare release intent. It consumes a verified impact classification; it never derives compatibility from diff size, label presence, or migration file count.

## Workflow

1. Read the installed `semver-change-governance` policy and verify the release unit's `version-impact` record.
2. Copy and fill `assets/templates/pr-label-release.example.json` with the exact type and channel labels, trusted label gate, release workflow input, durable snapshot/queue behavior, and failure context.
3. Make the label gate a declared required check when `quality-gates` is installed, or record an explicit waiver. Require exactly one supported type label; product types require exactly one supported channel label, while `type:docs` and `type:skip` forbid channel labels.
4. Resolve the next version from the verified maximum impact and the repository's tag history. Keep label validation, version identity, and publication as separate observable steps.

The release workflow must consume the checked-in intent and preserve an auditable snapshot. Notification transport remains a project decision governed by `release-failure-notification`.

## Adoption preview

Before applying this policy to a target project, preview the writes to `.agents/skills/semver-change-governance/`, `.agents/skills/pr-label-release/`, and `skills-lock.json`, then wait for explicit owner approval. Because `npx skills` does not resolve this package's `policy_dependencies` metadata, install the hard dependency first and then this policy:

```text
npx skills add IvanLi-CN/style-playbook-skills --skill semver-change-governance --yes
npx skills add IvanLi-CN/style-playbook-skills --skill pr-label-release --yes
```

The CLI detects the Agent and project layout; the dependency order is part of this policy's adoption contract.

## Package resources

- `assets/templates/pr-label-release.example.json`
- `assets/decision-memos/spotibind-version-only-release-pr.md`
