"""Deterministic generator for Aurora's external application identity.

Aurora has two canonical artwork roles (see ARCHITECTURE.md):

- the INTERNAL transparent white mark  (`static/aurora-icon.png`) - never
  modified by this tool, only read;
- the EXTERNAL application icon         (`static/aurora-app-icon.png`) -
  the OS-facing identity: the exact internal mark composited onto a
  rounded-square dark neutral vertical gradient.

This is DEVELOPMENT/ASSET tooling only. The launcher never generates its app
icon at runtime. Given the same canonical transparent mark and the same Pillow
major version, the tool is byte-for-byte reproducible (fixed resampling,
fixed PNG compression, no timestamps).

Usage:

    python tools/generate_external_icon.py

Requires Pillow (developed against 12.3.0). Writes:

    static/aurora-app-icon.png     1024x1024 external canonical master
    static/favicon.png             128x128 SPA favicon (external identity)
    src-tauri/icons/icon.ico       Windows ICO, frames 40,16,20,24,32,48,64,128,256
    src-tauri/icons/icon.png       512x512 (Linux window icon)
    src-tauri/icons/32x32.png
    src-tauri/icons/128x128.png
    src-tauri/icons/128x128@2x.png (256x256)
    src-tauri/icons/icon.icns      macOS ICNS (PNG chunks ic07..ic10, ic11, ic12)
    src-tauri/icons/Square*.png + StoreLogo.png   Windows Store logo set

Exact design parameters:

    CANVAS          1024 x 1024
    CORNER_RADIUS   225 px (21.97% of canvas) - restrained modern rounded square
    GRADIENT        vertical, linear in sRGB, no dithering:
                    top    rgb(38, 39, 42)   #26272A  (dark charcoal gray)
                    bottom rgb( 5,  5,  6)   #050506  (near-black)
    MARK_FRACTION   0.66 of canvas height (alpha-bbox height of the mark)
    MARK_CENTERING  alpha-bbox center of the mark at the canvas center
    COMPOSITING     exact source pixels/alpha; no shadow/glow/outline/recolor

Rasterization (afdc609 lesson, preserved):

    every derivative frame is ONE premultiplied-alpha LANCZOS resample taken
    directly from the 1024 master (never a resize of an already-small frame);
    the rounded-square mask is drawn at 4x supersampling and LANCZOS-reduced.

ICO frame order (compatibility contract):

    entry[0] MUST stay 40x40 - tauri-codegen builds the runtime window HICON
    from ICO entry[0] (tauri-codegen 2.6.3, image.rs new_ico), so the first
    frame decides what the title bar and taskbar rasterize from; full order is
    40, 16, 20, 24, 32, 48, 64, 128, 256 (all 32bpp PNG-encoded entries).
"""

import io
import os
import struct

from PIL import Image, ImageChops, ImageDraw

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INTERNAL_MARK = os.path.join(REPO, "static", "aurora-icon.png")

CANVAS = 1024
CORNER_RADIUS = 225
GRADIENT_TOP = (38, 39, 42)
GRADIENT_BOTTOM = (5, 5, 6)
MARK_FRACTION = 0.66
SUPERSAMPLE = 4

ICO_ORDER = (40, 16, 20, 24, 32, 48, 64, 128, 256)
PNG_LOGO_SIZES = (30, 44, 71, 89, 107, 142, 150, 284, 310)
STORE_LOGO_SIZE = 50
ICNS_CHUNKS = ((b"ic11", 32), (b"ic12", 64), (b"ic07", 128), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024))


def premultiply(image):
    """Resample-correct compositing: color channels weighted by alpha."""
    r, g, b, a = image.split()
    return Image.merge(
        "RGBA",
        (ImageChops.multiply(r, a), ImageChops.multiply(g, a), ImageChops.multiply(b, a), a),
    )


