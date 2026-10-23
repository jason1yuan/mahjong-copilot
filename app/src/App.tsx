import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { DevArea } from "./DevArea";
import { CALLS, hanText, name, paysText, roundText, sameKind, SEATS, shantenText, WINDS, YAKU } from "./format";
import { Tile } from "./TileImage";
import type {
  Advice,
  CallOptionView,
  EngineView,
  Latency,
  MortalView,
  OptionView,
  Progress,
  Recording,
  Status,
  Suggestion,
  TableTile,
  TileLeft,
  WinView,
} from "./types";
import "./App.css";

// Tile widths in px: the hand, the compact row, the status bar's dora, waits and the call card.
const HAND = 26;
const COMPACT = 22;
const SMALL = 16;
const WAIT = 34;
const UKEIRE = 22;

// Whether compact mode is on, kept per viewer; the panel works without it.
function rememberedCompact(): boolean {
  try {
    return localStorage.getItem("compact") === "1";
  } catch {
    return false;
  }
}

function rememberCompact(on: boolean) {
  try {
    localStorage.setItem("compact", on ? "1" : "0");
  } catch {
    // Storage is a convenience only.
  }
}

// What the panel puts first: Mortal's decision for this hand when there is one, else the
// rule layer's (docs/design.md, #22).
type Plan = {
  // The tile to discard, its rule-layer numbers, and the second and third choices.
  discard: string | null;
  option: OptionView | null;
  others: string[];
  // The big line: 打 北 · 立直 · 打 5萬 · 杠 中 · 自摸.
  headline: string | null;
  // Grey when it is another player's discard to decide on.
  waiting: boolean;
  card: "call" | "win" | null;
  mortal: boolean;
};

function optionFor(advice: Advice, tile: string | null): OptionView | null {
  if (tile === null) return null;
  return (
    advice.options.find((o) => o.discard === tile) ?? advice.options.find((o) => sameKind(o.discard, tile)) ?? null
  );
}

function plan(advice: Advice, ai: MortalView | null, win: WinView | null): Plan {
  const mustDiscard = advice.tiles.length % 3 === 2;
  const ruleDiscard = advice.discard !== null ? advice.tiles[advice.discard].tile : null;
  const ruleOthers = advice.options.slice(1, 3).map((o) => o.discard);
  const callButtons = advice.game?.buttons.some((b) => b === "chi" || b === "pon" || b === "kan") ?? false;
  const mortalCall = ai !== null && ["chi", "pon", "kan", "pass"].includes(ai.best.kind) && !mustDiscard;
  const base: Plan = {
    discard: null,
    option: null,
    others: [],
    headline: null,
    waiting: false,
    card: null,
    mortal: ai !== null,
  };

  if (win) {
    return {
      ...base,
      headline: win.ron ? "可以荣和" : "可以自摸",
      card: "win",
    };
  }
  if (mustDiscard) {
    let discard = ruleDiscard;
    let headline = ruleDiscard ? `打 ${name(ruleDiscard)}` : null;
    let others = ruleOthers;
    if (ai) {
      const b = ai.best;
      const mortalOthers = ai.discards.slice(1, 3).flatMap((d) => (d.tile ? [d.tile] : []));
      if (b.kind === "discard" && b.tile) {
        discard = b.tile;
        headline = `打 ${name(b.tile)}`;
        others = mortalOthers;
      } else if (b.kind === "riichi") {
        discard = ai.riichiDiscard?.tile ?? null;
        headline = discard ? `立直 · 打 ${name(discard)}` : "立直";
        others = [];
      } else if (b.kind === "kan" && b.tile) {
        discard = null;
        headline = `杠 ${name(b.tile)}`;
        others = [];
      } else if (b.kind === "draw") {
        discard = null;
        headline = "九种九牌 · 流局";
        others = [];
      } else if (b.kind === "win") {
        discard = null;
        headline = "自摸";
        others = [];
      }
    }
    return { ...base, discard, option: optionFor(advice, discard), others, headline };
  }
  const called = advice.call?.called ?? (mortalCall && ai?.best.kind !== "pass" ? ai?.best.tile : null) ?? null;
  if (advice.call || (mortalCall && callButtons)) {
    const seat = advice.call?.from ? SEATS[advice.call.from] : "";
    return {
      ...base,
      headline: called ? `${seat}打 ${name(called)}` : "可以鸣牌",
      waiting: true,
      card: "call",
    };
  }
  return base;
}

