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
  // A tile still to draw: drawn faint in a dashed outline.
  missing?: boolean;
  // How dangerous it is to discard (#12): a coloured bar under it, and why on hover.
  danger?: { level: string; why: string } | null;
  // Clicking it, e.g. to correct it (#16).
  onClick?: () => void;
};

// A tile picture with the panel's marks.
export function Tile({
  tile,
  width,
  mark = null,
  dim = false,
  doubt = false,
  missing = false,
  danger = null,
  onClick,
}: Props) {
  const title =
    tile === null
      ? "扣着的牌"
      : name(tile) + (doubt ? "（拿不准）" : missing ? "（还缺）" : "") + (danger ? `：${danger.why}` : "");
  const classes = ["tile", mark === "main" ? "main" : "", dim ? "dim" : "", missing ? "missing" : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <span
      className={onClick ? `${classes} clickable` : classes}
      style={{ width }}
      title={onClick ? `${title}（点击改正）` : title}
      onClick={onClick}
    >
      <span className="face">
        {mark === "main" && <span className="arrow">▼</span>}
        <img src={tilePicture(tile)} alt={title} draggable={false} />
        {doubt && <span className="doubt">?</span>}
      </span>
      {(mark === 2 || mark === 3) && <span className={`bar bar-${mark}`} />}
      {danger && <span className={`danger danger-${danger.level}`} />}
    </span>
  );
}
