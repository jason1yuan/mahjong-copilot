# 麻将 Copilot

离线麻将出牌助手，先支持 macOS，之后支持 Windows。它实时读取屏幕上的牌局，用牌图直接标出该打哪张、要不要吃碰杠、以及最值得追的 3 种胡牌牌型。

- 只读屏幕像素：不读内存、不抓包、不注入，也不操作游戏
- 完全离线：识别和决策都在本机完成
- 开箱即用：装好后在系统设置里确认打开一次，再授权屏幕录制、选中游戏窗口即可
- 短期适配日本麻将（雀魂、雀姬），长期适配任意三人 / 四人麻将游戏（网页版、Steam 单机版）

## 状态

设计阶段。完整方案见 [docs/design.md](docs/design.md)，进度按 GitHub Milestones（P0 → P3）跟踪。

## 技术栈

Tauri 2 + Rust + React / TypeScript，模型用 ONNX Runtime 推理。选型理由见设计方案第七节。

## 目录

| 目录 | 内容 |
|---|---|
| [app/](app/) | Tauri 2 应用：Rust 外壳（窗口、快捷键、把各模块串成流水线）和网页界面（React + TypeScript） |
| [perception/](perception/) | 采集与识别（Rust）：按平台实现的窗口采集、ONNX Runtime 推理 |
| [core/](core/) | 状态与决策（Rust）：牌局状态引擎、向听 / 进张、打点、防守 |
| [third_party/](third_party/) | 原样引入的上游代码：Mortal 的 libriichi（向听、和牌、算点） |
| [ml/](ml/) | 模型评测和格式转换（Python）：VLM 选型评测，Mortal 导出 ONNX。不训练任何模型 |
| [profiles/](profiles/) | 游戏适配包：各游戏的版面模板、牌面样本、按钮文字 |
| [docs/](docs/) | 设计文档 |

## 路线图

| 阶段 | 内容 |
|---|---|
| P0 原型 | 雀魂网页版四麻（macOS）：Tauri 工程搭建，VLM 选型评测，用本地 VLM 认出手牌，向听 / 进张计算，侧栏 UI 标出打哪张 |
| P1 MVP | 牌河 / 副露 / 宝牌识别，状态引擎和完整牌谱记录，鸣牌和立直建议，基础防守，目标牌型，叠加模式，首次启动引导 |
| P2 | 接入 Mortal 策略模型（四麻），雀姬，三麻，Steam 客户端，皮肤自适应，Windows 支持 |
| P3 | 国标 / 四川 / 广东 / 台湾等规则包，用 VLM 自动定位新游戏的版面 |

## 许可

AGPL-3.0-or-later，见 [LICENSE](LICENSE)。项目复用了 Mortal 的 libriichi 等第三方代码，来源和许可见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 使用须知

雀魂等在线游戏的用户协议一般禁止在对局中使用辅助工具，在段位场使用可能导致封号，对真人对手也不公平。本项目定位为学习和练习工具：AI 对战、友人房、牌谱复盘。