function WindowButtons({ compact, onCompact }: { compact: boolean; onCompact: () => void }) {
  const window = getCurrentWindow();
  return (
    <span className="window-buttons">
      <button type="button" className="close" aria-label="关闭" title="关闭" onClick={() => window.close()} />
      <button type="button" className="minimize" aria-label="最小化" title="最小化" onClick={() => window.minimize()} />
      <button
        type="button"
        className="compact"
        aria-label={compact ? "完整模式" : "紧凑模式"}
        title={compact ? "完整模式" : "紧凑模式"}
        onClick={onCompact}
      />
    </span>
  );
}

// The player's own melds, beside the hand: sideways tiles lie turned. They come in the order
// they were made, the first at the right on the table, so they are drawn reversed.
function OwnMelds({ melds }: { melds: TableTile[][] }) {
  return (
    <div className="melds">
      {[...melds].reverse().map((meld, i) => (
        <div key={i} className="meld">
          {meld.map((t, j) => (
            <span key={j} className={`meld-tile${t.sideways ? " sideways" : ""}`}>
              {t.added && <Tile tile={t.added} width={18} />}
              <Tile tile={t.tile} width={18} />
            </span>
          ))}
        </div>
      ))}
    </div>
  );
}

// The hand: the drawn tile apart, the first choice raised and framed, the second and third
// with bars; while a call is on offer, the tiles it does not use are dimmed.
function Hand({
  advice,
  p,
  keep,
  width,
}: {
  advice: Advice;
  p: Plan;
  keep: string[] | null;
  width: number;
}) {
  const marks = new Map<number, "main" | 2 | 3>();
  const lastIndex = (tile: string, taken: Set<number>) => {
    for (let i = advice.tiles.length - 1; i >= 0; i--) {
      if (!taken.has(i) && advice.tiles[i].tile === tile) return i;
    }
    for (let i = advice.tiles.length - 1; i >= 0; i--) {
      if (!taken.has(i) && sameKind(advice.tiles[i].tile, tile)) return i;
    }
    return null;
  };
  const taken = new Set<number>();
  if (p.discard) {
    const i = lastIndex(p.discard, taken);
    if (i !== null) {
      marks.set(i, "main");
      taken.add(i);
    }
  }
  p.others.forEach((tile, k) => {
    const i = lastIndex(tile, taken);
    if (i !== null) {
      marks.set(i, k === 0 ? 2 : 3);
      taken.add(i);
    }
  });
  const bright = new Set<number>();
  if (keep) {
    for (const tile of keep) {
      const i = lastIndex(tile, bright);
      if (i !== null) bright.add(i);
    }
  }
  const concealed = advice.tiles.length - (advice.drawn ? 1 : 0);
  const melds = advice.table?.melds[0] ?? [];
  return (
    <div className="hand">
      {advice.tiles.map((view, i) => (
        <span key={i} className={i === concealed ? "drawn" : undefined}>
          <Tile
            tile={view.tile}
            width={width}
            mark={marks.get(i) ?? null}
            dim={keep !== null && !bright.has(i)}
            doubt={view.doubt !== null}
          />
        </span>
      ))}
      {melds.length > 0 && <OwnMelds melds={melds} />}
    </div>
  );
}

// The tiles that improve the hand, or the waits once it is tenpai, with how many are left.
function Ukeire({ shanten, tiles }: { shanten: number; tiles: TileLeft[] }) {
  const total = tiles.reduce((n, t) => n + t.left, 0);
  if (shanten === 0) {
    return (
      <>
        <h2>
          待牌 · {tiles.length} 种 {total} 张
        </h2>
        <div className={tiles.length > 3 ? "waits grid" : "waits"}>
          {tiles.map((t) => (
            <div key={t.tile} className="wait">
              <Tile tile={t.tile} width={tiles.length > 3 ? UKEIRE : WAIT} />
              <div className="wait-left">
                剩 <b className="num">{t.left}</b> 张
                <span className="pips">
                  {[0, 1, 2, 3].map((k) => (
                    <i key={k} className={k < t.left ? "on" : ""} />
                  ))}
                </span>
              </div>
            </div>
          ))}
        </div>
      </>
    );
  }
  return (
    <>
      <h2>
        {shantenText(shanten)} · 进张 {tiles.length} 种 {total} 张
      </h2>
      <div className="ukeire">
        {tiles.map((t) => (
          <div key={t.tile} className={t.left === 0 ? "gone" : undefined}>
            <Tile tile={t.tile} width={UKEIRE} />
            <span className="num">{t.left}</span>
          </div>
        ))}
      </div>
    </>
  );
}

