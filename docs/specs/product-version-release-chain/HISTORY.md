# History

- Product release identity moved from VERSION-first successor selection to final-tag-first
  allocation with explicit prod, beta, rc, and dev channels.
- PR-local VERSION preparation remains GitHub-native verified and expected-head guarded, while an
  immutable repository reservation now precedes the preparation commit.
- Mainline release identity is recorded by append-only bound and consumed receipts. Git notes,
  control branches, external stores, release queues, history backfill, and automatic recovery PRs
  are outside the topic.
- Same-SHA recovery may append a missing bound receipt only after reservation, preparation, and
  merge provenance are re-verified; it never allocates a successor or mutates an existing ref.
- Protected reservation and receipt refs use a dedicated GitHub App authority that is explicitly
  allowed by the repository tag ruleset; product annotated tags retain GitHub Actions provenance.
- Pre-provenance lightweight prerelease tags remain historical ordinal occupancy when reachable
  from `main`; they are not identity, recovery, or publication proof.