def unpremultiply(image):
    width, height = image.size
    source = image.load()
    out = Image.new("RGBA", (width, height))
    target = out.load()
    for y in range(height):
        for x in range(width):
            r, g, b, a = source[x, y]
            if a == 0:
                target[x, y] = (0, 0, 0, 0)
            else:
                target[x, y] = (
                    min(255, (r * 255) // a),
                    min(255, (g * 255) // a),
                    min(255, (b * 255) // a),
                    a,
                )
    return out


def resample(image, size):
    """One premultiplied-alpha LANCZOS resample; the only resize path."""
    return unpremultiply(premultiply(image).resize((size, size), Image.Resampling.LANCZOS))


def rounded_square_mask(size):
    big = size * SUPERSAMPLE
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, big - 1, big - 1], radius=CORNER_RADIUS * SUPERSAMPLE, fill=255)
    return mask.resize((size, size), Image.Resampling.LANCZOS)


def compose_master():
    """The external master: gradient rounded square + exact internal mark."""
    source = Image.open(INTERNAL_MARK).convert("RGBA")
    left, top, right, bottom = source.getchannel("A").getbbox()
    mark = source.crop((left, top, right, bottom))
    mark_width, mark_height = mark.size

    background = Image.new("RGBA", (CANVAS, CANVAS))
    pixels = background.load()
    for y in range(CANVAS):
        t = y / (CANVAS - 1)
        row = tuple(round(GRADIENT_TOP[i] + (GRADIENT_BOTTOM[i] - GRADIENT_TOP[i]) * t) for i in range(3)) + (255,)
        for x in range(CANVAS):
            pixels[x, y] = row

    target_height = round(CANVAS * MARK_FRACTION)
    target_width = round(target_height * mark_width / mark_height)
    if target_width > CANVAS:
        target_width = CANVAS
        target_height = round(CANVAS * mark_height / mark_width)
    scaled = unpremultiply(premultiply(mark).resize((target_width, target_height), Image.Resampling.LANCZOS))
    plate = background.copy()
    plate.alpha_composite(scaled, ((CANVAS - target_width) // 2, (CANVAS - target_height) // 2))

    master = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    master.paste(plate, (0, 0), rounded_square_mask(CANVAS))
    return master


def save_png(image, path):
    image.save(path, format="PNG", compress_level=6, optimize=False)
    print(f"wrote {os.path.relpath(path, REPO)} ({image.size[0]}x{image.size[1]})")


def png_bytes(image):
    buffer = io.BytesIO()
    image.save(buffer, format="PNG", compress_level=6, optimize=False)
    return buffer.getvalue()


def write_ico(master, path):
    frames = [resample(master, n) for n in ICO_ORDER]
    blobs = [png_bytes(frame) for frame in frames]
    header = struct.pack("<HHH", 0, 1, len(frames))
    offset = 6 + 16 * len(frames)
    entries = b""
    for frame, blob in zip(frames, blobs):
        n = frame.size[0]
        entries += struct.pack(
            "<BBBBHHII",
            0 if n >= 256 else n,
            0 if n >= 256 else n,
            0,
            0,
            1,
            32,
            len(blob),
            offset,
        )
        offset += len(blob)
    with open(path, "wb") as handle:
        handle.write(header + entries + b"".join(blobs))
    print(f"wrote {os.path.relpath(path, REPO)} (frames {list(ICO_ORDER)}, entry[0]={ICO_ORDER[0]}x{ICO_ORDER[0]})")


def write_icns(master, path):
    chunks = []
    for code, size in ICNS_CHUNKS:
        payload = png_bytes(resample(master, size))
        chunks.append(struct.pack(">4sI", code, 8 + len(payload)) + payload)
    total = 8 + sum(len(chunk) for chunk in chunks)
    with open(path, "wb") as handle:
        handle.write(struct.pack(">4sI", b"icns", total) + b"".join(chunks))
    print(f"wrote {os.path.relpath(path, REPO)} (chunks {[c[0].decode() for c in ICNS_CHUNKS]})")


def main():
    icons = os.path.join(REPO, "src-tauri", "icons")
    static = os.path.join(REPO, "static")
    os.makedirs(icons, exist_ok=True)

    master = compose_master()

    save_png(master, os.path.join(static, "aurora-app-icon.png"))
    save_png(resample(master, 128), os.path.join(static, "favicon.png"))

    write_ico(master, os.path.join(icons, "icon.ico"))
    write_icns(master, os.path.join(icons, "icon.icns"))
    save_png(resample(master, 512), os.path.join(icons, "icon.png"))
    save_png(resample(master, 32), os.path.join(icons, "32x32.png"))
    save_png(resample(master, 128), os.path.join(icons, "128x128.png"))
    save_png(resample(master, 256), os.path.join(icons, "128x128@2x.png"))
    for size in PNG_LOGO_SIZES:
        save_png(resample(master, size), os.path.join(icons, f"Square{size}x{size}Logo.png"))
    save_png(resample(master, STORE_LOGO_SIZE), os.path.join(icons, "StoreLogo.png"))


if __name__ == "__main__":
    main()
