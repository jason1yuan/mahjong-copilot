# app — macOS 应用

Swift + SwiftUI + AppKit，最低 macOS 14，Apple Silicon 优先。

- 采集：ScreenCaptureKit 只捕获游戏窗口，30fps，画面不变就不推理
- 预处理：Metal / vImage 做裁剪、缩放、旋转
- 视觉推理：Core ML（神经网络引擎）找牌和认牌；Apple Vision 离线识字
- 界面：非激活、置顶的悬浮面板（NSPanel），侧栏模式和叠加模式
- 通过 UniFFI 调用 [core](../core/) 的状态与决策逻辑
