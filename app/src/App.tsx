import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { ALL_TILES, tileLabel, tilePicture } from "./tile";
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
// One way to call (#11): the two tiles from the hand, the discard after it and where the
// hand then stands, and the yaku still within reach.
type CallOptionView = {
  kind: "chi" | "pon";
  with: string[];
  discard: string;
  shanten: number;
  ukeireCount: number;
  yaku: string[];
};
// What the state engine (#10) made of the table: the current game's log so far.
type EngineView = {
  events: number;
  round: string | null;
  honba: number | null;
  filled: boolean;
  gaps: number;
  last: "taken" | "covered" | "unexplained" | "impossible";
};
// Whether and how to call the discard on offer; `take` is null to let it go.
type CallView = {
  called: string;
  stayShanten: number;
  stayUkeireCount: number;
  options: CallOptionView[];
  take: number | null;
};
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
  call: CallView | null;
  times: {
    settleMs: number;
    findMs: number;
    recognizeMs: number;
    tableMs: number;
    decideMs: number;
    sentMs: number;
  };
};
// One of Mortal's choices (#22): what to do, the tile discarded, called or made a kan of,
// and for a call the tiles from the hand.
type Suggestion = {
  kind: "discard" | "riichi" | "chi" | "pon" | "kan" | "win" | "draw" | "pass";
  tile: string | null;
  consumed: string[];
  q: number;
};
// Mortal's decision for the advice with the same `seq`.
type MortalView = {
  seq: number;
  best: Suggestion;
  riichiDiscard: Suggestion | null;
  discards: Suggestion[];
  decideMs: number;
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

// A tile in hand: the suggested discard raised in a red frame with an arrow, the second
// and third choices with a bar under them.
function Tile({ view, best, choice }: { view: TileView; best: boolean; choice: 2 | 3 | null }) {
  const classes = ["tile", best ? "best" : ""].join(" ");
  const title = name(view.tile) + (view.doubt ? "（拿不准）" : "");
  return (
    <div className={classes} title={title}>
      {best && <span className="arrow">▼</span>}
      <img src={tilePicture(view.tile)} alt={title} draggable={false} />
      {view.doubt && <span className="doubt">?</span>}
      {choice && <span className={`choice choice-${choice}`} />}
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

const CALLS: Record<string, string> = { chi: "吃", pon: "碰" };
const YAKU: Record<string, string> = { yakuhai: "役牌", tanyao: "断幺", honitsu: "混一色", toitoi: "对对和" };

// 吃 3p（4p5p）→ 打 F · 1 向听 11 张 · 断幺
function callText(called: string, o: CallOptionView): string {
  const yaku = o.yaku.length > 0 ? o.yaku.map((y) => YAKU[y] ?? y).join(" ") : "无役";
  return `${CALLS[o.kind]} ${name(called)}（${o.with.map(name).join("")}）→ 打 ${name(o.discard)} · ${shantenText(o.shanten)} ${o.ukeireCount} 张 · ${yaku}`;
}

// 立直，打 5m · 吃 3p（4p5p） · 跳过
function mortalText(m: MortalView): string {
  const t = (s: Suggestion) => (s.tile ? name(s.tile) : "");
  const b = m.best;
  switch (b.kind) {
    case "discard":
      return `打 ${t(b)}`;
    case "riichi":
      return m.riichiDiscard ? `立直，打 ${t(m.riichiDiscard)}` : "立直";
    case "chi":
      return `吃 ${t(b)}（${b.consumed.map(name).join("")}）`;
    case "pon":
      return `碰 ${t(b)}`;
    case "kan":
      return `杠 ${t(b)}`;
    case "win":
      return "和了";
    case "draw":
      return "九种九牌，流局";
    case "pass":
      return "跳过";
  }
}

// The call to take, or to let the discard go, then the other ways to call it.
function CallAdvice({ call }: { call: CallView }) {
  const take = call.take === null ? null : call.options[call.take];
  const stay = `不鸣 ${name(call.called)} · ${shantenText(call.stayShanten)} ${call.stayUkeireCount} 张`;
  const others = call.options.filter((_, i) => i !== call.take);
  return (
    <>
      <p className={take ? "call take" : "call"}>{take ? `✓ ${callText(call.called, take)}` : `✗ ${stay}`}</p>
      {(others.length > 0 || take) && (
        <p className="options">
          {take && <span>{stay}</span>}
          {others.map((o) => (
            <span key={`${o.kind}${o.with.join("")}`}>{callText(call.called, o)}</span>
          ))}
        </p>
      )}
    </>
  );
}

const SCENES: Record<EngineView["last"], string> = {
  taken: "跟上了",
  covered: "画面被挡住",
  unexplained: "这一帧对不上",
  impossible: "这一帧读错了",
};

// 牌谱 东2局 1本场 · 312 个事件, and what the log is missing; the dev build also shows how
// the last scene went.
function EngineLine({ engine, dev }: { engine: EngineView; dev: boolean }) {
  const round = engine.round ? `${WINDS[engine.round[0]] ?? "?"}${engine.round.slice(1)}局` : "";
  const honba = engine.honba ? ` ${engine.honba}本场` : "";
  return (
    <p className="engine">
      牌谱 {round}
      {honba} · {engine.events} 个事件
      {engine.filled && " · 这一局是中途开始看的，前面是补出的，建议在对局开始前打开"}
      {engine.gaps > 0 && ` · ${engine.gaps} 段没看清，已按画面补上`}
      {dev && ` · ${SCENES[engine.last]}`}
    </p>
  );
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
              {t.added && <img className="added" src={tilePicture(t.added)} alt="" draggable={false} />}
              <img src={tilePicture(t.tile)} alt={tableText(t)} draggable={false} />
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
// Every tile picture with its mjai name, to check them in both themes; dev build only.
function TileSheet() {
  return (
    <div className="tile-sheet">
      {[...ALL_TILES, null].map((mjai) => (
        <figure key={mjai ?? "back"}>
          <img src={tilePicture(mjai)} alt={mjai ?? "back"} />
          <figcaption>{mjai ?? "背"}</figcaption>
        </figure>
      ))}
    </div>
  );
}

function DebugRecording({ initial }: { initial: Recording }) {
  const [recording, setRecording] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [sheet, setSheet] = useState(false);

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
      <label>
        <input type="checkbox" checked={sheet} onChange={(e) => setSheet(e.target.checked)} />
        牌图
      </label>
      {error && <span className="dev-error">{error}</span>}
      {sheet && <TileSheet />}
    </div>
  );
}

function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [advice, setAdvice] = useState<Advice | null>(null);
  const [latency, setLatency] = useState<Latency | null>(null);
  const [models, setModels] = useState<string | null>(null);
  const [recording, setRecording] = useState<Recording | null>(null);
  const [engine, setEngine] = useState<EngineView | null>(null);
  const [mortal, setMortal] = useState<MortalView | null>(null);

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
      listen<EngineView>("engine", (e) => setEngine(e.payload)),
      listen<MortalView>("mortal", (e) => setMortal(e.payload)),
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
  // Mortal's decision for this hand comes first; the rule-based advice is the fallback.
  const ai = advice && mortal?.seq === advice.seq ? mortal : null;
  const lastIndex = (tile: string | null, skip: number | null) => {
    if (!advice || tile === null) return null;
    for (let i = advice.tiles.length - 1; i >= 0; i--) {
      if (advice.tiles[i].tile === tile && i !== skip) return i;
    }
    return null;
  };
  const aiDiscard = ai?.best.kind === "discard" ? ai.best : (ai?.riichiDiscard ?? null);
  const best = (aiDiscard && lastIndex(aiDiscard.tile, null)) ?? advice?.discard ?? null;
  // The second and third choices of discard, by their place in the hand.
  const choices = new Map<number, 2 | 3>();
  const others =
    ai && ai.discards.length > 0
      ? ai.discards.slice(1, 3).map((d) => d.tile)
      : (advice?.options.slice(1, 3).map((o) => o.discard) ?? []);
  others.forEach((tile, k) => {
    const i = lastIndex(tile, best);
    if (i !== null) choices.set(i, k === 0 ? 2 : 3);
  });

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
                <Tile view={view} best={i === best} choice={choices.get(i) ?? null} />
              </div>
            ))}
            {advice.table && advice.table.melds[0].length > 0 && <OwnMelds melds={advice.table.melds[0]} />}
          </div>
          {ai && <p className="mortal">Mortal：{mortalText(ai)}</p>}
          <p className="info">
            {advice.discard !== null &&
              (ai ? (
                <span>牌效 打 {name(advice.tiles[advice.discard].tile)}</span>
              ) : (
                <strong>打 {name(advice.tiles[advice.discard].tile)}</strong>
              ))}
            {advice.shanten !== null && <span>{shantenText(advice.shanten)}</span>}
            {advice.ukeireCount !== null && (
              <span>
                进张 {advice.ukeireCount} 张
                {advice.discard === null && advice.ukeire.length > 0 && `（${advice.ukeire.map(name).join(" ")}）`}
              </span>
            )}
            {advice.dora.length > 0 && (
              <span className="dora">
                宝牌指示
                {advice.dora.map((d, i) => (
                  <img
                    key={i}
                    src={tilePicture(d.tile)}
                    alt={name(d.tile)}
                    title={name(d.tile) + (d.doubt ? "（拿不准）" : "")}
                    draggable={false}
                  />
                ))}
              </span>
            )}
            {advice.seen !== null && <span className="muted-inline">已见 {advice.seen} 张</span>}
          </p>
          {advice.call && <CallAdvice call={advice.call} />}
          {advice.game && gameText(advice.game) && <p className="game">{gameText(advice.game)}</p>}
          {engine && <EngineLine engine={engine} dev={recording !== null} />}
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
            {advice.times.recognizeMs.toFixed(1)} · 牌桌 {advice.times.tableMs.toFixed(1)} · 计算 {advice.times.decideMs.toFixed(1)}
            {ai && ` · Mortal ${ai.decideMs.toFixed(1)}`} ms
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
