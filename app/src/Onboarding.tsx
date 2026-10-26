// First start (#17): screen recording, the models, the game window, then how to start.
// Each step shows only while it is not done; once all are done and the last one is read,
// the panel follows. Rendering and asking the app only: the checks live in Rust.

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Progress } from "./types";

type Choice = { id: number; app: string; title: string };
type Download =
  | { state: "checking" }
  | { state: "downloading"; downloaded: number; total: number; perSecond: number | null }
  | { state: "ready" }
  | { state: "error"; message: string };

// How often the screen recording grant and the game window are looked for again.
const POLL_MS = 2000;

// Whether the last step was read, kept per viewer; the panel works without it.
function rememberedStarted(): boolean {
  try {
    return localStorage.getItem("onboarded") === "1";
  } catch {
    return false;
  }
}

function rememberStarted() {
  try {
    localStorage.setItem("onboarded", "1");
  } catch {
    // Storage is a convenience only: without it the last step shows again next time.
  }
}

const MB = 1024 * 1024;

function sizeText(bytes: number): string {
  return `${(bytes / MB).toFixed(0)} MB`;
}

function timeText(seconds: number): string {
  if (seconds < 60) return `${Math.max(1, Math.round(seconds))} 秒`;
  return `${Math.round(seconds / 60)} 分钟`;
}

function Steps({ current }: { current: number }) {
  const names = ["屏幕录制", "下载模型", "找到雀魂", "开始"];
  return (
    <ol className="steps">
      {names.map((n, i) => (
        <li key={n} className={i < current ? "done" : i === current ? "current" : ""}>
          <span className="step-dot">{i < current ? "✓" : i + 1}</span>
          {n}
        </li>
      ))}
    </ol>
  );
}

function ScreenStep() {
  const [asked, setAsked] = useState(false);
  // macOS shows its own prompt only the first time an app asks; after a grant was turned
  // off it stays silent. So asking always goes on to the settings page, where the switch is.
  const ask = () => {
    invoke<boolean>("request_screen_access")
      .catch(() => false)
      .finally(() => invoke("open_screen_settings").catch(() => undefined));
    setAsked(true);
  };
  return (
    <section className="onboard-step">
      <h2>允许读取雀魂的画面</h2>
      <p>
        Mahjong Copilot 要看到雀魂窗口里的牌，才能给出建议。它<b>只读取你选中的那个窗口的画面</b>
        ，不操作游戏、不保存截图，也不上传任何数据。macOS 把这种权限叫作「屏幕录制」。
      </p>
      <ol className="howto">
        <li>点下面的「去系统设置里授权」，会打开「隐私与安全性 → 录屏与系统录音」。</li>
        <li>
          在列表里打开 <b>Mahjong Copilot</b> 的开关（如果系统先弹出询问，选「打开系统设置」也一样）。
        </li>
        <li>回到这里点「重新打开」：macOS 要重开 app 授权才生效。</li>
      </ol>
      <div className="onboard-actions">
        <button type="button" className="primary" onClick={ask}>
          去系统设置里授权
        </button>
        {asked && (
          <button type="button" onClick={() => invoke("relaunch").catch(() => undefined)}>
            重新打开
          </button>
        )}
      </div>
      <p className="onboard-note">
        macOS 会不定期再问一次是否继续允许屏幕录制，到时点「允许」就行；如果不小心关掉了，下次打开 app 时引导会重新出现。
      </p>
    </section>
  );
}

function ModelStep({ download }: { download: Download }) {
  const retry = () => invoke("download_models").catch(() => undefined);
  return (
    <section className="onboard-step">
      <h2>下载认牌模型</h2>
      <p>
        认牌模型约 345 MB，只下载这一次，之后完全离线运行。下载完会校验文件，确保没有损坏或被改动；中途断了下次会接着下载。
      </p>
      {download.state === "checking" && <p className="onboard-note">正在检查……</p>}
      {download.state === "downloading" && (
        <>
          <div className="progress">
            <i style={{ width: `${download.total ? (100 * download.downloaded) / download.total : 0}%` }} />
          </div>
          <p className="onboard-note num">
            {sizeText(download.downloaded)} / {sizeText(download.total)} · 还剩{" "}
            {sizeText(Math.max(0, download.total - download.downloaded))}
            {download.perSecond && download.perSecond > 0
              ? ` · 约 ${timeText((download.total - download.downloaded) / download.perSecond)}`
              : ""}
          </p>
        </>
      )}
      {download.state === "error" && (
        <>
          <p className="onboard-error">下载失败：{download.message}</p>
          <div className="onboard-actions">
            <button type="button" className="primary" onClick={retry}>
              重试
            </button>
          </div>
        </>
      )}
      <p className="onboard-note">决策模型（Mortal）以后提供，现在先按牌效给建议。</p>
    </section>
  );
}

