# perception — 采集与识别

Rust crate，和界面无关。

- 窗口采集：按平台实现，对上层提供同一个接口。macOS 用 ScreenCaptureKit，Windows（P2）用 Windows.Graphics.Capture
- 变化检测和稳定帧门控：画面不变就不推理，动画帧直接跳过
- 画面配准和版面定位：用适配包里的标准截图和区域模板，传统算法，不训练
- 认牌：先查运行时模板缓存，没命中再把单张牌的小图交给认牌模型 pjura/mahjong_soul_vision（ONNX Runtime，CPU），认出是五时用笔画颜色判断赤五，结果是 37 个牌名之一
- 数字和按钮：方法在 #9 里评测后再定，不用 VLM
- 输出带置信度的局面快照，交给 [core](../core/)
- 读取 [profiles](../profiles/) 里的游戏适配包

## 认牌（#4）

- `recognizer`：`TileRecognizer` 是流水线用的入口：先查模板缓存，没命中的牌一次交给认牌模型；牌宽不到 28 像素或置信度低于 0.7 时标为"不确定"（`Doubt`），这样的牌不进缓存。每个"游戏 + 牌面 + 窗口大小 + 区域种类"用一个，窗口大小变了就 `clear_cache`
- `cache`：运行时模板缓存。24×32 的小图按像素比较，容忍悬停上浮
- `classifier`：`TileClassifier` 加载 ONNX 模型，`classify` 给出 34 种牌，`recognize` 再加上赤五规则
- `red_five`：`ml/vlm_eval/red_five.py` 的移植，阈值和判定完全相同
- `pillow`：从 Pillow 逐步移植的颜色转换和双线性缩放，保证和 Python 评测看到的像素一模一样

模型不在仓库里。先在根目录导出：

```bash
ml/.venv/bin/python -I ml/convert/export_classifier.py --simulated recordings/simulated_multi
```

需要本机模型的测试默认跳过，这样运行：

```bash
cargo test -p mahjong-perception --release --test classifier -- --ignored
```

用本机截图测准确率和耗时（图片不进仓库）：

```bash
cargo run -p mahjong-perception --release --example eval_classifier -- recordings/source recordings/simulated_multi
```

标定模板缓存的阈值，并模拟连续的牌局（命中率、命中出错、耗时）：

```bash
cargo run -p mahjong-perception --release --example eval_cache -- recordings/source recordings/simulated_multi
```

ONNX Runtime 由 `ort` 在构建时下载预编译的静态库，运行时不联网。macOS 上它需要 clang 的运行时库，`build.rs` 会自动链接。
