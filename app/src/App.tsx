import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { DevArea } from "./DevArea";
import { Onboarding } from "./Onboarding";
import {
  CALLS,
  directionText,
  LEVELS,
  REASONS,
  threatText,
  VERDICTS,
  hanText,
  name,
  paysText,
  roundText,
  sameKind,
  SEATS,
  shantenText,
  valueText,
  WINDS,
  YAKU,
  yakuText,
} from "./format";
import { Tile } from "./TileImage";
import { ALL_TILES } from "./tile";
import type {
  Advice,
  CallOptionView,
  DefenseView,
  EngineView,
  Latency,
  MortalView,
  OptionView,
  Progress,
  Recording,
  Status,
  Suggestion,
  TableTile,
  TargetsView,
  TargetView,
  TileLeft,
  WaitValue,
  WinView,
} from "./types";
import "./App.css";

// Tile widths in px: the hand, the compact row, the status bar's dora, waits and the call card.
const HAND = 26;
const COMPACT = 22;
const SMALL = 16;
const WAIT = 34;
const UKEIRE = 22;
const TARGET = 19;
// Meld tiles beside the hand, in the full panel and in compact mode.
const MELD = 18;
const MELD_COMPACT = 15;

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

// The panel's look: following the system (the default), light or dark.
type Theme = "system" | "light" | "dark";
const THEMES: Theme[] = ["system", "light", "dark"];
const THEME_NAMES: Record<Theme, string> = { system: "跟随系统", light: "浅色", dark: "深色" };
const THEME_ICONS: Record<Theme, string> = { system: "◐", light: "☀", dark: "☾" };

function rememberedTheme(): Theme {
  try {
    const t = localStorage.getItem("theme");
    return t === "light" || t === "dark" ? t : "system";
  } catch {
    return "system";
  }
}

// Keeps the chosen theme, and applies it (or the system's) as `data-theme` on the page.
function useTheme(): [Theme, () => void] {
  const [theme, setTheme] = useState<Theme>(rememberedTheme);
  useEffect(() => {
    const system = window.matchMedia("(prefers-color-scheme: light)");
    const apply = () => {
      const light = theme === "light" || (theme === "system" && system.matches);
      document.documentElement.dataset.theme = light ? "light" : "dark";
    };
    apply();
    system.addEventListener("change", apply);
    return () => system.removeEventListener("change", apply);
  }, [theme]);
  const next = () =>
    setTheme((t) => {
      const n = THEMES[(THEMES.indexOf(t) + 1) % THEMES.length];
      try {
        localStorage.setItem("theme", n);
      } catch {
        // Storage is a convenience only.
      }
      return n;
    });
  return [theme, next];
}

