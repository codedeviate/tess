# Animated image samples

Tiny example animations for trying out `tess`'s animated-image playback
(added in 0.39.0). All three files are the **same** 8-frame, 120×80 animation —
a hue-cycling disc sweeping left-to-right, looping forever — encoded in each
supported animated format:

| File | Format | tess decode path |
|------|--------|------------------|
| `sweep.gif`  | animated GIF        | `GifDecoder` (loop count read from the NETSCAPE2.0 extension → infinite) |
| `sweep.apng` | animated PNG (APNG) | `PngDecoder::apng()` (8-bit; loop count not parsed → treated as infinite) |
| `sweep.webp` | animated WebP       | `WebPDecoder` (loop count not parsed → treated as infinite) |

Each decodes to 8 frames through `tess`'s `image_render::decode_animation`.

## View them

```sh
tess samples/animated/sweep.gif      # animates in any terminal (ASCII/half-block)
tess samples/animated/sweep.apng
tess samples/animated/sweep.webp
```

Kitty/Sixel terminals render true pixels (`--image-protocol auto`, the default);
elsewhere (e.g. Warp) you get the colored-ASCII rendering. Playback controls:

- `p` — pause / resume (also restarts a finished animation)
- `.` / `,` — step one frame forward / back (pauses)
- `Backspace` — restart
- `--no-animate` — open the static first frame instead of playing

The status line shows `[play i/n]` / `[pause i/n]` / `[done n/n]`.

## How they were generated

Frames drawn with ImageMagick, then encoded with the standard tools (kept here
only for reference — the committed files are the artifacts):

```sh
# 8 frames: a radius-14 disc sweeping x=18..102 on a #101018 background,
# fill colour cycling through 8 hues, written to /tmp/anim_frames/f0..f7.png
# (magick -size 120x80 xc:'#101018' -fill '<hue>' -draw 'circle CX,40 CX,26' fN.png)

magick -delay 12 -loop 0 /tmp/anim_frames/f*.png sweep.gif        # 120 ms/frame, loop forever
ffmpeg -framerate 8 -i /tmp/anim_frames/f%d.png -plays 0 \
       -pix_fmt rgb24 -f apng sweep.apng                          # 8-bit APNG (16-bit isn't decodable)
img2webp -loop 0 -d 120 /tmp/anim_frames/f*.png -o sweep.webp     # animated WebP
```

Note: the APNG must be **8-bit** — the `image` crate's APNG decoder rejects
16-bit (`Rgb16`) frames, in which case `tess` falls back to showing the static
first frame.
