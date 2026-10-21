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

## 应用图标

- 位置：`app/src-tauri/icons/`
- 来源：Tauri 项目模板自带的默认图标（MIT / Apache-2.0），之后会换成自己的图标
