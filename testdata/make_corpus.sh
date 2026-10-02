#!/bin/bash
# Generates a corpus of test photos & videos in testdata/work/corpus.
set -e
cd "$(dirname "$0")"
OUT=work/corpus
rm -rf "$OUT"; mkdir -p "$OUT"
python3 - "$OUT" <<'PY'
import sys, random, os
from PIL import Image, ImageDraw
out = sys.argv[1]
random.seed(7)
def photo(w, h, seed):
    random.seed(seed)
    im = Image.effect_noise((w, h), 60).convert("RGB")
    d = ImageDraw.Draw(im)
    for _ in range(40):
        x, y = random.randrange(w), random.randrange(h)
        d.ellipse([x, y, x + random.randrange(20, 300), y + random.randrange(20, 300)],
                  fill=tuple(random.randrange(256) for _ in range(3)))
    return im
for i in range(6):
    im = photo(2400 + i * 100, 1600, i)
    ex = Image.Exif()
    ex[0x010F] = "Canon"; ex[0x0110] = "Canon EOS R6"
    ex[0x0132] = f"2024:0{i+1}:1{i} 10:2{i}:00"
    ifd = ex.get_ifd(0x8769); ifd[0x9003] = f"2024:0{i+1}:1{i} 10:2{i}:00"
    im.save(f"{out}/IMG_{1000+i}.jpg", quality=90, exif=ex.tobytes())
photo(1200, 900, 20).save(f"{out}/progressive.jpg", quality=85, progressive=True)
photo(1024, 768, 21).save(f"{out}/screenshot.png")
photo(800, 600, 22).save(f"{out}/picture.bmp")
photo(1600, 1200, 23).save(f"{out}/scan.tif", compression="tiff_lzw")
frames = [photo(320, 240, 30 + k).convert("P") for k in range(8)]
frames[0].save(f"{out}/anim.gif", save_all=True, append_images=frames[1:], duration=100, loop=0)
photo(1280, 720, 24).save(f"{out}/wallpaper_src.png")
PY
cwebp -quiet -q 80 "$OUT/wallpaper_src.png" -o "$OUT/wallpaper.webp"; rm "$OUT/wallpaper_src.png"
sips -s format heic "$OUT/IMG_1001.jpg" --out "$OUT/IMG_2001.heic" >/dev/null
SRC="-f lavfi -i testsrc2=size=1280x720:rate=30,noise=alls=40:allf=t -f lavfi -i sine=frequency=440:sample_rate=44100"
ff() { ffmpeg -hide_banner -loglevel error -y $SRC -t "$1" "${@:2}"; }
ff 6 -c:v libx264 -pix_fmt yuv420p -c:a aac -metadata creation_time=2024-07-04T12:00:00Z "$OUT/VID_20240704.mp4"
ff 4 -c:v libx264 -pix_fmt yuv420p -c:a aac -movflags +faststart "$OUT/clip_faststart.mp4"
ff 4 -c:v libx265 -tag:v hvc1 -pix_fmt yuv420p -c:a aac "$OUT/iphone_hevc.mov"
ff 4 -c:v libvpx-vp9 -b:v 1M -c:a libopus "$OUT/web.webm" 2>/dev/null || ff 4 -c:v libvpx -b:v 1M -an "$OUT/web.webm"
ff 4 -c:v libx264 -pix_fmt yuv420p -c:a aac "$OUT/movie.mkv"
ff 4 -c:v mpeg4 -q:v 4 -c:a mp3 "$OUT/old_camera.avi" 2>/dev/null || ff 4 -c:v mpeg4 -q:v 4 -an "$OUT/old_camera.avi"
ff 4 -c:v wmv2 -b:v 2M -c:a wmav2 "$OUT/windows.wmv"
ff 4 -c:v flv -b:v 2M -c:a mp3 "$OUT/stream.flv" 2>/dev/null || ff 4 -c:v flv -b:v 2M -an "$OUT/stream.flv"
ff 4 -c:v mpeg2video -b:v 4M -c:a mp2 -f vob "$OUT/dvd.vob"
ff 4 -c:v libx264 -pix_fmt yuv420p -c:a aac -f mpegts "$OUT/broadcast.ts"
ff 4 -c:v libx264 -pix_fmt yuv420p -c:a aac -f mpegts -mpegts_m2ts_mode 1 "$OUT/00001.mts"
ffmpeg -hide_banner -loglevel error -y -f lavfi -i testsrc2=size=352x288:rate=15,noise=alls=30:allf=t -t 4 -c:v h263 "$OUT/phone.3gp"
ls -la "$OUT"
