"""Regenerate the app icon (assets/autotyper.ico and assets/autotyper.png).

Original artwork drawn with plain shapes (no fonts, no third-party art):
a keycap with a text cursor on a blue rounded square.

    uv run --no-project --with pillow python assets/make_icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
S = 1024  # master canvas size; every output size is downsampled from it


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def master() -> Image.Image:
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # Background: rounded square with a vertical blue gradient.
    grad = Image.new("RGBA", (S, S))
    top, bottom = (59, 130, 246), (30, 64, 175)
    for y in range(S):
        ImageDraw.Draw(grad).line([(0, y), (S, y)], fill=lerp(top, bottom, y / (S - 1)) + (255,))
    mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(mask).rounded_rectangle([40, 40, S - 40, S - 40], radius=220, fill=255)
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    # Keycap: darker base + white top face.
    d.rounded_rectangle([200, 250, 824, 830], radius=110, fill=(203, 213, 225, 255))
    d.rounded_rectangle([200, 210, 824, 760], radius=110, fill=(255, 255, 255, 255))
    # Typed text: two short bars.
    blue = (30, 64, 175, 255)
    d.rounded_rectangle([300, 400, 560, 470], radius=35, fill=blue)
    d.rounded_rectangle([300, 520, 470, 590], radius=35, fill=blue)
    # Text cursor (I-beam) after the text.
    d.rounded_rectangle([640, 330, 705, 660], radius=30, fill=(59, 130, 246, 255))
    return img


def main() -> None:
    big = master()
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    big.resize((256, 256), Image.LANCZOS).save(HERE / "autotyper.png", optimize=True)
    big.resize((256, 256), Image.LANCZOS).save(
        HERE / "autotyper.ico", sizes=[(s, s) for s in sizes]
    )
    print("wrote", HERE / "autotyper.ico", HERE / "autotyper.png")


if __name__ == "__main__":
    main()
