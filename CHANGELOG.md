# 更新日志

本项目的所有显著变更都记录在此文件中。
格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### 新增

- **SQLite 自动备份**：每日首次启动用 `VACUUM INTO` 产出一致性快照到 `backups/`，保留最近 7 份；失败仅记日志，绝不阻断启动
- **发布流水线**：`release.yml` —— 推送 `v*` 标签自动构建 MSI 并发布 GitHub Release
- 英文版 README（`README.en.md`）与下载指引

### 计划中

- 键盘快捷键（交互稿评审中）

## [0.1.0] — 首个公开版本

### 计时引擎（pristimer-core）

- 锚点计时模型：`elapsed = accumulated + (now − anchor)`，拒绝累加式计数，杜绝漂移
- 双时钟设计：`Instant` 保证进程内精度，`SystemTime` 负责落库与跨进程恢复
- 崩溃安全：WAL 持久化锚点；意外退出后恢复幂等（超 12h 按异常结束，<10s 会话丢弃）
- 倒计时封顶 limit_ms；番茄循环推进在引擎侧完成

### 界面（Vue 3 + 液态玻璃设计系统）

- 液态玻璃设计系统：四向边缘高光 + 底部折射暗影 + accent 环境色边缘光、周期性镜面扫光
- 六套主题（深空 / 虚空 / 晨雾 / 极光 / 暮霞 / 纸墨），`@property` 颜色插值 0.9s 丝滑过渡，每套主题专属背景动效；表盘指针、刻度与盘内粒子同主题换色（前景与底色明度反向）
- 迷你置顶组件：264×96 常驻桌面，原生 `SetWindowPos` 分帧动画（≤67fps，DwmFlush 对齐）平滑缩放
- 表盘粒子：运行时盘内漂移连线，出界沿法线反弹，颜色随主题
- 霓虹按钮多层光晕呼吸；顶部 2.5px 滚动进度条替代右侧滚动条
- 统计视图：年度热力图 / 小时分布 / 趋势曲线，数字滚动补间，浅色主题全量配色适配
- 科目标签：预设 + 自定义、内联重命名、每日目标
- 番茄钟：阶段条 + 可拖拽循环序列卡（拖「长休息」改轮数）+ 到点系统通知

### 持久化与数据（pristimer-store）

- SQLite（rusqlite bundled，WAL 模式）单表会话 + settings 表
- CSV 导出（明细与汇总两式，文件名净化防路径穿越）
- 科目日聚合 / 小时分布 / 年度热力数据

### 工程化

- 单实例守卫（`CreateMutexW` + 前台窗口拾取）、启动 panic 兜底对话框、三段重试开库
- GitHub Actions CI：Rust 测试 + clippy `-D warnings` 门禁 + vue-tsc + vitest
- 前端 vitest 14 例（date / tags / useCountUp）；Rust 测试 50 例
- 顶层 Cargo workspace（core / store / src-tauri 统一 lock 与 target）

[Unreleased]: https://github.com/Prisdvl/PrisTimer/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Prisdvl/PrisTimer/releases/tag/v0.1.0