function ThemeButton({ theme, onNext }: { theme: Theme; onNext: () => void }) {
  const next = THEMES[(THEMES.indexOf(theme) + 1) % THEMES.length];
  return (
    <button
      type="button"
      className="theme-button"
      aria-label={`外观：${THEME_NAMES[theme]}`}
      title={`外观：${THEME_NAMES[theme]}（点击切换到${THEME_NAMES[next]}）`}
      onClick={onNext}
    >
      {THEME_ICONS[theme]}
    </button>
  );
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

// Close and minimize as in any app; a floating panel has nothing to zoom, so the green
// button is greyed out (#59).
function WindowButtons() {
  const window = getCurrentWindow();
  return (
    <span className="window-buttons">
      <button type="button" className="close" aria-label="关闭" title="关闭" onClick={() => window.close()} />
      <button type="button" className="minimize" aria-label="最小化" title="最小化" onClick={() => window.minimize()} />
      <button type="button" className="zoom" aria-label="不可放大" title="面板不能放大" disabled />
    </span>
  );
}

// Switches the full panel to the one-row compact mode (design 1e, #59); in compact mode an
// icon at the title bar's right switches back.
function CompactButton({ onToggle }: { onToggle: () => void }) {
  return (
    <button type="button" className="title-button compact-button" title="紧凑模式：只留一行" onClick={onToggle}>
      ▭ 紧凑
    </button>
  );
}

// The player's own melds, beside the hand: sideways tiles lie turned. They come in the order
// they were made, the first at the right on the table, so they are drawn reversed.
function OwnMelds({ melds, width }: { melds: TableTile[][]; width: number }) {
  // The sideways tiles' box and the added tile's place follow the tile width.
  const size = { "--meld": `${width}px` } as React.CSSProperties;
  return (
    <div className="melds" style={size}>
      {[...melds].reverse().map((meld, i) => (
        <div key={i} className="meld">
          {meld.map((t, j) => (
            <span key={j} className={`meld-tile${t.sideways ? " sideways" : ""}`}>
              {t.added && <Tile tile={t.added} width={width} />}
              <Tile tile={t.tile} width={width} />
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
  defense = null,
  onPick,
}: {
  advice: Advice;
  p: Plan;
  keep: string[] | null;
  width: number;
  defense?: DefenseView | null;
  onPick?: (index: number) => void;
}) {
  const dangerOf = (tile: string) => {
    const d = defense?.tiles.find((t) => sameKind(t.tile, tile));
    if (!d) return null;
    const why = `${LEVELS[d.level] ?? d.level}（${REASONS[d.reason] ?? d.reason}${d.dora ? "，宝牌附近" : ""}）`;
    return { level: d.level, why };
  };
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
            danger={dangerOf(view.tile)}
            onClick={onPick ? () => onPick(i) : undefined}
          />
        </span>
      ))}
      {melds.length > 0 && <OwnMelds melds={melds} width={width === COMPACT ? MELD_COMPACT : MELD} />}
    </div>
  );
}

// The tiles that improve the hand, or the waits once it is tenpai, with how many are left.
function Ukeire({ shanten, tiles, values }: { shanten: number; tiles: TileLeft[]; values: WaitValue[] }) {
  const total = tiles.reduce((n, t) => n + t.left, 0);
  const valueOf = (tile: string) => values.find((v) => sameKind(v.tile, tile)) ?? null;
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
              <div className="wait-body">
                {valueOf(t.tile) && (
                  <div className="wait-value">
                    <span className="wait-yaku">{yakuText(valueOf(t.tile)?.yaku ?? []) || "无役"}</span>
                    <span className="num wait-points">{valueOf(t.tile)?.points}</span>
                  </div>
                )}
                <div className="wait-left">
                  剩 <b className="num">{t.left}</b> 张
                  <span className="pips">
                    {[0, 1, 2, 3].map((k) => (
                      <i key={k} className={k < t.left ? "on" : ""} />
                    ))}
                  </span>
                </div>
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

// A target hand's tiles, the ones still to draw drawn faint in a dashed outline.
function TargetTiles({ t }: { t: TargetView }) {
  const missing = [...t.missing];
  // The tiles still to draw are the last copies of their kind in the finished hand.
  const marks = t.tiles.map(() => false);
  for (let i = t.tiles.length - 1; i >= 0; i--) {
    const at = missing.findIndex((m) => sameKind(m, t.tiles[i]));
    if (at >= 0) {
      marks[i] = true;
      missing.splice(at, 1);
    }
  }
  return (
    <div className="target-tiles">
      {t.tiles.map((tile, i) => (
        <Tile key={i} tile={tile} width={TARGET} missing={marks[i]} />
      ))}
    </div>
  );
}

// Up to three target hands (#13): yaku, points, a rough chance and the finished hand;
// or broad directions when the hand is far from tenpai.
function Targets({ view }: { view: TargetsView }) {
  if (view.targets.length === 0) {
    return (
      <>
        <h2>目标方向 · 离听牌还远</h2>
        <div className="directions">
          {view.directions.map((d) => (
            <span key={directionText(d)}>{directionText(d)}</span>
          ))}
        </div>
      </>
    );
  }
  return (
    <>
      <h2>目标牌型{view.rough && " · 离听牌还远，仅供参考"}</h2>
      {view.targets.map((t, i) => {
        const percent = Math.round(100 * t.probability);
        return (
          <div key={i} className="target">
            <div className="target-head">
              <span className="target-key num">{"ABC"[i]}</span>
              <span className="target-name">{yakuText(t.yaku) || "宝牌"}</span>
              <span className="target-value">{valueText(t)}</span>
              <span className="num target-points">{t.points}</span>
              <span className="chance" title="按剩余牌数和摸牌次数粗略估算">
                <i style={{ width: `${Math.max(2, percent)}%` }} />
              </span>
              <span className="num target-percent">约 {percent}%</span>
            </div>
            <TargetTiles t={t} />
          </div>
        );
      })}
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
    case "paused":
      return "识别已暂停：点标题栏的「▶ 开始识别」继续";
    case "watching":
      return null;
  }
}

// The status light (#16): whether the advice can be trusted now, and why.
type Light = { color: "green" | "yellow" | "red" | "gray"; why: string[] };

// Scenes in a row the engine may miss (an animation, a hand covering the table) before the
// advice counts as not following the game.
const STUCK = 3;

function light(status: Status | null, advice: Advice | null, engine: EngineView | null): Light {
  if (!status || status.state === "paused") return { color: "gray", why: ["识别已暂停"] };
  if (status.state === "noWindow") return { color: "red", why: ["找不到雀魂窗口"] };
  if (status.state === "error") return { color: "red", why: [status.message] };
  if (!advice) return { color: "gray", why: ["等待手牌出现"] };
  const red: string[] = [];
  if (engine?.textFailing) red.push("文字识别失效，读不到局数和分数");
  if (engine && engine.stuck >= STUCK) red.push(`连续 ${engine.stuck} 帧没跟上牌局，只有牌效建议`);
  if (red.length > 0) return { color: "red", why: red };
  const yellow: string[] = [];
  const doubts = advice.tiles.filter((v) => v.doubt).length;
  if (doubts > 0) yellow.push(`${doubts} 张牌拿不准`);
  if (engine?.filled) yellow.push("这一局是中途开始看的");
  if (engine && engine.stuck > 0) yellow.push("这一帧没对上");
  if (yellow.length > 0) return { color: "yellow", why: yellow };
  return { color: "green", why: ["识别正常，跟上了牌局"] };
}

function StatusLight({ l }: { l: Light }) {
  return <span className={`status-light light-${l.color}`} title={l.why.join("；")} aria-label={l.why.join("；")} />;
}

function PauseButton({ paused, onToggle }: { paused: boolean; onToggle: () => void }) {
  return (
    <button
      type="button"
      className={`title-button pause-button${paused ? " paused" : ""}`}
      title={paused ? "开始识别（牌局开始时）" : "暂停识别（牌局结束后）：不截画面、不录制、不记牌谱"}
      onClick={onToggle}
    >
      {paused ? "▶ 开始识别" : "⏸ 暂停识别"}
    </button>
  );
}

// Correcting a hand tile (#16): the classifier's next guesses first, then every tile.
function TilePicker({
  current,
  alternatives,
  onPick,
  onCancel,
}: {
  current: string;
  alternatives: string[];
  onPick: (tile: string) => void;
  onCancel: () => void;
}) {
  return (
    <div className="picker">
      <div className="picker-head">
        <span>
          把 <b>{name(current)}</b> 改成
        </span>
        <button type="button" className="picker-close" onClick={onCancel}>
          取消
        </button>
      </div>
      {alternatives.length > 0 && (
        <div className="picker-row picker-likely">
          <span className="picker-label">可能是</span>
          {alternatives.map((t) => (
            <Tile key={t} tile={t} width={26} onClick={() => onPick(t)} />
          ))}
        </div>
      )}
      <div className="picker-grid">
        {ALL_TILES.filter((t) => t !== current).map((t) => (
          <Tile key={t} tile={t} width={22} onClick={() => onPick(t)} />
        ))}
      </div>
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
  const [win, setWin] = useState<WinView | null>(null);
  const [targets, setTargets] = useState<TargetsView | null>(null);
  const [defense, setDefense] = useState<DefenseView | null>(null);
  const [compact, setCompact] = useState(rememberedCompact);
  const [theme, nextTheme] = useTheme();
  // First start (#17): the guide until screen recording, the models and the game window
  // are in place.
  const [onboarded, setOnboarded] = useState(false);
  const finishOnboarding = useCallback(() => setOnboarded(true), []);
  // The hand tile being corrected, by its place (#16).
  const [picking, setPicking] = useState<number | null>(null);
  const [pausing, setPausing] = useState<boolean | null>(null);
  const panel = useRef<HTMLElement>(null);

  useEffect(() => {
    invoke<Status | null>("pipeline_status").then(setStatus).catch(() => undefined);
    invoke<Recording | null>("debug_recording").then(setRecording).catch(() => undefined);
    const unlisten = [
      listen<Status>("pipeline-status", (e) => {
        setStatus(e.payload);
        setPausing(null);
      }),
      listen<Advice | null>("advice", (e) => setAdvice(e.payload)),
      listen<EngineView>("engine", (e) => setEngine(e.payload)),
      listen<MortalView>("mortal", (e) => setMortal(e.payload)),
      listen<WinView>("win", (e) => setWin(e.payload)),
      listen<TargetsView>("targets", (e) => setTargets(e.payload)),
      listen<DefenseView>("defense", (e) => setDefense(e.payload)),
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

  // The window takes the panel's height, so it never shows empty space or cuts it off. The
  // panel is a different element in compact mode and during the first-start guide, so the
  // observer follows whichever is on screen.
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
  }, [compact, onboarded]);

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
  const aims = advice && targets?.seq === advice.seq ? targets : null;
  const guard = advice && defense?.seq === advice.seq && defense.threats.length > 0 ? defense : null;
  // The target hands take the lower area when there are any; a tenpai hand waiting gets
  // its waits instead.
  const showTargets = aims !== null && (aims.targets.length > 0 || aims.directions.length > 0);
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

  const paused = pausing ?? status?.state === "paused";
  const togglePause = () => {
    const next = !paused;
    setPausing(next);
    invoke("set_paused", { paused: next }).catch(() => setPausing(null));
  };
  const pick = (tile: string) => {
    if (picking === null) return;
    invoke("correct_tile", { index: picking, tile }).catch(() => undefined);
    setPicking(null);
  };
  const lamp = light(paused ? { state: "paused" } : status, advice, engine);

  const notes: string[] = [];
  if (advice?.tiles.some((v) => v.doubt === "small")) {
    notes.push("游戏窗口太小，有的牌认不准：请把雀魂窗口的宽度调到至少约 960 像素");
  }
  if (engine?.textFailing) notes.push("文字识别失效，读不到局数和分数，请重启");
  const statusLine = statusNote(status, models);
  if (statusLine && advice) notes.push(statusLine);
  if (engine?.filled) notes.push("这一局是中途开始看的，前面是补出的，Mortal 的建议可能偏弱");
  if (engine && engine.gaps > 0) notes.push(`${engine.gaps} 段没看清，已按画面补上`);
  const doubts = advice?.tiles.filter((v) => v.doubt).length ?? 0;
  if (doubts > 0) notes.push(`${doubts} 张牌拿不准（标 ?）`);

  if (compact && onboarded) {
    return (
      <main className="panel compact" ref={panel}>
        <header className="titlebar" data-tauri-drag-region>
          <WindowButtons />
          <button
            type="button"
            className="icon-button expand-button"
            aria-label="展开为完整面板"
            title="展开为完整面板"
            onClick={toggleCompact}
          >
            ⤢
          </button>
        </header>
        <div className="compact-row" data-tauri-drag-region>
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
        <WindowButtons />
        <PauseButton paused={paused} onToggle={togglePause} />
        <CompactButton onToggle={toggleCompact} />
        <ThemeButton theme={theme} onNext={nextTheme} />
      </header>
      {!onboarded && <Onboarding onDone={finishOnboarding} />}
      {onboarded && (
      <div className="status-bar" data-tauri-drag-region>
        <StatusLight l={lamp} />
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
      )}

      {!onboarded ? null : advice && p ? (
        <>
          <section className="hand-area">
            <Hand advice={advice} p={p} keep={keep} width={HAND} defense={guard} onPick={setPicking} />
            {picking !== null && advice.tiles[picking] && (
              <TilePicker
                current={advice.tiles[picking].tile}
                alternatives={advice.tiles[picking].alternatives}
                onPick={pick}
                onCancel={() => setPicking(null)}
              />
            )}
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
                {lamp.color === "red" && !ai && <span className="rules-only">只按牌效：引擎没跟上牌局</span>}
                {p.discard && p.others.length > 0 && (
                  <span className="legend">
                    <span className="second">▬ 候选2</span>
                    {p.others.length > 1 && <span className="third">▬ 候选3</span>}
                  </span>
                )}
              </div>
            )}
            {guard && (
              <div className={`defense-line verdict-${guard.verdict ?? "none"}`}>
                <span className="threats">{threatText(guard.threats)}</span>
                {guard.verdict && <b>建议{VERDICTS[guard.verdict]}</b>}
                {guard.safest && (guard.verdict === "fold" || guard.verdict === "careful") && (
                  <span>最安全：{name(guard.safest)}</span>
                )}
                <span className="danger-legend">
                  <i className="danger-safe" />
                  安全
                  <i className="danger-somewhat" />
                  有些危险
                  <i className="danger-dangerous" />
                  危险
                </span>
              </div>
            )}
          </section>
          {(ukeire || showTargets || p.card) && (
            <section className={`lower${p.card ? " covered" : ""}`}>
              {showTargets && aims ? (
                <div className="lower-content">
                  <Targets view={aims} />
                </div>
              ) : (
                ukeire &&
                ukeire.tiles.length > 0 && (
                  <div className="lower-content">
                    <Ukeire shanten={ukeire.shanten} tiles={ukeire.tiles} values={aims?.waits ?? []} />
                  </div>
                )
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
