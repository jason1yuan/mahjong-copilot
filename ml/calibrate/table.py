"""Calibrates where the 3D table lies on screen (#8).

Rivers lie flat on the table, so one perspective transform maps table coordinates (tile
widths from the centre, the player's own seat towards +y) to the game area, and each
seat's river is the own seat's turned by quarter turns. This fits that transform and the
river layout to the river tiles of recorded frames, and prints the profile's "table" and
"river" entries.

Only clean rivers are used: rows of `columns` tiles and then one shorter row, so a riichi
tile lying sideways or a tile assigned to the wrong seat cannot shift the slots. Frames
whose game area is off the median (the hand misread on a results screen) are skipped.

    cargo run -p mahjong-perception --release --example game_areas -- recordings/live > /tmp/areas.json
    ml/.venv/bin/python -I ml/calibrate/table.py /tmp/areas.json

Images stay local; only the fitted numbers go into the profile.
"""

from __future__ import annotations

import argparse
import json
import tomllib
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image
from scipy import ndimage, optimize

SEATS = ("own", "right", "across", "left")
# Per seat, in screen terms: the axis tiles follow within a row and its direction, then the
# axis rows follow away from the centre and its direction.
AXES = {
    "own": ("x", 1, "y", 1),
    "right": ("y", -1, "x", 1),
    "across": ("x", -1, "y", -1),
    "left": ("y", 1, "x", -1),
}
CORNERS = np.array([[-6.0, -6.0], [6.0, -6.0], [6.0, 6.0], [-6.0, 6.0]])


@dataclass
class Face:
    """A tile face: centre as fractions of the game area, box size in pixels."""

    x: float
    y: float
    w: int
    h: int


@dataclass
class Observation:
    seat: int
    col: int
    row: int
    x: float
    y: float


def find_faces(image: np.ndarray, area: dict[str, float], cfg: dict, face: dict[str, int]) -> list[Face]:
    rgb = image.astype(np.int16)
    hi, lo = rgb.max(axis=2), rgb.min(axis=2)
    mask = (hi >= face["min_brightness"]) & (hi - lo <= face["max_spread"])
    left, top, width, height = area["left"], area["top"], area["width"], area["height"]
    r = cfg["region"]
    x0, y0 = int(left + r[0] * width), int(top + r[1] * height)
    x1, y1 = int(left + r[2] * width), int(top + r[3] * height)
    labels, _ = ndimage.label(mask[y0:y1, x0:x1])
    min_area = cfg["min_face_area"] * (width / 3018) ** 2
    faces = []
    for i, box in enumerate(ndimage.find_objects(labels), start=1):
        if box is None:
            continue
        pixels = np.argwhere(labels[box] == i)
        if len(pixels) < min_area:
            continue
        cy, cx = pixels.mean(axis=0)
        h, w = box[0].stop - box[0].start, box[1].stop - box[1].start
        faces.append(
            Face(
                x=(box[1].start + cx + x0 - left) / width,
                y=(box[0].start + cy + y0 - top) / height,
                w=w,
                h=h,
            )
        )
    return faces


def by_seat(faces: list[Face], cfg: dict) -> dict[str, list[Face]]:
    cx, cy = cfg["centre"]
    seats: dict[str, list[Face]] = {s: [] for s in SEATS}
    for f in faces:
        if f.w / f.h > cfg["side_aspect"]:
            seats["left" if f.x < cx else "right"].append(f)
        else:
            seats["own" if f.y > cy else "across"].append(f)
    return seats


def rows_of(seat: str, faces: list[Face], gap: float) -> list[list[Face]]:
    along, along_dir, cross, cross_dir = AXES[seat]
    faces = sorted(faces, key=lambda f: cross_dir * getattr(f, cross))
    rows: list[list[Face]] = []
    for f in faces:
        if rows and cross_dir * (getattr(f, cross) - getattr(rows[-1][-1], cross)) <= gap:
            rows[-1].append(f)
        else:
            rows.append([f])
    for row in rows:
        row.sort(key=lambda f: along_dir * getattr(f, along))
    return rows


def observations(areas: list[dict], cfg: dict, face: dict[str, int]) -> list[Observation]:
    out = []
    for area in areas:
        image = np.asarray(Image.open(area["path"]).convert("RGB"))
        seats = by_seat(find_faces(image, area, cfg, face), cfg)
        for k, seat in enumerate(SEATS):
            rows = rows_of(seat, seats[seat], cfg["row_gap"][seat])
            lengths = [len(r) for r in rows]
            clean = (
                0 < len(rows) <= 3
                and all(n == cfg["columns"] for n in lengths[:-1])
                and lengths[-1] <= cfg["columns"]
            )
            if not clean:
                continue
            for r, row in enumerate(rows):
                for c, f in enumerate(row):
                    out.append(Observation(k, c, r, f.x, f.y))
    return out


