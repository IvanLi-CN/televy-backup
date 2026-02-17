# 状态面板：下载速率实时显示（1s 窗口）与异常波动修复

## 背景 / 问题陈述

当前 UI 的 NETWORK 区域中，下载（Down）速率经常出现以下问题：

- 显示的速率与系统网络监控（网口流量）差异大，且在下载过程中会出现明显“无正相关”。
- 长下载期间速率会逐步下降到接近 0，但此时系统网络仍在持续下载。
- 偶发出现明显超出物理带宽能力的巨大速率“闪一下就没”的尖峰。

根因（预期）：

- 进度管线里的 `bytes_downloaded` 只在“某个大对象下载完成”时才累计更新（缺乏 streaming progress），导致速率计算只能看到“长时间 0 + 突然跳变”，从而产生 0 值、尖峰与不稳定。
- 对于 daemon 已提供的速率字段，CLI 的 `status stream` 仍可能做二次计算并覆盖，放大上述问题。

## 目标

- 在下载进行中，**Down 速率至少 1Hz 刷新**（最好更快），并符合“最近 1 秒窗口速率”的直觉定义。
- Down 速率不再在长下载中“自然衰减到 0”；当实际下载持续时应持续更新。
- Down 速率不再出现明显不可能的巨大尖峰（> 链路能力的数量级）。

## 非目标

- Down/Up 数值与系统网络监控完全一致：系统统计包含协议开销、重传、TLS/MTProto 额外流量等；本项目以应用层统计为主，要求**趋势一致且稳定**。
- 引入复杂的全链路网络采样（例如抓包/系统 API 读网卡计数）。

## 范围（In / Out）

In:

- `TaskProgress.bytes_downloaded` 的统计与上报补齐（尤其是 remote index 下载与大对象下载场景）。
- daemon 侧速率计算：1 秒滚动窗口，避免 tick cadence 造成尖峰。
- CLI `status stream`：优先保留 daemon 的速率/总量字段；仅在兼容场景下做 best-effort 补齐。

Out:

- 改动 UI 展示样式（只修数据与刷新）。
- 改动 Telegram helper 的网络栈/协议（只在必要时补齐 progress 颗粒度）。

## 需求（MUST）

- MUST: restore/verify/index 下载过程能持续产生 `bytes_downloaded` 的进度更新（支持 streaming）。
- MUST: Down 速率按最近 1 秒窗口计算，stall 超过 1 秒后应降为 0。
- MUST: GUI 中 NETWORK Down 速率至少每秒更新一次（在 status stream 或 polling 路径上都成立）。

## 验收标准（Acceptance Criteria）

- Given: 运行 restore/verify 或包含 remote index 下载的流程
  When: NETWORK 区域显示 Down 速率
  Then:
    - Down 速率在下载期间持续更新，至少 1Hz
    - 不会长时间维持 0（除非下载确实暂停/无字节进展）
    - 不出现明显不可能的尖峰（例如远超链路带宽数量级）

## 测试 / 验证

- `cargo test --workspace`
- 本地手工：运行 macOS app（prod variant）并触发 restore/verify/index sync，观察 NETWORK Down 的刷新与稳定性。

## 风险与开放问题

- 若系统 keychain/签名导致“无法启动/频繁授权弹窗”，需要明确推荐的本地测试启动方式（例如禁用 keychain + workspace dirs）。

