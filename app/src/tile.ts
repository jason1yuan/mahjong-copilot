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

// Tile pictures by mjai name, bundled with the app (lietxia/mahjong_graphic, M+ licence;
// the back is ours). Each is its own image, so their inner styles do not mix.
const PICTURES = import.meta.glob<string>("./assets/tiles/*.svg", {
  eager: true,
  query: "?url",
  import: "default",
});

// The picture of an mjai tile, or of a tile face down for `null`.
export function tilePicture(mjai: string | null): string {
  return PICTURES[`./assets/tiles/${mjai ?? "back"}.svg`] ?? PICTURES["./assets/tiles/back.svg"];
}

// Every tile kind, red fives after their suit, for the check page.
export const ALL_TILES: string[] = [
  ..."mps".split("").flatMap((s) => [...Array.from({ length: 9 }, (_, i) => `${i + 1}${s}`), `5${s}r`]),
  ..."ESWNPFC".split(""),
];
