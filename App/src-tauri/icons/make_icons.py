"""Generate the console's geometric Windows icons with Pillow in VISA.

Run from the repository root with:
    conda run -n VISA --no-capture-output python -B App/src-tauri/icons/make_icons.py
"""

from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw


ROOT = Path(__file__).resolve().parent
SIZE = 1024


def make_icon() -> Image.Image:
    image = Image.new("RGB", (SIZE, SIZE), "#10212c")
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle((34, 34, 990, 990), radius=198, fill="#162e3a")
    draw.rounded_rectangle((80, 80, 944, 944), radius=164,
                           outline="#2a525e", width=12)
    # The same three rising cyan bars used by the application shell.
    draw.rounded_rectangle((230, 510, 374, 772), radius=38, fill="#40c8cb")
    draw.rounded_rectangle((434, 298, 578, 772), radius=38, fill="#7fe0d8")
    draw.rounded_rectangle((638, 414, 782, 772), radius=38, fill="#56b8cb")
    return image


def main() -> None:
    image = make_icon()
    image.resize((512, 512), Image.Resampling.LANCZOS).save(ROOT / "icon.png")
    image.save(ROOT / "icon.ico", format="ICO",
               sizes=[(16, 16), (24, 24), (32, 32), (48, 48),
                      (64, 64), (128, 128), (256, 256)])


if __name__ == "__main__":
    main()
