// The dev build's debug area: the tools and a one-line status always in sight, then
// folded sections (latency, the game as read, the game log's latest events, the table as
// read, the version and models), each opened on demand and remembered. A packaged release
// app has no debug recording, so it never shows this.

import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { name, SEAT_ORDER } from "./format";
import { ALL_TILES, tileLabel, tilePicture } from "./tile";
import type { Advice, BuildInfo, EngineView, Latency, MortalView, Recording, TableTile, TableView } from "./types";

// From the screen going still to the advice shown (docs/design.md).
const BUDGET_MS = 300;

const SCENES: Record<EngineView["last"], string> = {
  taken: "跟上",
  covered: "画面被挡住",
  unexplained: "对不上",
  impossible: "读错了",
};

// Whether a section is open, kept per viewer; the panel works without it.
function remembered(key: string): boolean {
  try {
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

function remember(key: string, open: boolean) {
  try {
    localStorage.setItem(key, open ? "1" : "0");
  } catch {
    // Storage is a convenience only.
  }
}

// A section folded until opened; whether it is open is remembered.
function Fold({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  const key = `dev-open-${id}`;
  const [open, setOpen] = useState(() => remembered(key));
  const toggle = () =>
    setOpen((o) => {
      remember(key, !o);
      return !o;
    });
  return (
    <>
      <button type="button" className="dev-sub" onClick={toggle} aria-expanded={open}>
        {open ? "▾" : "▸"} {title}
      </button>
      {open && children}
    </>
  );
}

function short(mjai: string): string {
  const label = tileLabel(mjai);
  return `${label.rank}${label.suit}${mjai.endsWith("r") ? "赤" : ""}`;
}

function tableText(t: TableTile): string {
  const face = t.tile === null ? "▮" : short(t.tile);
  return `${face}${t.sideways ? "↻" : ""}${t.added ? `+${short(t.added)}` : ""}${t.doubt ? "?" : ""}`;
}

function Bar({ label, ms, max }: { label: string; ms: number; max: number }) {
  const width = max > 0 ? Math.min(100, (100 * ms) / max) : 0;
  const tone = ms < 5 ? "fast" : ms >= max * 0.99 ? "slowest" : "";
  return (
    <div className="dev-bar">
      <span>{label}</span>
      <div className="track">
        <div className={`fill ${tone}`} style={{ width: `${width}%` }} />
      </div>
      <span className="num">{ms.toFixed(ms < 10 ? 1 : 0)} ms</span>
    </div>
  );
}

function Row({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="dev-row" title={title}>
      <span>{label}</span>
      <span className="num">{value}</span>
    </div>
  );
}

// Every seat's river and melds as read, for debugging the table reading.
function TableDebug({ table }: { table: TableView }) {
  return (
    <div className="table-debug">
      <p className="legend">↻ 横放（被鸣的牌、立直宣言牌） · +X 加杠 · ▮ 扣着的牌 · ? 拿不准</p>
      {SEAT_ORDER.map((seat, i) => (
        <p key={seat}>
          <span className="seat">{seat}</span>
          <span>河 {table.rivers[i].map(tableText).join(" ") || "—"}</span>
          {table.melds[i].length > 0 && (
            <span> ｜ 副露 {table.melds[i].map((m) => `[${m.map(tableText).join(" ")}]`).join(" ")}</span>
          )}
        </p>
      ))}
    </div>
  );
}

// Every tile picture with its mjai name, to check them in both themes.
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

type Props = {
  initial: Recording;
  advice: Advice | null;
  engine: EngineView | null;
  latency: Latency | null;
  ai: MortalView | null;
};

export function DevArea({ initial, advice, engine, latency, ai }: Props) {
  const [recording, setRecording] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  // The last screenshot's file, confirmed (#80).
  const [shot, setShot] = useState<string | null>(null);
  const [sheet, setSheet] = useState(false);
  const [version, setVersion] = useState("");
  const [build, setBuild] = useState<BuildInfo | null>(null);

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => undefined);
    // Each saved frame also refreshes how many the recording dropped.
    const unlisten = listen<number>("frame-saved", (e) => {
      setRecording((r) => ({ ...r, saved: e.payload }));
      invoke<Recording | null>("debug_recording")
        .then((r) => r && setRecording(r))
        .catch(() => undefined);
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  }, []);

  // The models load once downloaded and the pipeline starts: asked again when its status
  // changes.
  useEffect(() => {
    const ask = () =>
      invoke<BuildInfo>("build_info")
        .then(setBuild)
        .catch(() => undefined);
    ask();
    const unlisten = [listen("pipeline-status", ask), listen("models-ready", ask)];
    return () => {
      unlisten.forEach((p) => p.then((stop) => stop()));
    };
  }, []);

  // Shows the first-start guide again (#17), to check it on a machine already set up.
  const replayOnboarding = () => {
    try {
      localStorage.removeItem("onboarded");
    } catch {
      // Without storage the guide's last step shows every time anyway.
    }
    window.location.reload();
  };
  const report = (err: unknown) => setError(String(err));
  const shoot = () => {
    setError(null);
    setShot(null);
    invoke<string>("save_frame")
      .then((file) => {
        setShot(file);
        // The dropped count lives with the recording state.
        return invoke<Recording | null>("debug_recording");
      })
      .then((r) => r && setRecording(r))
      .catch(report);
  };
  const record = (on: boolean) => {
    setError(null);
    invoke<Recording>("set_recording", { on }).then(setRecording).catch(report);
  };
  const openFolder = () => {
    setError(null);
    invoke("open_dev_folder").catch(report);
  };

  const t = advice?.times;
  const render = t && latency ? Math.max(0, latency.lastMs - t.sentMs) : null;
  const steps: [string, number][] = t
    ? [
        ["稳定帧确认", t.settleMs],
        ["找手牌", t.findMs],
        ["认手牌", t.recognizeMs],
        ["读牌桌", t.tableMs],
        ["计算", t.decideMs],
        ...(render !== null ? [["界面渲染", render] as [string, number]] : []),
      ]
    : [];
  const slowest = Math.max(0, ...steps.map(([, ms]) => ms));
  const doubts = advice?.tiles.filter((v) => v.doubt).length ?? 0;
  const game = advice?.game;
  const scores = game?.scores ?? null;

  const mortal = build?.models.find((m) => m.loaded !== null)?.loaded ?? null;
  const summary = [
    `最后一帧 ${engine ? SCENES[engine.last] : "—"}`,
    ...(engine && engine.stuck > 0 ? [`连续 ${engine.stuck} 帧没跟上`] : []),
    latency ? `延迟 ${latency.lastMs.toFixed(0)} ms · P95 ${latency.p95Ms.toFixed(0)}` : "延迟 —",
    mortal === null ? "Mortal —" : mortal ? "Mortal 已加载" : "Mortal 未加载",
    ...(doubts > 0 ? [`${doubts} 张拿不准`] : []),
  ];

  return (
    <section className="dev">
      <div className="dev-head">
        <span className="dev-badge">DEV</span>
        <span>开发版 · 调试数据</span>
        <span className="num dev-version">
          {version && `v${version}-dev`}
          {build && ` · ${build.commit}`}
        </span>
      </div>

      <div className="dev-tools">
        <button type="button" onClick={shoot}>
          截图
        </button>
        <button type="button" className={recording.on ? "on" : ""} onClick={() => record(!recording.on)}>
          连续录制
        </button>
        <button type="button" className={sheet ? "on" : ""} onClick={() => setSheet((s) => !s)}>
          牌图
        </button>
        <button type="button" onClick={replayOnboarding}>
          重看引导
        </button>
        <button type="button" onClick={openFolder} title={`截图、日志和牌谱都在这里：${recording.dir.replace(/\/live$/, "")}`}>
          打开文件夹
        </button>
        <span className="dev-saved" title={recording.dir}>
          已存 {recording.saved} 张{recording.dropped > 0 ? ` · 丢了 ${recording.dropped} 张` : ""}
        </span>
      </div>
      {shot && (
        <p className="dev-shot num" title={shot}>
          已存：{shot.split("/").pop()}
        </p>
      )}
      {error && <p className="dev-error">截图或录制失败：{error}</p>}
      {sheet && <TileSheet />}

      <p className="dev-summary num">{summary.join(" · ")}</p>

      <Fold id="latency" title="延迟 · 稳定帧 → 建议显示">
        {latency ? (
          <div className="dev-latency">
            <span className="big num">
              {latency.lastMs.toFixed(0)}
              <small> ms</small>
            </span>
            <span className="num">
              P50 {latency.p50Ms.toFixed(0)} · P95 {latency.p95Ms.toFixed(0)} · 预算 {BUDGET_MS}（{latency.count} 次）
            </span>
            {latency.buttons && (
              <span className="num">
                有按钮时 P95 {latency.buttons.p95Ms.toFixed(0)}（{latency.buttons.count} 次）
                {latency.buttonsMortal &&
                  ` · Mortal P95 ${latency.buttonsMortal.p95Ms.toFixed(0)}（${latency.buttonsMortal.count} 次）`}
              </span>
            )}
          </div>
        ) : (
          <p className="dev-empty">还没有数据</p>
        )}
        {steps.length > 0 && (
          <div className="dev-bars">
            {steps.map(([label, ms]) => (
              <Bar key={label} label={label} ms={ms} max={slowest} />
            ))}
            {ai && <Bar label="Mortal" ms={ai.decideMs} max={slowest} />}
          </div>
        )}
      </Fold>

      <Fold id="game" title="牌局 · 识别状态和分数">
        <div className="dev-grid">
          <Row label="最后一帧" value={engine ? SCENES[engine.last] : "—"} />
          <Row label="不确定牌" value={`${doubts}`} />
          <Row
            label="宝牌指示"
            value={`${advice?.dora.map((d) => short(d.tile)).join(" ") || "—"}${advice?.seen !== null && advice?.seen !== undefined ? ` · 已见 ${advice.seen} 张` : ""}`}
          />
          <Row label="供托 / 本场" value={`${game?.riichiSticks ?? "—"} / ${game?.honba ?? "—"}`} />
          <Row label="牌谱事件" value={engine ? `${engine.events}` : "—"} />
          <Row label="补出 / 缺口" value={engine ? `${engine.filled ? "是" : "否"} / ${engine.gaps}` : "—"} />
        </div>
        {advice && doubts > 0 && (
          <p className="dev-empty">
            拿不准：{advice.tiles.filter((v) => v.doubt).map((v) => name(v.tile)).join(" ")}
          </p>
        )}
        {scores && (
          <>
            <h3>四家分数 · 合计 {scores.reduce((a, b) => a + b, 0)}</h3>
            <div className="dev-scores">
              {SEAT_ORDER.map((seat, i) => (
                <div key={seat}>
                  <span>{seat}</span>
                  <b className="num">{scores[i]}</b>
                </div>
              ))}
            </div>
          </>
        )}
      </Fold>

      <Fold id="events" title="牌谱事件流 · mjai">
        {engine && engine.recent.length > 0 ? (
          <div className="dev-events num">
            {engine.recent.map((line) => (
              <div key={line}>{line}</div>
            ))}
          </div>
        ) : (
          <p className="dev-empty">还没有事件</p>
        )}
      </Fold>

      <Fold id="table" title="读牌视图 · 四家牌河和副露">
        {advice?.table ? <TableDebug table={advice.table} /> : <p className="dev-empty">还没有读到牌桌</p>}
      </Fold>

      <Fold id="build" title="版本与模型">
        {build ? (
          <div className="dev-grid">
            <Row label="版本" value={`v${build.version} · ${build.commit} · ${build.profile}`} />
            {build.models.map((m) => (
              <Row
                key={m.file}
                label={`${m.role}模型`}
                title={`SHA-256 ${m.sha256}`}
                value={`${m.file} · 修订 ${m.revision.slice(0, 7)} · SHA-256 ${m.sha256.slice(0, 12)}… · ${(m.size / 1e6).toFixed(1)} MB · ${m.present ? "已下载" : "未下载"}${m.loaded === null ? "" : m.loaded ? " · 已加载" : " · 未加载"}`}
              />
            ))}
          </div>
        ) : (
          <p className="dev-empty">读不到版本信息</p>
        )}
      </Fold>
    </section>
  );
}
