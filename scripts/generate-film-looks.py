"""Generates the film looks and the desktop's film simulation from the film data in
spectral_film_lut (MIT), Jan Lohse's spectral simulation of film stocks.

A film is split in two 3D LUTs, so the desktop can pair any negative with any print and
change what happens in between:

- a negative LUT, from scene light (log2 around mid grey, per channel) to the densities a
  printer would see (ADX-coded, as spectral_film_lut's APD intermediate);
- an output LUT, from those densities through a print stock and its viewing light to
  display sRGB. A slide has its own output LUT: it is projected rather than printed.

Printer lights shift the densities between the two, film exposure shifts the input, and
grain is added to the densities, following each stock's RMS granularity curve.

The web edition's film looks are the same pair composed at the default settings into one
LUT from display sRGB to display sRGB: the display colour is turned back into scene light
with the inverse of OpenPixels' tone curve, as the desktop's shader does.

Everything is committed, so building the apps never needs Python. To regenerate:

    uv venv --python 3.12 /tmp/sfl-venv
    VIRTUAL_ENV=/tmp/sfl-venv uv pip install numpy scipy colour-science numba matplotlib
    git clone https://github.com/JanLohse/spectral_film_lut /tmp/spectral_film_lut
    /tmp/sfl-venv/bin/python -I scripts/generate-film-looks.py /tmp/spectral_film_lut/src

Writes, with LUTs as little-endian u16 RGB with red varying fastest:
- web/public/film-looks/<id>.lut and web/src/film-looks.json, the composed looks;
- desktop/film-sim/negative-<id>.lut, print-<id>.lut, slide-<id>.lut and film-sim.json.
"""

import json
import os
import sys
from pathlib import Path

import numpy as np

if len(sys.argv) < 2:
    sys.exit(__doc__)
sys.path.insert(0, sys.argv[1])

import spectral_film_lut as sfl  # noqa: E402
from spectral_film_lut.film_spectral import FilmSpectral  # noqa: E402
from spectral_film_lut.utils import film_conversion  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
LOOK_FOLDER = ROOT / "web" / "public" / "film-looks"
LOOK_MANIFEST = ROOT / "web" / "src" / "film-looks.json"
SIM_FOLDER = ROOT / "desktop" / "film-sim"
SIZE = 33

# Keep in sync with op-core/src/film.rs and the shader.
LOG_MIN = -8.0
LOG_MAX = 7.0
MID = 0.18
ADX_SCALING = 2.0
ADX_CHANNEL_SCALE = np.array([1.00, 0.92, 0.95])
GRAIN_SAMPLES = 16

# OpenPixels' default tone curve (op-core/src/look.rs), inverted to recover scene light.
LOOK_CONTRAST = 1.5
LOOK_KNEE = 0.348
# Display white maps to this much scene light; brighter highlights were already compressed.
WHITEST = 0.995
# Display mid grey, kept as bright through every look.
MID_GREY = 0.5

REFERENCE_NEGATIVE = "Kodak Vision3 250D 5207"

# Print stocks the desktop offers. Ids are stored in edits, so they never change.
PRINTS = [
    (1, "colour", "Kodak Portra Endura Paper"),
    (2, "colour", "Kodak Endura Premier Paper"),
    (3, "colour", "Kodak Supra Endura Paper"),
    (4, "colour", "Fuji Crystal Archive Maxima"),
    (5, "colour", "Fuji Crystal Archive DPII"),
    (6, "colour", "Fuji Crystal Archive Pro PDII"),
    (7, "cine", "Kodak Vision 2383"),
    (8, "cine", "Kodak Vision Premier 2393"),
    (9, "cine", "Fuji Eterna-CP 3523XD"),
    (10, "cine", "Fuji Eterna-CP Type 3513DI"),
    (11, "bw", "Kodak Polymax Fine-Art Paper Grade 2"),
    (12, "bw", "Kodak Polymax Fine-Art Paper Grade 3"),
    (13, "bw", "Kodak Polymax Fine-Art Paper Grade 4"),
    (14, "bw", "Kodak 2302 Dev 7"),
]
PRINT_IDS = {name: print_id for print_id, _, name in PRINTS}

