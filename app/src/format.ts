// Text for the panel. Rendering only: the advice itself comes from the pipeline.

import { tileLabel } from "./tile";
import type { GameView, WinView } from "./types";

// 5萬 · 3筒（赤） · 北
export function name(mjai: string): string {
  const label = tileLabel(mjai);
  return `${label.rank}${label.suit}${mjai.endsWith("r") ? "（赤）" : ""}`;
}

export function shantenText(shanten: number): string {
  if (shanten < 0) return "和了";
  if (shanten === 0) return "听牌";
  return `${shanten} 向听`;
}

export const WINDS: Record<string, string> = { E: "東", S: "南", W: "西", N: "北" };

// 東2局 0本場, leaving out what was not read.
export function roundText(game: GameView): string {
  const round = game.round ? `${WINDS[game.round[0]] ?? "?"}${game.round.slice(1)}局` : "";
  const honba = game.honba !== null ? ` ${game.honba}本場` : "";
  return `${round}${honba}`.trim();
}

// Seats as the call card names them.
export const SEATS: Record<string, string> = { right: "下家", across: "对家", left: "上家" };
// Seats in turn order from the player, as the table view lists them.
export const SEAT_ORDER = ["自家", "下家", "对家", "上家"];

export const CALLS: Record<string, string> = { chi: "吃", pon: "碰", kan: "杠" };
export const YAKU: Record<string, string> = {
  yakuhai: "役牌",
  tanyao: "断幺",
  honitsu: "混一色",
  toitoi: "对对和",
};

// 3 番 40 符 · 役满 · 2 倍役满
export function hanText(w: WinView): string {
  if (w.yakuman > 0) return `${w.yakuman > 1 ? `${w.yakuman} 倍` : ""}役满`;
  return `${w.han} 番${w.fu > 0 ? ` ${w.fu} 符` : ""}`;
}

// 5200 · 1300/2600 · 2600 all, then the total with the honba and riichi sticks.
export function paysText(w: WinView): string {
  if (w.ron) return `${w.points}`;
  return w.dealerPays > 0 ? `${w.dealerPays}/${w.othersPay}` : `各 ${w.othersPay}`;
}

// The same tile, red five or not.
export function sameKind(a: string, b: string): boolean {
  return a.replace("r", "") === b.replace("r", "");
}
