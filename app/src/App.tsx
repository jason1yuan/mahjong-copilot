import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { tileLabel } from "./tile";
import "./App.css";

// Payloads sent by app/src-tauri/src/pipeline.rs; rendering only.
type Status =
  | { state: "noWindow" }
  | { state: "watching"; app: string; title: string }
  | { state: "error"; message: string };

type TileView = { tile: string; doubt: "small" | "unsure" | null };
// A tile on the table as read; `tile` is null for a tile face down.
type TableTile = { tile: string | null; sideways: boolean; added: string | null; doubt: boolean };
type TableView = { rivers: TableTile[][]; melds: TableTile[][][] };
// The round, the player's wind and the numbers on the table, each null when not read, and
// the action buttons shown, left to right.
type GameView = {
  round: string | null;
  seatWind: string | null;
  scores: number[] | null;
  tilesLeft: number | null;
  honba: number | null;
  riichiSticks: number | null;
  buttons: string[];
};
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
  dora: TileView[];
  seen: number | null;
  table: TableView | null;
  game: GameView | null;
  times: {
    settleMs: number;
    findMs: number;
    recognizeMs: number;
    tableMs: number;
    decideMs: number;
    sentMs: number;
  };
};
type Latency = { count: number; lastMs: number; p50Ms: number; p95Ms: number };
type Progress = { name: string; downloaded: number; total: number };
// The dev build's debug recording (#8); null in a packaged app.
type Recording = { on: boolean; saved: number; dir: string };

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

const WINDS: Record<string, string> = { E: "东", S: "南", W: "西", N: "北" };
const BUTTONS: Record<string, string> = {
  chi: "吃",
  pon: "碰",
  kan: "杠",
  ron: "和",
  riichi: "立直",
  tsumo: "自摸",
};

// 东2局 · 自风 北 · 余 45 · 1 本场 · 供托 0 · 可以 碰 杠, leaving out what was not read.
function gameText(game: GameView): string {
  const parts = [];
  if (game.round) parts.push(`${WINDS[game.round[0]] ?? "?"}${game.round.slice(1)}局`);
  if (game.seatWind) parts.push(`自风 ${WINDS[game.seatWind] ?? "?"}`);
  if (game.tilesLeft !== null) parts.push(`余 ${game.tilesLeft}`);
  if (game.honba !== null) parts.push(`${game.honba} 本场`);
  if (game.riichiSticks !== null) parts.push(`供托 ${game.riichiSticks}`);
  if (game.buttons.length > 0) {
    parts.push(`可以 ${game.buttons.map((b) => BUTTONS[b] ?? b).join(" ")}`);
  }
  return parts.join(" · ");
}

// Seats in turn order from the player, as the table view lists them.
const SEATS = ["自家", "下家", "对家", "上家"];
// The dev build opens taller, to fit the table view.
const DEV_HEIGHT = 430;

function short(mjai: string): string {
  const label = tileLabel(mjai);
  return `${label.rank}${label.suit}${mjai.endsWith("r") ? "赤" : ""}`;
}

function tableText(t: TableTile): string {
  const face = t.tile === null ? "▮" : short(t.tile);
  return `${face}${t.sideways ? "↻" : ""}${t.added ? `+${short(t.added)}` : ""}${t.doubt ? "?" : ""}`;
}

// The player's own melds, beside the hand: sideways tiles lie turned. They come in the order
// they were made, the first at the right on the table, so they are drawn reversed.
function OwnMelds({ melds }: { melds: TableTile[][] }) {
  return (
    <div className="melds">
      {[...melds].reverse().map((meld, i) => (
        <div key={i} className="meld">
          {meld.map((t, j) => (
            <span key={j} className={`meld-tile${t.sideways ? " sideways" : ""}`} title={tableText(t)}>
              {t.tile === null ? "" : short(t.tile)}
              {t.added && <small>+</small>}
            </span>
          ))}
        </div>
      ))}
    </div>
  );
}