# (id, group, stock, default print or None to project the film itself). Ids are stored in
# edits, so they never change or get reused; 30-99 are kept for film looks (1-24 are the
# other built-in looks, 100 and up imported ones).
LOOKS = [
    (30, "彩色负片", "Kodak Portra 160", "Kodak Portra Endura Paper"),
    (31, "彩色负片", "Kodak Portra 400", "Kodak Portra Endura Paper"),
    (32, "彩色负片", "Kodak Portra 800", "Kodak Portra Endura Paper"),
    (33, "彩色负片", "Kodak Ektar 100", "Kodak Portra Endura Paper"),
    (34, "彩色负片", "Kodak Gold 200", "Kodak Portra Endura Paper"),
    (35, "彩色负片", "Kodak Ultramax 400", "Kodak Portra Endura Paper"),
    (36, "彩色负片", "Fuji Pro 400H", "Fuji Crystal Archive Maxima"),
    (37, "彩色负片", "Fuji Pro 160S", "Fuji Crystal Archive Maxima"),
    (38, "彩色负片", "Fuji Superia X-Tra 400", "Fuji Crystal Archive Maxima"),
    (39, "彩色负片", "Fuji Superia Reala", "Fuji Crystal Archive Maxima"),
    (40, "彩色负片", "Fuji C200", "Fuji Crystal Archive Maxima"),
    (41, "彩色负片", "Fuji Natura 1600", "Fuji Crystal Archive Maxima"),
    (42, "彩色负片", "Agfa Vista 100", "Kodak Portra Endura Paper"),
    (43, "电影负片", "Kodak Vision3 50D 5203", "Kodak Vision 2383"),
    (44, "电影负片", "Kodak Vision3 250D 5207", "Kodak Vision 2383"),
    (45, "电影负片", "Kodak Vision3 200T 5213", "Kodak Vision 2383"),
    (46, "电影负片", "Kodak Vision3 500T 5219", "Kodak Vision 2383"),
    (47, "电影负片", "Kodak EXR 200T 5293", "Kodak Vision 2383"),
    (48, "电影负片", "Fuji Eterna 500", "Fuji Eterna-CP 3523XD"),
    (49, "电影负片", "Fuji Eterna 500 Vivid", "Fuji Eterna-CP 3523XD"),
    (50, "反转片", "Fuji Velvia 50", None),
    (51, "反转片", "Fuji Provia 100F", None),
    (52, "反转片", "Kodak Ektachrome 100D", None),
    (53, "反转片", "Kodachrome 64", None),
    (54, "反转片", "Fuji Instax color", None),
    (55, "反转片", "Fuji FP-100C", None),
    (56, "黑白胶片", "Kodak Tri-X 400", "Kodak Polymax Fine-Art Paper Grade 3"),
    (57, "黑白胶片", "Kodak 5222", "Kodak 2302 Dev 7"),
]

# Names shown in place of the data's own, where the stock is better known by another.
SHOWN_NAMES = {
    "Kodak 5222": "Kodak Double-X 5222",
    "Kodak Polymax Fine-Art Paper Grade 2": "Kodak Polymax Fine-Art Grade 2",
    "Kodak Polymax Fine-Art Paper Grade 3": "Kodak Polymax Fine-Art Grade 3",
    "Kodak Polymax Fine-Art Paper Grade 4": "Kodak Polymax Fine-Art Grade 4",
}

# Linear sRGB (D65) to CIE XYZ.
SRGB_TO_XYZ = np.array(
    [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.1191920, 0.9503041]]
)
LUMA = np.array([0.2126, 0.7152, 0.0722])


