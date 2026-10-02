#!/usr/bin/env python3
"""Copy-on-write damage scenarios for testing health detection (writes into damaged.img)."""
import json, os
files = {f['name']: f for f in json.load(open('testdata/work/fat32.json'))}
img = open('testdata/work/damaged.img', 'r+b')
def at(name, frac):
    f = files[name]; e = f['extents'][0]
    return e['offset'] + int(f['size'] * frac) // 4096 * 4096
# 1) JPEG: 64 KB in the middle replaced by random data (space reused by another file)
img.seek(at('IMG_1001.jpg', 0.5)); img.write(os.urandom(65536))
# 2) MP4: first 4 KB zeroed (wiped / SSD TRIM)
img.seek(at('clip_faststart.mp4', 0)); img.write(b'\0' * 4096)
# 3) MP4 with index at the end: last 1 MB destroyed
f = files['VID_20240704.mp4']; e = f['extents'][0]
img.seek(e['offset'] + f['size'] - (1 << 20)); img.write(os.urandom(1 << 20))
# 4) MOV: 8 MB of frames in the middle zeroed
img.seek(at('iphone_hevc.mov', 0.6)); img.write(b'\0' * (8 << 20))
# 5) PNG: tail corrupted
f = files['screenshot.png']; e = f['extents'][0]
img.seek(e['offset'] + f['size'] - 300); img.write(os.urandom(300))
# 6) JPEG: second half zeroed (classic "grey bottom half")
img.seek(at('IMG_1003.jpg', 0.5)); img.write(b'\0' * (files['IMG_1003.jpg']['size'] // 2 + 4096))
img.close()
print("damage applied")
