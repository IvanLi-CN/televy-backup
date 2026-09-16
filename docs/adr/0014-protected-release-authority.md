# Protected Release Authority

- Status: accepted
- Date: 2026-09-16

The release identity chain must keep server-side deletion and non-fast-forward protection while GitHub's default Actions integration cannot bypass the repository's personal-account tag ruleset. Use a dedicated GitHub App installation with only `Contents: read and write` on this repository as the protected release authority, and configure that App explicitly as the ruleset bypass actor. The workflow must fail before release work when the App configuration is missing. Product annotated tags continue to use the default `GITHUB_TOKEN` so their required `github-actions[bot]` provenance remains unchanged; the App token is used for reservation and receipt refs only. The receipt writer remains append-only and idempotent at the application layer because a bypass actor is exempt from the server ruleset.

## Considered Options

- A maintainer-only receipt write provides the smallest automation authority but makes the release chain partially manual.
- Removing receipt refs from the ruleset weakens the immutable audit chain.
- A personal PAT expands credential scope and is harder to rotate safely.

## Consequences

The repository must maintain `TELEVYBACKUP_RELEASE_APP_ID`, `TELEVYBACKUP_RELEASE_APP_PRIVATE_KEY`, and the App's ruleset bypass membership. Missing or invalid configuration fails closed before packaging. Same-SHA recovery remains the only repair path; a failed receipt never allocates a successor version.