function WindowStep() {
  const [choices, setChoices] = useState<Choice[] | null>(null);
  const look = () =>
    invoke<Choice[]>("window_choices")
      .then(setChoices)
      .catch(() => setChoices([]));
  return (
    <section className="onboard-step">
      <h2>打开雀魂</h2>
      <p>在浏览器里打开雀魂（game.maj-soul.com 等），保持窗口在屏幕上，这里会自动找到它。</p>
      <p className="onboard-note">正在查找……</p>
      <div className="onboard-actions">
        <button type="button" onClick={look}>
          找不到？手动选择窗口
        </button>
      </div>
      {choices && (
        <ul className="window-list">
          {choices.length === 0 && <li className="onboard-note">屏幕上没有可以选择的窗口</li>}
          {choices.map((c) => (
            <li key={c.id}>
              <button type="button" onClick={() => invoke("choose_window", { id: c.id }).catch(() => undefined)}>
                <b>{c.app}</b> — {c.title}
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function StartStep({ game, onDone }: { game: Choice; onDone: () => void }) {
  return (
    <section className="onboard-step">
      <h2>准备好了</h2>
      <p>
        找到了雀魂：<b>{game.title}</b>
      </p>
      <ol className="howto">
        <li>
          牌局开始时点标题栏的 <b>「▶ 开始识别」</b>，结束后可以点「⏸ 暂停识别」。
        </li>
        <li>面板会直接在手牌上标出打哪张；状态栏左边的灯是绿色时，建议跟上了牌局。</li>
        <li>
          <b>⌥ Space</b> 显示或隐藏面板；「▭ 紧凑」只留一行。
        </li>
      </ol>
      <div className="onboard-actions">
        <button type="button" className="primary" onClick={onDone}>
          知道了
        </button>
      </div>
    </section>
  );
}

export function Onboarding({ onDone }: { onDone: () => void }) {
  const [screen, setScreen] = useState<boolean | null>(null);
  const [download, setDownload] = useState<Download>({ state: "checking" });
  const [game, setGame] = useState<Choice | null>(null);
  const [started, setStarted] = useState(rememberedStarted);
  const last = useRef<{ at: number; bytes: number } | null>(null);

  useEffect(() => {
    invoke<boolean>("models_ready")
      .then((ready) => setDownload((d) => (ready ? { state: "ready" } : d)))
      .catch(() => undefined);
    const unlisten = [
      listen<Progress>("model-progress", (e) => {
        const now = performance.now();
        const prev = last.current;
        const perSecond = prev && now > prev.at ? ((e.payload.downloaded - prev.bytes) * 1000) / (now - prev.at) : null;
        if (!prev || now - prev.at > 1000) last.current = { at: now, bytes: e.payload.downloaded };
        setDownload((d) => ({
          state: "downloading",
          downloaded: e.payload.downloaded,
          total: e.payload.total,
          perSecond: perSecond ?? (d.state === "downloading" ? d.perSecond : null),
        }));
      }),
      listen("models-ready", () => setDownload({ state: "ready" })),
      listen<string>("models-error", (e) => setDownload({ state: "error", message: e.payload })),
    ];
    return () => {
      unlisten.forEach((p) => p.then((stop) => stop()));
    };
  }, []);

  // The screen recording grant and the game window are looked for until found.
  useEffect(() => {
    const check = () => {
      invoke<boolean>("screen_access")
        .then(setScreen)
        .catch(() => setScreen(false));
      invoke<Choice | null>("game_window")
        .then(setGame)
        .catch(() => setGame(null));
    };
    check();
    const timer = window.setInterval(check, POLL_MS);
    return () => window.clearInterval(timer);
  }, []);

  const ready = screen === true && download.state === "ready" && game !== null;
  useEffect(() => {
    if (ready && started) onDone();
  }, [ready, started, onDone]);

  if (screen === null) return <p className="placeholder">正在启动</p>;
  const step = !screen ? 0 : download.state !== "ready" ? 1 : !game ? 2 : 3;
  return (
    <div className="onboarding">
      <Steps current={step} />
      {step === 0 && <ScreenStep />}
      {step === 1 && <ModelStep download={download} />}
      {step === 2 && <WindowStep />}
      {step === 3 && game && (
        <StartStep
          game={game}
          onDone={() => {
            rememberStarted();
            setStarted(true);
          }}
        />
      )}
    </div>
  );
}