def load_stocks() -> dict[str, FilmSpectral]:
    """All stocks, prepared the way the spectral_film_lut app prepares them."""
    stocks = []
    for data in sfl.FILM_STOCKS:
        stock = FilmSpectral(data)
        if stocks and stock.stage == "print" and stock.density_measure == "status_a":
            stock.set_color_checker(negative=stocks[0])
        else:
            stock.set_color_checker()
        stocks.append(stock)
    return {stock.name: stock for stock in stocks}


def grid() -> np.ndarray:
    """The LUT's input points, red varying fastest."""
    axis = np.linspace(0.0, 1.0, SIZE)
    b, g, r = np.meshgrid(axis, axis, axis, indexing="ij")
    return np.stack([r, g, b], axis=-1)


def three(values: np.ndarray) -> np.ndarray:
    values = np.clip(np.nan_to_num(values, nan=0.0), 0.0, 1.0)
    return np.repeat(values, 3, axis=-1) if values.shape[-1] == 1 else values


def is_slide(stock: FilmSpectral) -> bool:
    return stock.density_measure == "status_a"


def negative_lut(negative: FilmSpectral) -> np.ndarray:
    """Scene light, log2-coded per channel, to ADX-coded printing densities."""
    scene = MID * 2.0 ** (LOG_MIN + grid() * (LOG_MAX - LOG_MIN))
    return three(
        film_conversion(
            scene @ SRGB_TO_XYZ.T,
            negative,
            None,
            mode="negative",
            input_colorspace=None,
            color_masking=negative.color_masking,
            adx_coding=True,
            adx_scaling=ADX_SCALING,
            apd_intermediate=True,
        )
    )


def output_lut(film: FilmSpectral, printed: FilmSpectral | None, reference: FilmSpectral) -> np.ndarray:
    """ADX-coded densities to display sRGB: printed and viewed, or a slide projected."""
    return three(
        film_conversion(
            grid(),
            film if printed is None else reference,
            printed,
            mode="print",
            output_gamut="Rec. 709",  # sRGB primaries
            gamma_func="sRGB",
            # Slides are balanced on mid grey when projected; prints by their printer lights.
            white_balance=1,
            adx_coding=True,
            adx_scaling=ADX_SCALING,
            apd_intermediate=True,
            reference_negative=reference,
        )
    )


def grain_table(film: FilmSpectral) -> list[list[float]]:
    """RMS grain of each layer at evenly spaced ADX codes, in code units per pixel when a
    millimetre of film covers one pixel. The shader scales it by pixels per millimetre."""
    if getattr(film, "grain_curve", None) is None:
        return []
    curve = film.get_grain_curve(scale=1.0, std_div=1.0, adx=True, apd_intermediate=not is_slide(film))
    codes = np.linspace(0.0, 1.0, GRAIN_SAMPLES)
    table = []
    for code in codes:
        density = (code / ADX_SCALING - 1520.0 / 65535.0) / (ADX_CHANNEL_SCALE * 8000.0 / 65535.0)
        row = []
        for channel in range(3):
            source = min(channel, curve.shape[0] - 2)
            sigma = np.interp(density[channel], curve[0], curve[1 + source])
            row.append(round(float(sigma) * ADX_SCALING, 7))
        table.append(row)
    return table


def trilinear(lut: np.ndarray, at: np.ndarray) -> np.ndarray:
    """Samples a LUT stored [b][g][r] at points in 0..1, as the GPU's filtering does."""
    n = lut.shape[0]
    position = np.clip(at, 0.0, 1.0) * (n - 1)
    base = np.clip(np.floor(position).astype(int), 0, n - 2)
    t = position - base
    out = 0.0
    for db in (0, 1):
        for dg in (0, 1):
            for dr in (0, 1):
                weight = (t[..., 0:1] if dr else 1 - t[..., 0:1]) * (t[..., 1:2] if dg else 1 - t[..., 1:2]) * (t[..., 2:3] if db else 1 - t[..., 2:3])
                out = out + weight * lut[base[..., 2] + db, base[..., 1] + dg, base[..., 0] + dr]
    return out


