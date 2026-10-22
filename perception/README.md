# perception — 采集与识别

Rust crate，和界面无关。

- 窗口采集：按平台实现，对上层提供同一个接口。macOS 用 ScreenCaptureKit，Windows（P2）用 Windows.Graphics.Capture
- 变化检测和稳定帧门控：画面不变就不推理，动画帧直接跳过
- 画面配准和版面定位：用适配包里的标准截图和区域模板，传统算法，不训练
- 认牌：先查运行时模板缓存，没命中再把单张牌的小图交给认牌模型 pjura/mahjong_soul_vision（ONNX Runtime，CPU），认出是五时用笔画颜色判断赤五，结果是 37 个牌名之一
- 数字和按钮：方法在 #9 里评测后再定，不用 VLM
- 输出带置信度的局面快照，交给 [core](../core/)
- 读取 [profiles](../profiles/) 里的游戏适配包

## 采集（#1）

- `capture`：`list_windows` 列出可以采集的窗口（不含我们自己的窗口），`start` 开始采集一个窗口，每一帧在采集线程上以 `FrameView`（BGRA，借用不复制）交给回调。位置用相对窗口内容的比例（`RelRect`），窗口缩放、Retina、全屏切换后不变。平台实现在 `capture/macos.rs`（ScreenCaptureKit），其他平台目前返回"不支持"
- `LatestFrame`：从采集线程把最新一帧交给流水线，不排队，缓冲区重复使用
- `change`：`FrameGate` 在采集线程上丢掉没变的帧；`ChangeDetector` 按区域判断变化，连续 70 ms 不变才算稳定，再交给认牌

在真实窗口上看采集效果（帧数、变化帧、稳定事件、尺寸、CPU；不保存任何画面）：

```bash
cargo run -p mahjong-perception --release --example capture_probe
```

```bash
cargo run -p mahjong-perception --release --example capture_probe -- 雀魂 20
```

第一次运行时 macOS 会要求屏幕录制权限。

## 认牌（#4）

- `recognizer`：`TileRecognizer` 是流水线用的入口：先查模板缓存，没命中的牌一次交给认牌模型；牌宽不到 28 像素或置信度低于 0.7 时标为"不确定"（`Doubt`），这样的牌不进缓存。每个"游戏 + 牌面 + 窗口大小"用一个，里面按区域（`Zone`：手牌、各家牌河）各存一份模板，窗口大小变了就 `clear_cache`
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

## 盯窗口（#7）

- `watch`：`Watcher::start` 加载认牌模型并先空跑一次（免得第一手牌慢），开始采集，然后在后台线程上等新帧、做变化检测，稳定后找手牌（`HandTracker`）并认牌
- 找到手牌后只盯手牌那几行；窗口大小变了就重新盯整个窗口，并清空模板缓存
- 手牌变了发 `WatchEvent::Hand`，其中的 `HandReading` 带每张牌、牌框和各环节耗时；画面里没有手牌时发 `NoHand`，窗口关了发 `Stopped`
- 拿到手牌后怎么打不在这里决定，由 app 交给 core
- `FrameTap`：给开发版的截图按钮和连续录制用（#8）：`snapshot` 复制最新一帧；`record` 打开后，每次手牌变化都把读到的那一帧交给 `Recorder`，在发出建议之后才做
- `record`：`Recorder` 在后台线程上把帧存成 PNG，最多排队 4 张，再多就丢掉，不拖慢流水线。只有明确开启的调试录制才用它

## 牌桌：牌河、副露、宝牌指示（#8）

- `table`：牌桌的透视变换。`Table::new` 用适配包里 `table.corners` 和这一帧的游戏区域算出桌面到画面的变换，四家（`Seat`：自家、下家、对家、上家）各一个；`rectify` 把某一家的一块桌面重采样成从那一家看过去的正面图
- `river`：读四家牌河。`rectify` 按 `river` 版面校正出每家的牌河，`find_tiles` 按牌面切牌（行、横放），`crop` 取出转正的牌图，`read` 用每家自己的模板缓存（`Zone::River`）认牌
- `faces`：在校正后的正面图里找牌面（浅色、灰或粉、够大、不在别的牌面里面），判断横放，牌河和副露共用
- `meld`：读四家副露。`rectify` 校正出每家的副露带子，`find_strip` 从右往左找出牌（含横放、加杠叠上的牌、扣着的牌），`group` 按牌本身分成一副副，`read` 用每家自己的模板缓存（`Zone::Meld`）认牌
- `dora`：在左上角找宝牌指示框，`find` 找出翻开的格子，`read` 用自己的模板缓存（`Zone::Dora`）认牌
- `recognizer`：模板缓存按区域（`Zone`）分开，共用一个认牌模型

用录下的帧评测牌河、副露和宝牌指示（标签和图片都只在本机），用法和标签格式见文件开头：

```bash
cargo run -p mahjong-perception --release --example eval_table -- recordings/live --labels recordings/live/table.json
```

重新标定 `table` 和 `river`（换皮肤或别的游戏时）：

```bash
cargo run -p mahjong-perception --release --example game_areas -- recordings/live > /tmp/areas.json
```

```bash
ml/.venv/bin/python -I ml/calibrate/table.py /tmp/areas.json
```

ONNX Runtime 由 `ort` 在构建时下载预编译的静态库，运行时不联网。macOS 上它需要 clang 的运行时库，`build.rs` 会自动链接。
