# CLAUDE.md

离线麻将出牌助手：读取游戏窗口画面，用牌图给出打牌、鸣牌建议和目标牌型。完整设计见 [docs/design.md](docs/design.md)，任务见 GitHub Issues（jason1yuan/mahjong-copilot）。改代码前先读设计文档里相关的那一节。

## 硬性约束（不能违反，拿不准就停下来问）

- 不碰游戏：只读取用户选中窗口的画面像素。不模拟鼠标键盘、不读进程内存、不抓包、不注入。
- 离线：除了首次启动时下载模型（以及用户手动开启的检查更新），运行时不发任何网络请求，不加遥测，不引入会自行联网的依赖。截下来的画面不落盘（明确开启的调试录制除外）。
- 延迟：从画面稳定到建议显示 P95 < 300ms。
  - 采集 → 识别 → 决策这条热路径上不做阻塞 IO，不按帧分配大块内存。
  - 画面不进网页视图，界面只接收建议结果。
- 许可：项目以 AGPL-3.0-or-later 开源（见 `LICENSE`），因为复用了 Mortal 的 libriichi。为了用上已训练好的模型，我们接受它附带的许可义务。
  - 引入任何第三方代码、模型或素材前，先确认许可允许使用并随 app 分发。许可不明的（比如来源不清的模型权重）不要用，先问；
  - 引入后在 `THIRD_PARTY_NOTICES.md` 里记下来源、版本和许可，并保留原有的版权和许可声明；
  - 依赖必须和 AGPL-3.0 兼容（MIT、Apache-2.0、BSD、MPL-2.0、GPL-3.0 都行），不用"仅限非商业"这类附加限制的许可；
  - 不使用从游戏里扒出来的牌图或其他素材。牌图用 lietxia/mahjong_graphic 的 SVG（M+ 字体许可），不用其中的「雀魂样式」文件夹。
  - 游戏截图、录屏和外部雀魂数据集（如 pjura/mahjong_souls_tiles）只在本机用于评测，不提交到仓库，也不随 app 分发。
- 签名：不改 bundle identifier（`io.github.jason1yuan.mahjongcopilot`）和签名设置，否则用户的屏幕录制授权会失效。

## 目录与边界

| 目录 | 职责 | 不能做 |
|---|---|---|
| `core/` | 状态引擎、规则、决策（Rust）。向听、和牌、算点调用 `third_party/libriichi` | 依赖任何平台 API、界面或 perception |
| `third_party/` | 原样引入的上游代码，目前是 libriichi 的算法部分。不在 Cargo workspace 内，不受我们的 clippy 规则约束 | 修改或格式化；确实要改时，先按它自己的 README 记录改动 |
| `perception/` | 采集、画面配准和版面定位、变化检测、认牌（运行时模板缓存 + 认牌模型 + 赤五颜色规则）（Rust） | 包含决策逻辑；平台代码必须放在 `#[cfg(target_os = ...)]` 隔离的平台模块里 |
| `models/` | 首次启动时下载模型（Rust）：固定地址、大小和 SHA-256，断点续传，校验通过才放进应用数据目录。整个项目只有这里联网 | 下载模型以外的任何联网；被采集、识别、决策的热路径调用 |
| `app/src-tauri/` | Tauri 外壳：窗口、快捷键、串联各模块 | 写业务逻辑，它只负责组装 |
| `app/src/` | React + TypeScript 界面 | 计算牌效或做任何决策，只渲染收到的结果 |
| `ml/` | 模型评测和格式转换（Python）：认牌方案评测，认牌模型和 Mortal 导出 ONNX | 训练任何模型；提交截图、录屏、数据集或模型权重 |
| `profiles/` | 游戏适配包（JSON + 少量图片） | 放代码 |

## 模型

