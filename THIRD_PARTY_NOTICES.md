# 第三方代码与素材

本项目以 AGPL-3.0-or-later 发布（见 [LICENSE](LICENSE)）。下面列出直接放进仓库或随 app 分发的第三方内容。通过 Cargo 和 npm 引入的依赖不在此列，见 `Cargo.lock` 和 `app/package-lock.json`。

## libriichi（算法部分）

- 位置：[third_party/libriichi/](third_party/libriichi/)
- 来源：[Equim-chan/Mortal](https://github.com/Equim-chan/Mortal) 的 `libriichi/`，提交 `0cff2b52982be5b1163aa9a62fb01f03ce91e0d2`
- 版权与许可：Copyright (C) 2021-2022 Equim，AGPL-3.0-or-later
- 改动：只保留向听、和牌判定与番符、点数表、单人期望值计算、牌和手牌这几部分；`Cargo.toml` 和 `src/lib.rs` 改为不含 Python 绑定。详见该目录的 README
- 它所引用的更早实现：
  - 向听计算和数据表：[tomohxx/shanten-number-calculator](https://github.com/tomohxx/shanten-number-calculator)，LGPL-3.0
  - 和牌判定：[EndlessCheng/mahjong-helper](https://github.com/EndlessCheng/mahjong-helper)，MIT；算法出自山岡忠夫
  - 单人期望值计算：[nekobean/mahjong-cpp](https://github.com/nekobean/mahjong-cpp)，GPL-3.0

## ONNX Runtime

- 位置：静态链接进 app（经 `perception` 的 `ort` 依赖）
- 来源：[microsoft/onnxruntime](https://github.com/microsoft/onnxruntime) 1.31，构建时由 `ort` 2.0.0-rc.15 从 pyke 的 CDN 下载预编译的静态库（[ort.pyke.io](https://ort.pyke.io/)）；运行时不联网
- 版权与许可：Copyright (c) Microsoft Corporation，MIT

## Pillow 的颜色转换和缩放算法

- 位置：[perception/src/pillow.rs](perception/src/pillow.rs)
- 来源：[python-pillow/Pillow](https://github.com/python-pillow/Pillow) 13.0.0 的 `src/libImaging/Convert.c`（`rgb2hsv_row`）和 `src/libImaging/Resample.c`（双线性缩放），按原算法逐步移植到 Rust，使识别结果和 Python 评测时完全一致
- 版权与许可：MIT-CMU，原文如下

  > The Python Imaging Library (PIL) is
  >
  > Copyright © 1997-2011 by Secret Labs AB
  > Copyright © 1995-2011 by Fredrik Lundh and contributors
  >
  > Pillow is the friendly PIL fork. It is
  >
  > Copyright © 2010 by Jeffrey 'Alex' Clark and contributors
  >
  > Like PIL, Pillow is licensed under the open source MIT-CMU License:
  >
  > By obtaining, using, and/or copying this software and/or its associated documentation, you agree that you have read, understood, and will comply with the following terms and conditions:
  >
  > Permission to use, copy, modify and distribute this software and its documentation for any purpose and without fee is hereby granted, provided that the above copyright notice appears in all copies, and that both that copyright notice and this permission notice appear in supporting documentation, and that the name of Secret Labs AB or the author not be used in advertising or publicity pertaining to distribution of the software without specific, written prior permission.
  >
  > SECRET LABS AB AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS SOFTWARE, INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL SECRET LABS AB OR THE AUTHOR BE LIABLE FOR ANY SPECIAL, INDIRECT OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

## 应用图标

- 位置：`app/src-tauri/icons/`
- 来源：Tauri 项目模板自带的默认图标（MIT / Apache-2.0），之后会换成自己的图标