def turn(xy: np.ndarray, quarter_turns: np.ndarray) -> np.ndarray:
    """Seat coordinates to table coordinates: (x, y) -> (y, -x) per quarter turn."""
    out = xy.copy()
    for _ in range(3):
        m = quarter_turns > 0
        out[m] = np.c_[out[m, 1], -out[m, 0]]
        quarter_turns = quarter_turns - 1
    return out


def project(h: np.ndarray, xy: np.ndarray) -> np.ndarray:
    v = np.c_[xy, np.ones(len(xy))] @ h.T
    return v[:, :2] / v[:, 2:3]


def dlt(src: np.ndarray, dst: np.ndarray) -> np.ndarray:
    """Least-squares homography from point pairs (direct linear transform)."""
    rows = []
    for (x, y), (u, v) in zip(src, dst):
        rows.append([x, y, 1, 0, 0, 0, -u * x, -u * y, -u])
        rows.append([0, 0, 0, x, y, 1, -v * x, -v * y, -v])
    _, _, vt = np.linalg.svd(np.array(rows))
    h = vt[-1].reshape(3, 3)
    return h / h[2, 2]


def fit(
    obs: list[Observation], cfg: dict, size: tuple[float, float]
) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    seat = np.array([o.seat for o in obs])
    col = np.array([o.col for o in obs], float)
    row = np.array([o.row for o in obs], float)
    target = np.array([[o.x, o.y] for o in obs])

    def table_points(layout: np.ndarray) -> np.ndarray:
        left, near, pitch = layout
        own = np.c_[left + col + 0.5, near + (row + 0.5) * pitch]
        return turn(own, seat)

    def residuals(p: np.ndarray) -> np.ndarray:
        h = np.append(p[:8], 1.0).reshape(3, 3)
        d = project(h, table_points(p[8:])) - target
        return np.r_[d[:, 0] * size[0], d[:, 1] * size[1]]

    start = np.array(cfg["start"], float)
    h0 = dlt(table_points(start), target)
    result = optimize.least_squares(
        residuals,
        np.r_[h0.ravel()[:8], start],
        loss="soft_l1",
        f_scale=cfg["loss_scale"],
        x_scale="jac",
    )
    r = residuals(result.x)
    err = np.hypot(r[: len(obs)], r[len(obs):])
    return np.append(result.x[:8], 1.0).reshape(3, 3), result.x[8:], err, seat


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("areas", type=Path, help="JSON from the game_areas example")
    parser.add_argument("--config", type=Path, default=Path(__file__).with_name("config.toml"))
    args = parser.parse_args()
    cfg = tomllib.loads(args.config.read_text())
    face = json.loads(Path(cfg["profile"]).read_text())["tile_face"]
    areas = json.loads(args.areas.read_text())
    width = float(np.median([a["width"] for a in areas]))
    areas = [a for a in areas if abs(a["width"] / width - 1) <= cfg["max_area_change"]]
    size = (width, float(np.median([a["height"] for a in areas])))

    obs = observations(areas, cfg, face)
    h, layout, err, seat = fit(obs, cfg, size)
    print(f"{len(areas)} frames, {len(obs)} tiles in clean rivers")
    for k, name in enumerate(SEATS):
        e = err[seat == k]
        print(f"  {name:6s} {len(e):4d} tiles, error px median {np.median(e):.1f}, p90 {np.percentile(e, 90):.1f}")
    corners = project(h, CORNERS)
    profile = {
        "table": {"corners": [[round(float(x), 5), round(float(y), 5)] for x, y in corners]},
        "river": {
            "left": round(float(layout[0]), 4),
            "near": round(float(layout[1]), 4),
            "row_pitch": round(float(layout[2]), 4),
            "columns": cfg["columns"],
        },
    }
    corners_text = ", ".join(f"[{x}, {y}]" for x, y in profile["table"]["corners"])
    river_text = json.dumps(profile["river"], indent=4).replace("\n}", "\n  }")
    print(f'  "table": {{\n    "corners": [{corners_text}]\n  }},\n  "river": {river_text}')


if __name__ == "__main__":
    main()
