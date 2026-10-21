# perception — 采集与识别

Rust crate，和界面无关。

- 窗口采集：按平台实现，对上层提供同一个接口。macOS 用 ScreenCaptureKit，Windows（P2）用 Windows.Graphics.Capture
- 变化检测和稳定帧门控：画面不变就不推理，动画帧直接跳过
- 推理：ONNX Runtime（ort），mac 走 CoreML，Windows 走 DirectML，CPU 兜底
- 版面定位、找牌、认牌、数字和按钮识别，输出带置信度的逐帧观测，交给 [core](../core/)
- 读取 [profiles](../profiles/) 里的游戏适配包