// Every seat's river and melds as read; dev build only, for debugging the table reading.
function TableDebug({ table, scores }: { table: TableView; scores: number[] | null }) {
  return (
    <div className="table-debug">
      <p className="legend">↻ 横放（被鸣的牌、立直宣言牌） · +X 加杠 · ▮ 扣着的牌 · ? 拿不准</p>
      {SEATS.map((seat, i) => (
        <p key={seat}>
          <span className="seat">{seat}</span>
          {scores && <span className="score">{scores[i]}</span>}
          <span>河 {table.rivers[i].map(tableText).join(" ") || "—"}</span>
          {table.melds[i].length > 0 && (
            <span> ｜ 副露 {table.melds[i].map((m) => `[${m.map(tableText).join(" ")}]`).join(" ")}</span>
          )}
        </p>
      ))}
    </div>
  );
}

function WindowButtons() {
  const window = getCurrentWindow();
  return (
    <span className="window-buttons">
      <button type="button" className="close" aria-label="关闭" title="关闭" onClick={() => window.close()} />
      <button type="button" className="minimize" aria-label="最小化" title="最小化" onClick={() => window.minimize()} />
      <button
        type="button"
        className="maximize"
        aria-label="最大化"
        title="最大化"
        onClick={() => window.toggleMaximize()}
      />
    </span>
  );
}

// Screenshot button and recording switch for the table evaluation set; dev build only.
function DebugRecording({ initial }: { initial: Recording }) {
  const [recording, setRecording] = useState(initial);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const unlisten = listen<number>("frame-saved", (e) =>
      setRecording((r) => ({ ...r, saved: e.payload })),
    );
    return () => {
      unlisten.then((stop) => stop());
    };
  }, []);

  const report = (err: unknown) => setError(String(err));
  const shoot = () => {
    setError(null);
    invoke("save_frame").catch(report);
  };
  const toggle = (on: boolean) => {
    setError(null);
    invoke<Recording>("set_recording", { on }).then(setRecording).catch(report);
  };

  return (
    <div className="dev">
      <button type="button" onClick={shoot}>
        截图
      </button>
      <label>
        <input type="checkbox" checked={recording.on} onChange={(e) => toggle(e.target.checked)} />
        连续录制
      </label>
      <span title={recording.dir}>已存 {recording.saved} 张</span>
      {error && <span className="dev-error">{error}</span>}
    </div>
  );
}

function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [advice, setAdvice] = useState<Advice | null>(null);
  const [latency, setLatency] = useState<Latency | null>(null);
  const [models, setModels] = useState<string | null>(null);
  const [recording, setRecording] = useState<Recording | null>(null);

  useEffect(() => {
    invoke<Status | null>("pipeline_status").then(setStatus).catch(() => undefined);
    invoke<Recording | null>("debug_recording")
      .then((r) => {
        setRecording(r);
        if (r) getCurrentWindow().setSize(new LogicalSize(560, DEV_HEIGHT)).catch(() => undefined);
      })
      .catch(() => undefined);
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
  const concealed = advice ? advice.tiles.length - (advice.drawn ? 1 : 0) : 0;

  return (
    <main className="panel">
      <header className="titlebar" data-tauri-drag-region>
        <WindowButtons />
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
            {advice.table && advice.table.melds[0].length > 0 && <OwnMelds melds={advice.table.melds[0]} />}
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
            {advice.dora.length > 0 && (
              <span>
                宝牌指示 {advice.dora.map((d) => name(d.tile) + (d.doubt ? "?" : "")).join(" ")}
              </span>
            )}
            {advice.seen !== null && <span className="muted-inline">已见 {advice.seen} 张</span>}
          </p>
          {advice.game && gameText(advice.game) && <p className="game">{gameText(advice.game)}</p>}
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
            {advice.times.recognizeMs.toFixed(1)} · 牌桌 {advice.times.tableMs.toFixed(1)} · 计算 {advice.times.decideMs.toFixed(1)} ms
            {latency && ` · 显示 ${latency.lastMs.toFixed(0)} ms · P95 ${latency.p95Ms.toFixed(0)} ms（${latency.count} 次）`}
          </p>
        </>
      ) : (
        <p className="placeholder">{watching ? "等待手牌出现" : statusText(status, models)}</p>
      )}
      {recording && advice?.table && <TableDebug table={advice.table} scores={advice.game?.scores ?? null} />}
      {recording && <DebugRecording initial={recording} />}
    </main>
  );
}

export default App;
