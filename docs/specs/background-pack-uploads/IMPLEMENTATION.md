# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 目标/非目标、范围（in/out）已明确
- 验收标准覆盖 core path + 队列回压 + 失败传播
- 契约已定稿（见 `./contracts/config.md`），实现与测试可直接按契约落地
- 关键取舍已由主人确认（见 Blockers）


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: uploader 队列回压（capacity/bytes budget）与速率限制口径（min delay）测试（tokio time 控制）。
- Integration tests: 以 fake storage 注入延迟与失败，覆盖：
  - scan 不等待上传（在不触发回压前）
  - 触发回压后的行为符合契约
  - 上传失败能终止任务并给出错误上下文

### Quality checks

- 按仓库既有约定执行 lint/typecheck/格式化/静态检查（不引入新工具）。


## 实现里程碑（Milestones）

- [ ] M1: 设计并落地 upload 队列与 worker pool（有界队列 + 并发上限 + 最小间隔）
- [x] M1: 设计并落地 upload 队列与 worker pool（有界队列 + 并发上限 + 最小间隔）
- [x] M2: 改造 backup scan：pack/direct blob 产出 → enqueue（scan 内不 await 上传）
- [x] M3: upload 阶段 drain 队列并回写 `chunk_objects`；错误传播与取消语义对齐
- [x] M4: 补齐测试（延迟/失败/回压/并发上限）并在本地跑通
- [x] M5: 更新文档（`docs/architecture.md`）并补充可观测口径（scan/upload 分离后的含义）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/0007:background-pack-uploads/PLAN.md`.
