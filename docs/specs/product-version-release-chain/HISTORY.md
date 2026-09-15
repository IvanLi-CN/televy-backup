# History

- Product release identity moved from VERSION-first successor selection to final-tag-first
  allocation with explicit prod, beta, rc, and dev channels.
- PR-local VERSION preparation remains GitHub-native verified and expected-head guarded, while an
  immutable repository reservation now precedes the preparation commit.
- Mainline release identity is recorded by append-only bound and consumed receipts. Git notes,
  control branches, external stores, release queues, history backfill, and automatic recovery PRs
  are outside the topic.
- Pre-provenance lightweight prerelease tags remain historical ordinal occupancy when reachable
  from `main`; they are not identity, recovery, or publication proof.