def display_to_scene(display: np.ndarray) -> np.ndarray:
    """Display sRGB code values to scene-linear sRGB, undoing the default tone curve."""
    linear = np.where(display <= 0.04045, display / 12.92, ((display + 0.055) / 1.055) ** 2.4)
    linear = np.minimum(linear, WHITEST)
    return (linear * LOOK_KNEE / (1.0 - linear)) ** (1.0 / LOOK_CONTRAST)


def compose(negative: np.ndarray, output: np.ndarray, display: np.ndarray, exposure: float) -> np.ndarray:
    """What the desktop's shader does with default printer lights and no grain or halation."""
    scene = display_to_scene(display) * 2.0**exposure
    code = (np.log2(np.maximum(scene, 1e-12) / MID) - LOG_MIN) / (LOG_MAX - LOG_MIN)
    return np.clip(trilinear(output, trilinear(negative, code)), 0.0, 1.0)


def balance(negative: np.ndarray, output: np.ndarray) -> float:
    """The exposure, in stops, that leaves mid grey as bright as it went in, so choosing a
    look changes colour and contrast but not the photo's overall brightness. The desktop
    finds it the same way for any pairing."""
    grey = np.full((1, 3), MID_GREY)
    low, high = -4.0, 4.0
    for _ in range(30):
        middle = (low + high) / 2
        if float(compose(negative, output, grey, middle)[0] @ LUMA) > MID_GREY:
            high = middle
        else:
            low = middle
    return (low + high) / 2


def write_lut(path: Path, values: np.ndarray) -> None:
    path.write_bytes(np.round(np.clip(values, 0, 1) * 65535).astype("<u2").tobytes())


def main() -> None:
    stocks = load_stocks()
    reference = stocks[REFERENCE_NEGATIVE]
    LOOK_FOLDER.mkdir(parents=True, exist_ok=True)
    SIM_FOLDER.mkdir(parents=True, exist_ok=True)

    outputs: dict[str, np.ndarray] = {}
    prints = []
    for print_id, kind, name in PRINTS:
        outputs[name] = output_lut(reference, stocks[name], reference)
        write_lut(SIM_FOLDER / f"print-{print_id}.lut", outputs[name])
        prints.append({"id": print_id, "kind": kind, "name": SHOWN_NAMES.get(name, name)})
        print(f"print {print_id} {name}", flush=True)

    looks = []
    sim = []
    for look_id, group, name, paper in LOOKS:
        film = stocks[name]
        negative = negative_lut(film)
        write_lut(SIM_FOLDER / f"negative-{look_id}.lut", negative)
        if paper is None:
            output = output_lut(film, None, reference)
            write_lut(SIM_FOLDER / f"slide-{look_id}.lut", output)
        else:
            output = outputs[paper]
        exposure = balance(negative, output)
        write_lut(LOOK_FOLDER / f"{look_id}.lut", compose(negative, output, grid(), exposure))
        looks.append({"id": look_id, "group": group, "name": SHOWN_NAMES.get(name, name), "print": SHOWN_NAMES.get(paper, paper)})
        sim.append(
            {
                "id": look_id,
                "kind": "slide" if paper is None else "negative",
                "mono": film.density_measure == "bw",
                "print": PRINT_IDS.get(paper, 0),
                "grain": grain_table(film),
            }
        )
        grey = compose(negative, output, np.full((1, 3), MID_GREY), exposure)[0]
        print(f"{look_id} {name}: {exposure:+.2f} EV, densities up to {negative.max():.2f}, grey {np.round(grey, 3)}", flush=True)

    LOOK_MANIFEST.write_text(json.dumps({"size": SIZE, "looks": looks}, ensure_ascii=False, indent=2) + "\n")
    settings = {
        "size": SIZE,
        "logMin": LOG_MIN,
        "logMax": LOG_MAX,
        "adxScaling": ADX_SCALING,
        "prints": prints,
        "films": sim,
    }
    (SIM_FOLDER / "film-sim.json").write_text(json.dumps(settings, ensure_ascii=False, indent=1) + "\n")


if __name__ == "__main__":
    os.chdir(ROOT)
    main()
