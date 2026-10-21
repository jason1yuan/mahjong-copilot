# libriichi-algo

从 Mortal 的 libriichi 里原样拿来的算法代码，供 `mahjong-core` 使用。

- 来源：[Equim-chan/Mortal](https://github.com/Equim-chan/Mortal) 的 `libriichi/`，提交 `0cff2b52982be5b1163aa9a62fb01f03ce91e0d2`（2025-09-28）
- 许可：AGPL-3.0-or-later，Copyright (C) 2021-2022 Equim，原文见同目录的 [LICENSE](LICENSE)
- 包含：`algo/`（向听、和牌判定与番符、点数表、单人期望值计算 sp）、`tile.rs`、`hand.rs`、`macros.rs`、`vec_ops.rs`，以及 `algo/data/` 里的数据表
- 改动（文件开头都有注明）：
  - `Cargo.toml`：只保留上面这些模块用到的依赖，去掉 pyo3 等
  - `src/lib.rs`：只声明上面这些模块，去掉 Python 绑定和其他模块
- 其余文件和上游完全一致。

这些代码本身也引用了更早的开源实现，许可都和 AGPL-3.0 兼容：

| 模块 | 上游 | 许可 |
|---|---|---|
| `algo/shanten.rs` 和向听数据表 | [tomohxx/shanten-number-calculator](https://github.com/tomohxx/shanten-number-calculator) | LGPL-3.0 |
| `algo/agari.rs` | [EndlessCheng/mahjong-helper](https://github.com/EndlessCheng/mahjong-helper)，算法出自山岡忠夫 | MIT |
| `algo/sp/` | [nekobean/mahjong-cpp](https://github.com/nekobean/mahjong-cpp) | GPL-3.0 |

## 维护规则

- 不要改这里的代码，也不要对它运行格式化。需要的功能在 `core/` 里包一层。
- 确实要改时，在被改文件开头写明日期和改动，并更新上面的改动列表。
- 升级上游时，换成新提交的文件，重新应用上面两处改动，再更新提交号。
