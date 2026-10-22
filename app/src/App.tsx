import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { tileLabel } from "./tile";
import "./App.css";

// Payloads sent by app/src-tauri/src/pipeline.rs; rendering only.
type Status =
  | { state: "noWindow" }
  | { state: "watching"; app: string; title: string }
  | { state: "error"; message: string };

type TileView = { tile: string; doubt: "small" | "unsure" | null };
type OptionView = { discard: string; shanten: number; ukeireCount: number };
type Advice = {
  seq: number;
  tiles: TileView[];
  drawn: boolean;
  discard: number | null;
  options: OptionView[];
  shanten: number | null;
  ukeire: string[];
  ukeireCount: number | null;
  times: {
    settleMs: number;
    findMs: number;
    recognizeMs: number;
    decideMs: number;
    sentMs: number;
  };
};
type Latency = { count: number; lastMs: number; p50Ms: number; p95Ms: number };
type Progress = { name: string; downloaded: number; total: number };

function shantenText(shanten: number): string {
  if (shanten < 0) return "和了";
  if (shanten === 0) return "听牌";
  return `${shanten} 向听`;
}

function name(mjai: string): string {
  const label = tileLabel(mjai);
  return `${label.rank}${label.suit}${mjai.endsWith("r") ? "（赤）" : ""}`;
}

function Tile({ view, best }: { view: TileView; best: boolean }) {
  const label = tileLabel(view.tile);
  const classes = ["tile", label.red ? "red" : "", best ? "best" : ""].join(" ");
  return (
    <div className={classes} title={view.doubt ? "拿不准" : undefined}>
      {best && <span className="arrow">▼</span>}
      <span>{label.rank}</span>
      {label.suit && <small>{label.suit}</small>}
      {view.doubt && <span className="doubt">?</span>}
    </div>
  );
}

function statusText(status: Status | null, models: string | null): string {
  if (models) return models;
  if (!status) return "正在启动";
  switch (status.state) {
    case "noWindow":
      return "未找到雀魂窗口：请在浏览器中打开雀魂";
    case "watching":
      return status.title;
    case "error":
      return status.message;
  }
}

function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [advice, setAdvice] = useState<Advice | null>(null);
  const [latency, setLatency] = useState<Latency | null>(null);
  const [models, setModels] = useState<string | null>(null);

  useEffect(() => {
    invoke<Status | null>("pipeline_status").then(setStatus).catch(() => undefined);
    const unlisten = [
      listen<Status>("pipeline-status", (e) => setStatus(e.payload)),
      listen<Advice | null>("advice", (e) => setAdvice(e.payload)),
      listen<Latency>("latency", (e) => setLatency(e.payload)),
      listen<Progress>("model-progress", (e) => {
        const percent = Math.floor((100 * e.payload.downloaded) / e.payload.total);
        setModels(`正在下载认牌模型 ${percent}%`);
      }),
      listen("models-ready", () => setModels(null)),
      listen<string>("models-error", (e) => setModels(`模型下载失败：${e.payload}`)),
    ];
    return () => {
      unlisten.forEach((p) => p.then((stop) => stop()));
    };
  }, []);

  // Report when the advice is on screen, for the latency measurement.
  useEffect(() => {
    if (!advice) return;
    const seq = advice.seq;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        invoke("advice_shown", { seq }).catch(() => undefined);
      }),
    );
  }, [advice]);

  const watching = status?.state === "watching";
  const dot = models || status?.state === "error" ? "bad" : watching ? "ok" : "wait";
  const concealed = advice ? advice.tiles.length - (advice.drawn ? 1 : 0) : 0;

  return (
    <main className="panel">
      <header className="titlebar" data-tauri-drag-region>
        <span className={`status-dot ${dot}`} data-tauri-drag-region />
        <span data-tauri-drag-region>麻将 Copilot</span>
        <span className="muted" data-tauri-drag-region>
          {statusText(status, models)}
        </span>
      </header>
      {advice ? (
        <>
          <div className="hand">
            {advice.tiles.map((view, i) => (
              <div key={i} className={i === concealed ? "drawn" : undefined}>
                <Tile view={view} best={i === advice.discard} />
              </div>
            ))}
          </div>
          <p className="info">
            {advice.discard !== null && <strong>打 {name(advice.tiles[advice.discard].tile)}</strong>}
            {advice.shanten !== null && <span>{shantenText(advice.shanten)}</span>}
            {advice.ukeireCount !== null && (
              <span>
                进张 {advice.ukeireCount} 张
                {advice.discard === null && advice.ukeire.length > 0 && `（${advice.ukeire.map(name).join(" ")}）`}
              </span>
            )}
          </p>
          {advice.options.length > 1 && (
            <p className="options">
              {advice.options.map((o) => (
                <span key={o.discard}>
                  {name(o.discard)}：{shantenText(o.shanten)} · {o.ukeireCount} 张
                </span>
              ))}
            </p>
          )}
          <p className="debug">
            稳定 {advice.times.settleMs.toFixed(0)} · 找牌 {advice.times.findMs.toFixed(1)} · 认牌{" "}
            {advice.times.recognizeMs.toFixed(1)} · 计算 {advice.times.decideMs.toFixed(1)} ms
            {latency && ` · 显示 ${latency.lastMs.toFixed(0)} ms · P95 ${latency.p95Ms.toFixed(0)} ms（${latency.count} 次）`}
          </p>
        </>
      ) : (
        <p className="placeholder">{watching ? "等待手牌出现" : statusText(status, models)}</p>
      )}
    </main>
  );
}

export default App;
