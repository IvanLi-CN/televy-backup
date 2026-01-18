# Brew 安装与发布最小闭环（#0008）

## 状态

- Status: 待实现
- Created: 2026-01-18
- Last: 2026-01-18

## 背景 / 问题陈述

既然目标是“brew 安装 + brew services 管理”，就需要一个最小可用的发布闭环：用户能安装、能启动后台服务、能升级到新版本，并且在升级后不破坏本地数据目录与 Keychain secrets。

## 目标 / 非目标

### Goals

- 提供 brew 安装方式：formula（后台 daemon）+ cask（GUI app）。
- 提供 `brew services` 的 service 定义（用户级 LaunchAgent，管理 formula 安装的 daemon）。
- 提供升级策略：升级不丢数据（SQLite/配置/Keychain）。

### Non-goals

- 自动更新（Tauri updater）与签名复杂度扩展（不做）。

## 范围（Scope）

### In scope

- 打包产物形态（app bundle / cli / daemon）。
- brew service 的启动命令与日志路径规范。
- 升级兼容性检查（schema migrations、config 迁移、Keychain key 稳定）。

## 需求（Requirements）

### MUST

- 安装后能通过 `brew services start` 启动后台。
- 升级后：配置与索引可继续使用；Keychain secrets 不需要重新输入（除非用户主动擦除）。

## 接口契约（Interfaces & Contracts）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| None | - | - | - | - | - | - | 本交付项主要是发布/运维口径；接口契约沿用 #0001 |

## 验收标准（Acceptance Criteria）

- Given 用户通过 brew 安装，When `brew services start`，Then 后台进程运行且能按 schedule 触发一次备份（以 tasks 表可追溯为准）。
- Given 用户升级版本，When 再次启动后台，Then 仍可读写索引并继续备份，不要求重新配置 token。

## 里程碑（Milestones）

- [ ] M1: 固定产物结构：daemon（formula）+ app（cask）
- [ ] M2: service 定义与日志约定
- [ ] M3: 升级兼容性与回滚策略（文档化）
