# majsoul-web — 雀魂网页版

`profile.json` 只有数字和文字，不含任何游戏截图或素材。

- `windows`：浏览器窗口，标题里含"雀魂""じゃんたま""Mahjong Soul"之一
- `tile_face`：牌面的颜色。平均亮度高于 `min_brightness`，并且三个通道相差小于 `max_spread`（浅灰色）
- `hand`：以手牌为锚点的版面比例，单位是 16:9 游戏区域（`aspect`）的宽和高。在 10 张真实截图上量出（#3）：
  - `pitch`：牌间距 ÷ 游戏区域宽，0.0479–0.0499，取中位数
  - `left`：手牌左端 ÷ 游戏区域宽
  - `top`：手牌上沿 ÷ 游戏区域高