function callLine(o: CallOptionView): string {
  const yaku = o.yaku.length > 0 ? o.yaku.map((y) => YAKU[y] ?? y).join(" ") : "无役";
  return `→ 打 ${name(o.discard)} · ${shantenText(o.shanten)} ${o.ukeireCount} 张 · ${yaku}`;
}

// The call on offer: who discarded what, the call to make (or to let it go), and the
// other choice under it.
function CallCard({ advice, ai }: { advice: Advice; ai: MortalView | null }) {
  const call = advice.call;
  const m: Suggestion | null =
    ai && ["chi", "pon", "kan", "pass"].includes(ai.best.kind) ? ai.best : null;
  let take: { kind: string; tiles: string[]; option: CallOptionView | null } | null = null;
  if (m) {
    if (m.kind !== "pass") {
      const option =
        call?.options.find(
          (o) =>
            o.kind === m.kind &&
            o.with.every((w) => m.consumed.some((c) => sameKind(c, w))),
        ) ?? null;
      take = { kind: m.kind, tiles: m.consumed, option };
    }
  } else if (call && call.take !== null) {
    const o = call.options[call.take];
    take = { kind: o.kind, tiles: o.with, option: o };
  }
  const called = call?.called ?? m?.tile ?? null;
  const stay = call
    ? `不鸣 · ${shantenText(call.stayShanten)} · ${call.stayUkeireCount} 张`
    : "不鸣";
  const alternative = !take && call && call.options.length > 0 ? call.options[0] : null;
  return (
    <div className="card call-card">
      <div className="card-main">
        {called && (
          <div className="from">
            {call?.from ? SEATS[call.from] : ""}
            <Tile tile={called} width={WAIT} />
          </div>
        )}
        <span className="to">→</span>
        {take ? (
          <div className="decision">
            <div className="decision-top">
              <span className="yes">✓</span>
              <span className="verb">{CALLS[take.kind] ?? take.kind}</span>
              <span className="call-tiles">
                {[...take.tiles, ...(called ? [called] : [])].map((t, i) => (
                  <Tile key={i} tile={t} width={UKEIRE} />
                ))}
              </span>
            </div>
            <span className="decision-sub">{take.option ? callLine(take.option) : take.kind === "kan" ? "→ 杠后摸岭上牌" : ""}</span>
          </div>
        ) : (
          <div className="decision">
            <div className="decision-top">
              <span className="no">✗</span>
              <span className="verb">不鸣</span>
            </div>
            <span className="decision-sub">{call ? `${shantenText(call.stayShanten)} · ${call.stayUkeireCount} 张` : ""}</span>
          </div>
        )}
      </div>
      {(take || alternative) && (
        <div className="card-alt">
          {take ? (
            <>
              <span className="no small">✗</span>
              {stay}
            </>
          ) : (
            alternative && (
              <>
                <span className="small">{CALLS[alternative.kind]}</span>
                {alternative.with.map(name).join("")} {callLine(alternative)}
              </>
            )
          )}
        </div>
      )}
      {m && <span className="source">Mortal</span>}
    </div>
  );
}

// The win on offer and what it is worth.
function WinCard({ win, ai }: { win: WinView; ai: MortalView | null }) {
  const declines = ai !== null && ai.best.kind !== "win";
  return (
    <div className="card win-card">
      <div className="card-main">
        <div className="decision">
          <div className="decision-top">
            <span className={declines ? "no" : "yes"}>{declines ? "✗" : "✓"}</span>
            <span className="verb">{win.ron ? "荣和" : "自摸"}</span>
            <span className="points num">{paysText(win)}</span>
          </div>
          <span className="decision-sub">
            {hanText(win)}
            {win.total !== win.points && ` · 加本场和供托共得 ${win.total}`} · 不含里宝牌
          </span>
        </div>
      </div>
      {declines && <div className="card-alt">Mortal 建议先不和</div>}
      {ai && <span className="source">Mortal</span>}
    </div>
  );
}

