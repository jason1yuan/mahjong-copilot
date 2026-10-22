# app — Tauri 2 应用

- `src-tauri/`：Rust 外壳。创建悬浮面板和叠加窗口，注册 ⌥ Space 快捷键，把 [perception](../perception/) 和 [core](../core/) 串成流水线，再把建议结果推给界面
- `src/`：网页界面，React + TypeScript + Vite。用自带的 SVG 牌图渲染手牌、鸣牌提示和目标牌型
- 窗口行为：无边框、置顶、半透明，叠加模式下鼠标穿透；mac 上用 tauri-nspanel 做成不抢焦点的面板
- 画面不经过网页，界面只接收建议结果

## 开发

前置条件：Rust stable（rustup）、Node 22+、Xcode 命令行工具。

```bash
npm install
```

```bash
npm run tauri dev
```

```bash
npm run tauri build
```

打包产物在仓库根目录的 `target/release/bundle/`（`macos/` 下是 .app，`dmg/` 下是 DMG）。

启动后先下载认牌模型（只有第一次），然后自动找雀魂窗口，盯住手牌。每次手牌变化，面板上都会给出打哪张、向听和进张。

窗口底部的灰字是调试行，显示：

- 稳定、找牌、认牌、计算各花了多少毫秒；
- 从画面稳定到建议显示的耗时，以及最近 200 次的 P95。

同样的数字也写在日志里，日志级别用 `RUST_LOG` 调（默认 `info`，ONNX Runtime 只报警告）。

测延迟要用 release 版，debug 版的找牌和认牌要慢好几倍：

```bash
npm run tauri dev -- --release
```
