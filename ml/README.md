# ml — 模型评测与格式转换

Python。这里不训练任何模型：识别和决策都用现成的。截图、录屏、数据集和模型权重都不进 git（见根目录 `.gitignore`）。

- 认牌方案评测（P0，`vlm_eval/`）：在雀魂单张牌切图和真实截图上，比较开源小型 VLM、在线 API 和现成的认牌模型 pjura/mahjong_soul_vision（ViT-base 图像分类器）。结论是用后者加赤五颜色规则，见 [docs/vlm-eval.md](../docs/vlm-eval.md)。
- 认牌模型导出（`convert/`）：`export_classifier.py` 把 pjura/mahjong_soul_vision 导出成 ONNX（另有 int8 版本），并和 PyTorch 逐张对照；`pillow_fixtures.py` 生成 Rust 移植 Pillow 算法时用的测试数据；`prepare_upload.py` 准备上传到我们 Hugging Face 模型仓库的内容（`classifier`：模型、原有配置、Apache-2.0 许可和说明；`mortal`：Mortal-S42 的 ONNX、上游原样的 NOTICE 和 model-identity.json、权重的 Apache-2.0 和网络结构代码的 AGPL-3.0 许可、说明），上传由你用自己的账号完成。
- 牌桌标定（#8，`calibrate/`）：`table.py` 从录下的整桌画面里找出四家牌河的牌，拟合桌面到屏幕的透视变换和牌河版面，输出适配包里的 `table` 和 `river` 两项。参数在 `calibrate/config.toml`，用法见脚本开头。
- 决策模型（P2）：用 Mortal（haor/Mortal-S42 权重）。这里只做 ONNX 导出，以及和原 PyTorch 模型的输出对照测试。
- 评测集（P1）：在游戏里回放已知牌谱并录屏，用牌谱作为标准答案，评测识别准确率和事件推断是否正确。

评测数据来源：

1. [pjura/mahjong_souls_tiles](https://huggingface.co/datasets/pjura/mahjong_souls_tiles)：雀魂单张牌切图，34 类，不含赤五。只用原图，不用由原图变换出来的增强图
2. 自己截的雀魂截图和牌谱回放录屏，放在本机的 `recordings/` 里

这些图片都是雀魂的游戏美术，只能在本机评测用，不能提交或分发。