function Notices({ notes }: { notes: string[] }) {
  return (
    <div className="notices">
      {notes.map((n) => (
        <span key={n}>{n}</span>
      ))}
    </div>
  );
}

function statusNote(status: Status | null, models: string | null): string | null {
  if (models) return models;
  if (!status) return "正在启动";
  switch (status.state) {
    case "noWindow":
      return "未找到雀魂窗口：请在浏览器中打开雀魂";
    case "error":
      return status.message;
    case "watching":
      return null;
  }
}

function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [advice, setAdvice] = useState<Advice | null>(null);
  const [latency, setLatency] = useState<Latency | null>(null);
  const [models, setModels] = useState<string | null>(null);
  const [recording, setRecording] = useState<Recording | null>(null);
  const [engine, setEngine] = useState<EngineView | null>(null);
  const [mortal, setMortal] = useState<MortalView | null>(null);
  const [win, setWin] = useState<WinView | null>(null);
  const [compact, setCompact] = useState(rememberedCompact);
  const panel = useRef<HTMLElement>(null);

  useEffect(() => {
    invoke<Status | null>("pipeline_status").then(setStatus).catch(() => undefined);
    invoke<Recording | null>("debug_recording").then(setRecording).catch(() => undefined);
    const unlisten = [
      listen<Status>("pipeline-status", (e) => setStatus(e.payload)),
      listen<Advice | null>("advice", (e) => setAdvice(e.payload)),
      listen<EngineView>("engine", (e) => setEngine(e.payload)),
      listen<MortalView>("mortal", (e) => setMortal(e.payload)),
      listen<WinView>("win", (e) => setWin(e.payload)),
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

  // The window takes the panel's height, so it never shows empty space or cuts it off.
  useLayoutEffect(() => {
    const el = panel.current;
    if (!el) return;
    let last = 0;
    const fit = () => {
      const height = Math.ceil(el.getBoundingClientRect().height);
      if (height === last || height === 0) return;
      last = height;
      getCurrentWindow()
        .setSize(new LogicalSize(window.innerWidth, height))
        .catch(() => undefined);
    };
    const observer = new ResizeObserver(fit);
    observer.observe(el);
    fit();
    return () => observer.disconnect();
  }, []);

  // Report when Mortal's decision is on screen too, for the latency of call decisions.
  useEffect(() => {
    if (!mortal) return;
    const seq = mortal.seq;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        invoke("advice_shown", { seq, mortal: true }).catch(() => undefined);
      }),
    );
  }, [mortal]);

  // Report when the advice is on screen, for the latency measurement.
  useEffect(() => {
    if (!advice) return;
    const seq = advice.seq;
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        invoke("advice_shown", { seq, mortal: false }).catch(() => undefined);
      }),
    );
  }, [advice]);

  const toggleCompact = () =>
    setCompact((c) => {
      rememberCompact(!c);
      return !c;
    });

  const ai = advice && mortal?.seq === advice.seq ? mortal : null;
  const winNow = advice && win?.seq === advice.seq ? win : null;
  const p = advice ? plan(advice, ai, winNow) : null;
  const game = advice?.game ?? null;
  const gameName = status?.state === "watching" ? status.game : "麻将 Copilot";

  // The tiles a call uses and the discard after it stay bright in the hand.
  let keep: string[] | null = null;
  if (advice && p?.card === "call") {
    const m = ai && ["chi", "pon", "kan"].includes(ai.best.kind) ? ai.best : null;
    const option = advice.call && advice.call.take !== null ? advice.call.options[advice.call.take] : null;
    if (m) {
      const after = advice.call?.options.find((o) => o.kind === m.kind && o.with.every((w) => m.consumed.some((c) => sameKind(c, w))));
      keep = [...m.consumed, ...(after ? [after.discard] : [])];
    } else if (option && !ai) {
      keep = [...option.with, option.discard];
    } else {
      keep = [];
    }
  }

  const notes: string[] = [];
  if (engine?.textFailing) notes.push("文字识别失效，读不到局数和分数，请重启");
  const statusLine = statusNote(status, models);
  if (statusLine && advice) notes.push(statusLine);
  if (engine?.filled) notes.push("这一局是中途开始看的，前面是补出的，Mortal 的建议可能偏弱");
  if (engine && engine.gaps > 0) notes.push(`${engine.gaps} 段没看清，已按画面补上`);
  const doubts = advice?.tiles.filter((v) => v.doubt).length ?? 0;
  if (doubts > 0) notes.push(`${doubts} 张牌拿不准（标 ?）`);

  if (compact) {
    return (
      <main className="panel compact" ref={panel}>
        <div className="compact-row" data-tauri-drag-region>
          <WindowButtons compact={compact} onCompact={toggleCompact} />
          <div className="compact-round" data-tauri-drag-region>
            <b className="num">{game?.round ? `${WINDS[game.round[0]] ?? "?"}${game.round.slice(1)}` : "—"}</b>
            <span>余 {game?.tilesLeft ?? "—"}</span>
          </div>
          {advice && p ? (
            <>
              <Hand advice={advice} p={p} keep={keep} width={COMPACT} />
              <span className={`compact-headline${p.waiting ? " waiting" : ""}`}>
                {p.card === "win" && winNow ? (winNow.ron ? "荣和" : "自摸") : (p.headline ?? "")}
              </span>
            </>
          ) : (
            <span className="placeholder">{statusLine ?? "等待手牌出现"}</span>
          )}
        </div>
      </main>
    );
  }

  const ukeire =
    advice && p
      ? p.option
        ? { shanten: p.option.shanten, tiles: p.option.ukeire }
        : advice.tiles.length % 3 === 1 && advice.shanten !== null
          ? { shanten: advice.shanten, tiles: advice.ukeire }
          : null
      : null;

  return (
    <main className="panel" ref={panel}>
      <header className="titlebar" data-tauri-drag-region>
        <WindowButtons compact={compact} onCompact={toggleCompact} />
      </header>
      <div className="status-bar" data-tauri-drag-region>
        <span className="game" data-tauri-drag-region>
          {gameName}
        </span>
        {game && roundText(game) && (
          <span className="strong num" data-tauri-drag-region>
            {roundText(game)}
          </span>
        )}
        {game?.seatWind && <span data-tauri-drag-region>自风 {WINDS[game.seatWind] ?? "?"}</span>}
        {game?.tilesLeft !== null && game?.tilesLeft !== undefined && (
          <span data-tauri-drag-region>
            余 <b className="strong num">{game.tilesLeft}</b>
          </span>
        )}
        {advice && advice.dora.length > 0 && (
          <span className="dora" data-tauri-drag-region>
            宝牌指示
            {advice.dora.map((d, i) => (
              <Tile key={i} tile={d.tile} width={SMALL} doubt={d.doubt !== null} />
            ))}
          </span>
        )}
      </div>

      {advice && p ? (
        <>
          <section className="hand-area">
            <Hand advice={advice} p={p} keep={keep} width={HAND} />
            {p.headline && (
              <div className="headline-row">
                <span className={`headline${p.waiting ? " waiting" : ""}`}>{p.headline}</span>
                <span className="headline-sub">
                  {p.waiting
                    ? "等待你的决定"
                    : p.option && (
                        <>
                          {shantenText(p.option.shanten)} · 进张 <b className="strong">{p.option.ukeireCount}</b>
                        </>
                      )}
                </span>
                {p.discard && p.others.length > 0 && (
                  <span className="legend">
                    <span className="second">▬ 候选2</span>
                    {p.others.length > 1 && <span className="third">▬ 候选3</span>}
                  </span>
                )}
              </div>
            )}
          </section>
          {(ukeire || p.card) && (
            <section className={`lower${p.card ? " covered" : ""}`}>
              {ukeire && ukeire.tiles.length > 0 && (
                <div className="lower-content">
                  <Ukeire shanten={ukeire.shanten} tiles={ukeire.tiles} />
                </div>
              )}
              {p.card === "call" && <CallCard advice={advice} ai={ai} />}
              {p.card === "win" && winNow && <WinCard win={winNow} ai={ai} />}
            </section>
          )}
        </>
      ) : (
        <p className="placeholder">{statusLine ?? "等待手牌出现"}</p>
      )}

      <footer className="footer">
        <Notices notes={notes} />
        <span className="num hint">⌥ Space 隐藏</span>
      </footer>

      {recording && <DevArea initial={recording} advice={advice} engine={engine} latency={latency} ai={ai} />}
    </main>
  );
}

export default App;
