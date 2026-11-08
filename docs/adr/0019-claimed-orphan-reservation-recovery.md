# ADR 0019: Claimed Orphan Reservation Recovery

- Status: accepted
- Date: 2026-11-09

## Context

一次版本准备已经创建了 claimed reservation，但后续没有形成 decision、bound、consumed、released 或 artifact identity；Release Product 运行也已完成且没有产生可复用 artifact。原 reservation 仍然是不可变历史事实，但它会阻止同一版本的后续正式发布。该情况不同于已经进入 released 状态的 legacy claim，也不同于拥有 merge identity 的 same-SHA recovery。

## Decision

增加受保护的 dispatch-only workflow `Recover unbound release reservation`。workflow 必须从 trusted `main` 检查精确的原始 reservation、产品 tag 和 GitHub Release 缺失、所有后续 identity refs 缺失，以及有界的成功 Release Product run 且其 artifact 列表为空。

验证成功后，`release_reservation.py recover-orphan` 只追加 `refs/tags/release-orphaned/v<version>/<reservation-id>`。receipt 的父节点是原 reservation commit，旧 reservation ref 保持不变；trailers 记录原 claim、边界 token、absence facts、检查过的 run、证据主线 SHA、证据 digest 和 `immutable-orphan-unbound-recovery` provenance。

版本解析器只对合法 orphan receipt 豁免旧 reservation 和该 receipt 的占用，并要求下一次 claim 使用新的 `release-reservation-reclaimed` 和新的 claim key。后续发布仍然必须经过新的 VERSION-only release PR、正常检查、merge identity、Release Product 和最终 consumed receipt。该 receipt 不提供 same-SHA recovery，也不创建产品 Release identity。

所有 writer 继续使用默认 `GITHUB_TOKEN`，只允许 append-only create 和相同请求的幂等重试。缺失、冲突、伪造或不完整的 receipt 都保持版本 occupied 并 fail closed；没有删除、更新或回滚旧 reservation 的操作。

## Consequences

- 未绑定的历史 reservation 有明确、可审计且与 legacy retirement 分离的恢复边界。
- 同一版本可以通过独立 reclaimed namespace 重新进入正常 release flow，而不会破坏旧 reservation 的 ref-file 结构或历史 SHA。
- recovery workflow 需要明确的 Release Product run 列表；它不能扫描未知历史或把空 artifact 的失败运行当作成功证据。
- orphan receipt 只释放版本解析占用，不能绕过新的 PR、review、merge、发布和 artifact 验证。

## Evidence

- [Manual version release delivery Spec](../specs/manual-version-release-delivery/SPEC.md)
- [Immutable Release Identity Reservation](0010-release-identity-reservation.md)
- [Same-SHA Recovery Bound Repair](0013-release-recovery-bound-repair.md)
- [Unpublished Legacy Identity Retirement](0018-unpublished-legacy-identity-retirement.md)
