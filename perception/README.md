# perception — 采集与识别

Rust crate，和界面无关。

- 窗口采集：按平台实现，对上层提供同一个接口。macOS 用 ScreenCaptureKit，Windows（P2）用 Windows.Graphics.Capture
- 变化检测和稳定帧门控：画面不变就不推理，动画帧直接跳过
- 画面配准和版面定位：用适配包里的标准截图和区域模板，传统算法，不训练
- 认牌：先查运行时模板缓存，没命中再把单张牌的小图交给本地 VLM（llama.cpp 或 MLX，以选型评测为准），答案限定在 37 个牌名里
- 数字和按钮：数字交给 VLM 读，按钮先用颜色检测发现、再由 VLM 确认
- 输出带置信度的局面快照，交给 [core](../core/)
- 读取 [profiles](../profiles/) 里的游戏适配包
