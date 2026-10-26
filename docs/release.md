# 发布新版本

发布是自动的：把 `app/src-tauri/tauri.conf.json` 里的 `version` 改成新版本号，合并到 `main`，GitHub Actions（`.github/workflows/release.yml`）就会：

1. 跑 `cargo test --workspace`；
2. 用自签名证书打包、签名正式版 `Mahjong-Copilot_<版本>_aarch64.dmg`；
3. 检查签名；
4. 建标签 `v<版本>`，发布 Release。说明包括 [release-notes.md](release-notes.md) 里的安装步骤、DMG 的 SHA-256，最后是 GitHub 自动列出的、这一版合并的 PR。

版本号没变的提交不会触发发布；这个版本号已经有 Release（包括草稿）时也会跳过。

## 开发版

开发版「Mahjong Copilot Dev」带「开发」区域（调试信息、截图、连续录制），只给开发用，**不放进 Release**。

- 什么时候打包（`.github/workflows/dev-build.yml`）：
  - 版本号改动合并到 `main` 时，和正式版同时自动打包；
  - 两个版本之间想要一个新的开发版时，在仓库的 Actions 页面选「Dev build」→「Run workflow」，选 `main`（或别的分支）手动打包。
- 打包好的开发版签好名，作为那次运行的构建产物（artifact）保存 30 天。
- 下载：仓库的 Actions 页面 →「Dev build」→ 选一次运行 → 页面底部 Artifacts 里的 `Mahjong-Copilot-Dev_<版本>_<提交>`。下载的是 zip，解开就是 DMG。只有对仓库有权限、登录了 GitHub 的人能下载，仓库公开以后也一样。
- bundle id 是 `io.github.jason1yuan.mahjongcopilot.dev`，可以和正式版同时安装；需要单独授权屏幕录制、单独下载模型。录下的画面存在它自己的 app 数据目录的 `recordings/live/` 里。
- 本机也可以自己打：在 `app/` 里运行 `npm run tauri build -- --config src-tauri/tauri.dev.conf.json --features dev-tools`。

## 一次性设置：把签名证书交给 GitHub

CI 要用和本机同一张证书签名，用户升级后屏幕录制授权才不会失效。证书和密码只由你自己操作，不要发给任何人。

1. 按 [signing.md](signing.md) 第 5 步导出的 `.p12` 备份，在「终端」里转成 base64 并复制到剪贴板（文件路径换成你自己的）：

   ```bash
   base64 -i ~/path/to/MahjongCopilot-signing.p12 | pbcopy
   ```

2. 打开仓库的 Settings → Secrets and variables → Actions → New repository secret，添加两个：
   - `SIGNING_CERTIFICATE_P12_BASE64`：粘贴上一步复制的内容；
   - `SIGNING_CERTIFICATE_PASSWORD`：导出 `.p12` 时设置的密码。
3. 清空剪贴板（随便复制一段别的文字）。

正式版和开发版两个工作流共用这两个 Secrets（`.github/actions/signing`）。CI 每次运行时把证书导入运行机器上的临时钥匙串，运行结束机器就被清掉；GitHub 不会在日志里显示 Secrets 的内容。

## 发布一个版本

1. 开一个 PR，把 `tauri.conf.json` 的 `version` 改成新版本号（比如 `0.1.1`），在 PR 描述里写这一版改了什么。
2. 合并后，在仓库的 Actions 页面可以看到「Release」在运行，约十几分钟后 Release 页面出现新版本。
3. 换了签名证书的版本（见 [signing.md](signing.md) 第 7 步），发布后在 Release 说明里手动加上「需要重新授权屏幕录制」的提醒。

## 注意

- 仓库是私有的时候，只有有权限的人能看到和下载 Release。
- 私有仓库的 macOS 机器按 10 倍计算 GitHub Actions 的免费分钟数：一次发布或一次开发版构建大约用掉一两百分钟的额度；公开仓库不计费。普通的合并不会触发任何构建。
