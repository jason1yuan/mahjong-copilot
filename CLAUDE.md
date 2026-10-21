# CLAUDE.md

离线麻将出牌助手：读取游戏窗口画面，用牌图给出打牌、鸣牌建议和目标牌型。完整设计见 [docs/design.md](docs/design.md)，任务见 GitHub Issues（jason1yuan/mahjong-copilot）。改代码前先读设计文档里相关的那一节。

## 硬性约束（不能违反，拿不准就停下来问）

- 不碰游戏：只读取用户选中窗口的画面像素。不模拟鼠标键盘、不读进程内存、不抓包、不注入。
- 离线：运行时不发任何网络请求，不加遥测，不引入会联网的依赖。截下来的画面不落盘（明确开启的调试录制除外）。
- 延迟：从画面稳定到建议显示 P95 < 300ms。
  - 采集 → 识别 → 决策这条热路径上不做阻塞 IO，不按帧分配大块内存。
  - 画面不进网页视图，界面只接收建议结果。
- 许可：
  - 不复制 Mortal、libriichi 或其他 GPL / AGPL 代码；
  - 新依赖只用 MIT / Apache-2.0 / BSD 这类许可，加之前说明理由；
  - 不使用从游戏里扒出来的牌图或其他素材。
- 签名：不改 bundle identifier（`io.github.jason1yuan.mahjongcopilot`）和签名设置，否则用户的屏幕录制授权会失效。

## 目录与边界

| 目录 | 职责 | 不能做 |
|---|---|---|
| `core/` | 状态引擎、规则、决策（Rust） | 依赖任何平台 API、界面或 perception |
| `perception/` | 采集、ONNX 推理、识别（Rust） | 包含决策逻辑；平台代码必须放在 `#[cfg(target_os = ...)]` 隔离的平台模块里 |
| `app/src-tauri/` | Tauri 外壳：窗口、快捷键、串联各模块 | 写业务逻辑，它只负责组装 |
| `app/src/` | React + TypeScript 界面 | 计算牌效或做任何决策，只渲染收到的结果 |
| `ml/` | 训练和导出（Python） | 提交数据集或模型权重 |
| `profiles/` | 游戏适配包（JSON + 少量图片） | 放代码 |

## 约定

- 牌的写法统一用 mjai 记法：`1m`–`9m`、`1p`–`9p`、`1s`–`9s`、赤五 `5mr` `5pr` `5sr`、字牌 `E S W N P F C`（P 白、F 發、C 中）。代码、测试、日志都用这一套。
- 语言：代码标识符、注释、提交信息用英文；文档、Issue 和界面文案用中文（界面另做中日英多语言）。
- Rust：edition 2024。库里用 `thiserror`，app 里用 `anyhow`；非测试代码不用 `unwrap` / `expect`，确实是不变量的地方要加注释说明。日志用 `tracing`。
- TypeScript：开启 `strict`，不用 `any`。
- Python：写类型标注；训练参数放配置文件，保证能复现。

## 测试

- `core` 的每个规则函数都要有单元测试；向听、进张、番符用何切题和已知牌谱做黄金样例测试。
- 识别相关的改动要报告准确率变化，延迟相关的改动要报告各环节耗时变化。
- 修 bug 时先补一个能复现它的测试。

## 命令

在仓库根目录（Cargo workspace：`app/src-tauri`、`core`、`perception`）：

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

在 `app/` 目录：

- `npm install`（第一次）
- `npm run tauri dev`：启动开发版
- `npm run tauri build`：打包出 `.app` 和 DMG，产物在根目录 `target/release/bundle/`
- `npx tsc --noEmit`：TypeScript 类型检查

提交前上面三条 cargo 命令和类型检查都要通过。

## 工作流程

- 每项工作对应一个 Issue。分支命名 `<issue 号>-<简短英文>`，例如 `6-tauri-workspace`。PR 描述里写 `Closes #N`，并勾选完成的验收标准。
- 小步提交，一个 PR 只做一件事。
- 改了架构或接口，同步更新 `docs/design.md`。
- 先问再做：加重量级依赖，改动设计文档里定下的方案，以及任何涉及上面硬性约束的事。