- 决策模型用 Mortal：v4 结构，权重用 Hugging Face 上的 haor/Mortal-S42（Apache-2.0），只支持四麻。不自己训练，也不换用其他来源不明的权重。输入编码直接复用 Mortal 的 libriichi。
- 决策模型转成 ONNX，在 Rust 里用 ONNX Runtime 在 CPU 上推理（实测比 CoreML 快）。转换后要写对照测试，保证和原 PyTorch 模型的输出一致。
- 认牌用现成的雀魂认牌模型 pjura/mahjong_soul_vision（ViT-base 图像分类器，Apache-2.0，由原作者训练），不训练识别模型，产品里也不用 VLM（评测结论见 [docs/vlm-eval.md](docs/vlm-eval.md)）。
  - "牌在哪里"用适配包和传统算法：画面配准、区域模板、变化检测；
  - "这是哪张牌"先查运行时模板缓存，没命中再交给认牌模型。它只认 34 种牌，认出是五时，再按笔画颜色判断是不是赤五（参考 `ml/vlm_eval/red_five.py`）。结果限定在 37 个牌名里，并缓存成模板；
  - 认牌模型转成 ONNX，用 ONNX Runtime 推理，转换后要写对照测试；
  - 原作者的仓库只有 PyTorch 权重，所以转换后的 ONNX 文件（fp32）放在我们自己的 Hugging Face 模型仓库 `xzhe121/mahjong_soul_vision-onnx`，app 从那里按固定修订版下载。它是用游戏截图训练的：仓库里只放这个模型文件和原有的配置，附原始许可（Apache-2.0）和来源、改动说明，不放截图或数据集；收到下架要求就撤下。用 `ml/convert/prepare_upload.py` 准备上传内容。
- 不训练任何模型：识别和决策都用现成的。
- 模型不打包进 app，也不提交到 git 或 GitHub：首次启动时由 `models` crate 从固定版本的地址下载，校验哈希后存到本机的应用数据目录，之后运行不再联网。新增或更换模型时，在 `models/src/lib.rs` 里锁定地址、大小和 SHA-256。

## 约定

- 牌的写法统一用 mjai 记法：`1m`–`9m`、`1p`–`9p`、`1s`–`9s`、赤五 `5mr` `5pr` `5sr`、字牌 `E S W N P F C`（P 白、F 發、C 中）。代码、测试、日志都用这一套。
- 语言：代码标识符、注释、提交信息用英文；文档、Issue 和界面文案用中文（界面另做中日英多语言）。
- Rust：edition 2024。库里用 `thiserror`，app 里用 `anyhow`；非测试代码不用 `unwrap` / `expect`，确实是不变量的地方要加注释说明。日志用 `tracing`。
- TypeScript：开启 `strict`，不用 `any`。
- Python：写类型标注；评测参数放配置文件，保证能复现。

## 测试

- `core` 的每个规则函数都要有单元测试；向听、进张、番符用何切题和已知牌谱做黄金样例测试。
- 识别相关的改动要报告准确率变化，延迟相关的改动要报告各环节耗时变化。
- 修 bug 时先补一个能复现它的测试。

## 命令

在仓库根目录（Cargo workspace：`app/src-tauri`、`core`、`models`、`perception`）：

- `cargo fmt --all`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

在 `app/` 目录：

- `npm install`（第一次）
- `npm run tauri dev`：启动开发版
- `npm run tauri build`：打包出 `.app` 和 DMG，产物在根目录 `target/release/bundle/`
- `npx tsc --noEmit`：TypeScript 类型检查

提交前上面三条 cargo 命令和类型检查都要通过。

发布：把 `app/src-tauri/tauri.conf.json` 的 `version` 改成新版本号并合并到 `main`，CI 自动打包签名正式版 DMG 并发布 Release。开发版不放到 GitHub 上，只在本机构建：调试用 `npm run tauri dev`（截图、日志、牌谱都存在仓库的 `recordings/`，git 忽略），要打包好的开发版：在 `app/` 里 `npm run tauri build -- --config src-tauri/tauri.dev.conf.json --features dev-tools`。

`third_party/libriichi` 的上游测试不在 workspace 里，升级或改动它之后，在根目录单独跑：

- `CARGO_TARGET_DIR=target cargo test --manifest-path third_party/libriichi/Cargo.toml`

## 工作流程

- 每项工作对应一个 Issue。分支命名 `<issue 号>-<简短英文>`，例如 `6-tauri-workspace`。PR 描述里写 `Closes #N`，并勾选完成的验收标准。
- 小步提交，一个 PR 只做一件事。
- 改了架构或接口，同步更新 `docs/design.md`。
- 先问再做：加重量级依赖，改动设计文档里定下的方案，以及任何涉及上面硬性约束的事。
- Dependabot（`.github/dependabot.yml`）每周提依赖升级 PR。合并前确认新版本的许可仍和 AGPL-3.0 兼容（有变化的同步 `THIRD_PARTY_NOTICES.md`），拉到本地跑过上面的提交前检查；PR 上没有 CI，不要直接合并。
