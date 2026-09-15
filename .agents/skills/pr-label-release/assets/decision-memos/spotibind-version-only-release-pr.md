# Decision Memo: SpotiBind `VERSION`-only release PR

## Context

- 目标是在不重写已合并历史、不修改 `main` 保护边界的前提下，为缺少 preparation identity 的已合并产品代码建立下一个可发布 identity。
- 该决策运行在 SpotiBind 的 PR-only `main` / GitHub Actions release surface；本备忘只记录只读回验，不执行合并、tag、GitHub Release、规则或 secret 变更。
- 约束是 `VERSION` 仍为唯一数字源、release identity 不可变，且已有发布失败后的 same-SHA recovery 不得写入 `VERSION`。

## Evidence (from references)

- 相邻可复用发布合同：`skills/style-playbook/references/topics/pr-label-release/TOPIC.md` 与 `skills/policies/pr-label-release/assets/templates/pr-label-release.example.json`。
- 项目发布面：`projects/spotibind.md` 指向其 `.github/workflows/release-preparation.yml`、`release-completion.yml`、`release.yml` 和 `VERSION`；该 snapshot 只证明发布面存在，不替代下列当前只读事实。
- 本次只读事实由任务上下文明确提供：`main/VERSION` 为 `0.1.0`，唯一 release/tag 为 `v0.1.0`；已合并的产品 PR #5 没有 preparation identity；PR #7 是 `type:none` bootstrap，不产生 release。该事实是本备忘的应用输入，不被提升为通用 reference implementation。

## Recommendation (default)

- 选择：在一个单独、受控的 SpotiBind 实现任务先让 `Release completion` 和相关测试识别 `version-only-release-pr` proposal 后，为 PR #5 所代表的单一、尚未准备的产品边界创建一个显式、非空的 `VERSION`-only release PR。该 PR 从当前 `main` 建立，branch diff 只改 `VERSION`，以正常 PR 流程通过完整 CI、`Label Gate`、tag reservation 与该 mode 的 `Release completion` 后 merge。
- 为什么符合既有风格：普通 preparation 已无法回到 PR #5 分支；新 PR 保留 PR-only `main`，让新的 merge SHA 与新 `VERSION` 共同成为不可变 release identity，而不伪造旧 merge 的 provenance。
- 版本选择：先以 release labels 冻结 PR #5 的发布意图，再从 `VERSION=0.1.0` 执行相应写入。自动 patch 才写 `0.1.1`；若语义是 minor/major/RC，必须使用受控 exact 值。tag、历史 release 与 package metadata 都不能作为数字 fallback。
- 风险：把这条路径误称为 same-SHA recovery 或 historical backfill，会再次触发“preparation must modify VERSION only”类结构失败，或让旧 SHA 被错误发布。
- 缓解：新 PR 的 provenance 明确记录 PR #5 的 merge SHA、冻结 intent、目标版本和 `release-mode: version-only-release-pr`；release workflow 只发布该新 PR merge SHA/version。

## Alternatives (1-2)

### Option A: same-SHA recovery for PR #5

- 优点：若 PR #5 已有 immutable release identity 且仅 publish 失败，可避免新 PR。
- 缺点：当前 PR #5 没有 preparation identity，输入不成立；recovery 也禁止修改 `VERSION`。
- 何时选择：仅在一个已锁定的 merged SHA/version 已经存在且发布动作失败时选择。

### Option B: historical backfill or release queue/train

- 优点：历史记录可以补足审计语境；队列在其他发布模型中可安排多个候选项。
- 缺点：两者都不能为本 Topic 分配版本或创建 release identity；queue/train 还会把多条产品边界混为一个发布。
- 何时选择：仅为非发布性的历史审计使用 backfill；本 Topic 不使用 queue/train。

## Follow-ups

- 开放问题：PR #5 对应的 `type:*` / `channel:*` 需要在创建补救 PR 前按实际发布语义确认，不能从改动大小或旧 tag 猜测。
- 下一步：先在独立、明确授权的 SpotiBind 任务中实现并验证该 proposal；随后创建且仅创建上述 `VERSION`-only release PR，其合并后按新 merge SHA/version 进入正常 `Release`。本次备忘不授权任何 GitHub 写入。
