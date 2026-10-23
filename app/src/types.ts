// Payloads sent by app/src-tauri/src/pipeline.rs; rendering only.

export type Status =
  | { state: "noWindow" }
  | { state: "watching"; app: string; title: string; game: string }
  | { state: "error"; message: string };

export type TileView = { tile: string; doubt: "small" | "unsure" | null };
// A tile on the table as read; `tile` is null for a tile face down.
export type TableTile = { tile: string | null; sideways: boolean; added: string | null; doubt: boolean };
export type TableView = { rivers: TableTile[][]; melds: TableTile[][][] };
// The round, the player's wind and the numbers on the table, each null when not read, and
// the action buttons shown, left to right.
export type GameView = {
  round: string | null;
  seatWind: string | null;
  scores: number[] | null;
  tilesLeft: number | null;
  honba: number | null;
  riichiSticks: number | null;
  buttons: string[];
};
// A tile that improves the hand (or wins it), and how many copies are still unseen.
export type TileLeft = { tile: string; left: number };
export type OptionView = { discard: string; shanten: number; ukeire: TileLeft[]; ukeireCount: number };
// One way to call (#11): the two tiles from the hand, the discard after it and where the
// hand then stands, and the yaku still within reach.
export type CallOptionView = {
  kind: "chi" | "pon";
  with: string[];
  discard: string;
  shanten: number;
  ukeireCount: number;
  yaku: string[];
};
// What the state engine (#10) made of the table: the current game's log so far.
export type EngineView = {
  events: number;
  round: string | null;
  honba: number | null;
  filled: boolean;
  gaps: number;
  last: "taken" | "covered" | "unexplained" | "impossible";
  recent: string[];
};
// Whether and how to call the discard on offer; `take` is null to let it go. `from` is
// the seat that discarded it, null when unsure.
export type CallView = {
  called: string;
  from: "right" | "across" | "left" | null;
  stayShanten: number;
  stayUkeireCount: number;
  options: CallOptionView[];
  take: number | null;
};
export type Advice = {
  seq: number;
  tiles: TileView[];
  drawn: boolean;
  // Index into `tiles` of the rule layer's discard, for a hand that must discard.
  discard: number | null;
  // Every discard the rule layer ranked, best first.
  options: OptionView[];
  shanten: number | null;
  ukeire: TileLeft[];
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
export type Suggestion = {
  kind: "discard" | "riichi" | "chi" | "pon" | "kan" | "win" | "draw" | "pass";
  tile: string | null;
  consumed: string[];
  q: number;
};
// Mortal's decision for the advice with the same `seq`.
export type MortalView = {
  seq: number;
  best: Suggestion;
  riichiDiscard: Suggestion | null;
  discards: Suggestion[];
  decideMs: number;
};
// What the win on offer is worth (#11), for the advice with the same `seq`. `fu` is 0
// from 5 han on, `han` and `fu` are 0 for a yakuman; ura dora are not counted.
export type WinView = {
  seq: number;
  ron: boolean;
  han: number;
  fu: number;
  yakuman: number;
  points: number;
  dealerPays: number;
  othersPay: number;
  total: number;
};
// Still screen to shown: all advice, and on frames with action buttons the advice and
// Mortal's decision (null until there is one).
export type Spread = { count: number; p50Ms: number; p95Ms: number };
export type Latency = {
  count: number;
  lastMs: number;
  p50Ms: number;
  p95Ms: number;
  buttons: Spread | null;
  buttonsMortal: Spread | null;
};
export type Progress = { name: string; downloaded: number; total: number };
// The dev build's debug recording (#8); null in a packaged app.
export type Recording = { on: boolean; saved: number; dir: string };
