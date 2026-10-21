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

目前界面显示的是 Rust 端返回的一手示例牌，用来验证 Rust → 界面这条链路；真实手牌等采集（#1）和识别（#4）完成后接入。
