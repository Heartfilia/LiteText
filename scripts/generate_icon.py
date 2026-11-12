"""Regenerate LiteText's PNG/ICO/ICNS assets (requires Pillow).

The geometric, semibold T matches assets/app-icon.svg without relying on a
system font. Run from any directory: python3 scripts/generate_icon.py
"""

from pathlib import Path

from PIL import Image, ImageDraw


def main():
    assets = Path(__file__).resolve().parents[1] / "assets"
    scale = 8
    image = Image.new("RGBA", (512 * scale, 512 * scale))
    draw = ImageDraw.Draw(image)

    def rounded_rect(bounds, radius, fill):
        draw.rounded_rectangle(
            tuple(round(value * scale) for value in bounds),
            radius=radius * scale,
            fill=fill,
        )

    rounded_rect((40, 40, 472, 472), 112, "#0e1116")
    rounded_rect((231, 181, 281, 373), 6, "#f8fafc")
    rounded_rect((148, 147, 364, 197), 6, "#f8fafc")
    icon = image.resize((512, 512), Image.Resampling.LANCZOS)
    icon.save(assets / "app-icon.png", optimize=True)
    icon.save(
        assets / "app-icon.ico",
        sizes=[(size, size) for size in (16, 24, 32, 48, 64, 128, 256)],
    )
    image.resize((1024, 1024), Image.Resampling.LANCZOS).save(
        assets / "app-icon.icns"
    )


if __name__ == "__main__":
    main()
