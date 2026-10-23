import { name } from "./format";
import { tilePicture } from "./tile";

type Props = {
  tile: string | null;
  // Width in px; the height follows the picture (1.36 times as tall).
  width: number;
  // The first choice: raised, framed in red, with an arrow. The second and third: a bar.
  mark?: "main" | 2 | 3 | null;
  dim?: boolean;
  doubt?: boolean;
};

// A tile picture with the panel's marks.
export function Tile({ tile, width, mark = null, dim = false, doubt = false }: Props) {
  const title = tile === null ? "扣着的牌" : name(tile) + (doubt ? "（拿不准）" : "");
  const classes = ["tile", mark === "main" ? "main" : "", dim ? "dim" : ""].filter(Boolean).join(" ");
  return (
    <span className={classes} style={{ width }} title={title}>
      <span className="face">
        {mark === "main" && <span className="arrow">▼</span>}
        <img src={tilePicture(tile)} alt={title} draggable={false} />
        {doubt && <span className="doubt">?</span>}
      </span>
      {(mark === 2 || mark === 3) && <span className={`bar bar-${mark}`} />}
    </span>
  );
}
