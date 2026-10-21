// Display labels for mjai tile strings. Rendering only; tile logic lives in mahjong-core.

const SUITS: Record<string, string> = { m: "万", p: "筒", s: "索" };
const HONORS: Record<string, string> = {
  E: "东",
  S: "南",
  W: "西",
  N: "北",
  P: "白",
  F: "发",
  C: "中",
};

export type TileLabel = { rank: string; suit: string; red: boolean };

export function tileLabel(mjai: string): TileLabel {
  const honor = HONORS[mjai];
  if (honor) return { rank: honor, suit: "", red: mjai === "C" };
  return { rank: mjai[0], suit: SUITS[mjai[1]] ?? "?", red: mjai.endsWith("r") };
}
