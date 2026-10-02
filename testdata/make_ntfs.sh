#!/bin/bash
# Builds work/ntfs.img: an NTFS volume with deleted photos and videos, an emptied Recycle Bin
# (a file and a whole folder) and a file whose space was reused.
#
# Needs ntfs-3g built from source, plus testdata/ntfsops.c compiled against it:
#   NTFS3G=/path/to/ntfs-3g-src   (after ./configure --disable-ntfs-3g && make)
#   cc -DHAVE_CONFIG_H -I$NTFS3G -I$NTFS3G/include/ntfs-3g ntfsops.c \
#      $NTFS3G/libntfs-3g/.libs/libntfs-3g.a -framework CoreFoundation -o ntfsops
#   NTFSOPS=/path/to/ntfsops
set -euo pipefail
cd "$(dirname "$0")"
NTFS3G=${NTFS3G:?set NTFS3G to a built ntfs-3g source tree}
NTFSOPS=${NTFSOPS:?set NTFSOPS to the compiled ntfsops helper}
IMG=work/ntfs.img
rm -f -- "$IMG"
mkfile -n 700m "$IMG"
"$NTFS3G/ntfsprogs/mkntfs" -F -f -q -L RLTNTFS -c 4096 "$IMG"
python3 - > work/ntfs_ops1.txt <<'PY'
import struct
def i_file(path, size):
    p = path.encode('utf-16-le')
    return struct.pack('<QQQI', 2, size, 133500000000000000, len(path) + 1) + p + b'\0\0'
open('work/$IBEACH1.jpg', 'wb').write(i_file(r'C:\Users\Bob\Pictures\Vacation\beach.jpg', 2599521))
open('work/$ITRIP01', 'wb').write(i_file(r'C:\Users\Bob\Desktop\Trip 2024', 0))
B = '/$Recycle.Bin/S-1-5-21-1111-2222-3333-1001'
print(f"""mkdir /Users
mkdir /Users/Bob
mkdir /Users/Bob/Pictures
mkdir /Users/Bob/Videos
mkdir /$Recycle.Bin
mkdir {B}
put work/corpus/IMG_1000.jpg /Users/Bob/Pictures/IMG_1000.jpg
put work/corpus/screenshot.png /Users/Bob/Pictures/small_gap.png
put work/corpus/IMG_1002.jpg /Users/Bob/Pictures/IMG_1002.jpg
put work/corpus/scan.tif /Users/Bob/Pictures/gap2.tif
put work/corpus/IMG_1003.jpg /Users/Bob/Pictures/keep.jpg
del /Users/Bob/Pictures/small_gap.png
del /Users/Bob/Pictures/gap2.tif
put work/corpus/VID_20240704.mp4 /Users/Bob/Videos/fragmented.mp4
put work/corpus/iphone_hevc.mov /Users/Bob/Videos/IMG_0042.mov
put work/corpus/movie.mkv /Users/Bob/Videos/movie.mkv
put work/$IBEACH1.jpg {B}/$IBEACH1.jpg
put work/corpus/IMG_1001.jpg {B}/$RBEACH1.jpg
put work/$ITRIP01 {B}/$ITRIP01
mkdir {B}/$RTRIP01
put work/corpus/IMG_1004.jpg {B}/$RTRIP01/sunset.jpg
put work/corpus/phone.3gp {B}/$RTRIP01/clip.3gp
put work/corpus/IMG_1005.jpg /Users/Bob/Pictures/will_be_overwritten.jpg
del /Users/Bob/Pictures/IMG_1000.jpg
del /Users/Bob/Pictures/IMG_1002.jpg
del /Users/Bob/Videos/fragmented.mp4
del /Users/Bob/Videos/IMG_0042.mov
del /Users/Bob/Videos/movie.mkv
del {B}/$IBEACH1.jpg
del {B}/$RBEACH1.jpg
del {B}/$ITRIP01
del {B}/$RTRIP01/sunset.jpg
del {B}/$RTRIP01/clip.3gp
del {B}/$RTRIP01
del /Users/Bob/Pictures/will_be_overwritten.jpg
put work/corpus/broadcast.ts /Users/Bob/Videos/new_recording.ts""")
PY
"$NTFSOPS" "$IMG" < work/ntfs_ops1.txt
echo "$IMG ready"
