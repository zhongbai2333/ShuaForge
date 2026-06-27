<!--
提交前请确认：
- 选择一个分类，并在标题前保留对应前缀：
  - [BUG] 修复: ...
  - [Feature] 新功能: ...
  - [Doc] 文档: ...
  - [Update] 更新/优化: ...
-->

# Pull Request 模板

## 分类

- [ ] BUG 修复（bug）
- [ ] 新功能（enhancement）
- [ ] 文档（documentation）
- [ ] 更新/优化（update）

## 变更内容概述

## 相关 Issue（可选）

## 影响范围

- [ ] 桌面端 Rust / egui
- [ ] 题库导入 / 存储
- [ ] 油猴脚本导出器
- [ ] AI 配置 / AI 解析
- [ ] CI / Release / 构建脚本
- [ ] 文档 / 示例

## 验证方式

- [ ] 已运行 `cargo fmt --all -- --check`
- [ ] 已运行 `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] 已运行 `cargo test --all --all-features`
- [ ] 如修改 userscript，已运行 `npm run check:userscript`
- [ ] 如修改 userscript，已运行 `npm run build:userscript`

## 兼容性/风险评估

- [ ] 无破坏性变更
- [ ] 需要文档更新
- [ ] 配置/环境有变更
- [ ] 涉及数据结构或本地 SQLite 存储变更
