#!/bin/bash
# Usage: make_image.sh <name> <FAT32|ExFAT> <sizeMB>
# Builds work/<name>.img (a raw MBR disk image), copies the test corpus onto it and deletes
# most files, leaving deleted data to recover. It only ever touches the image it attaches.
set -euo pipefail
cd "$(dirname "$0")"
NAME=${1:?name}; FS=${2:?FAT32 or ExFAT}; SIZE=${3:?size in MB}
IMG="work/$NAME.img"
LABEL="RLT$(printf '%05d' $((RANDOM % 100000)))"   # unique: never matches a real volume
mkdir -p work
rm -f -- "$IMG"
mkfile -n "${SIZE}m" "$IMG"
DEV=$(hdiutil attach -imagekey diskimage-class=CRawDiskImage -nomount "$IMG" | awk 'NR==1 {print $1}')
if [[ ! "$DEV" =~ ^/dev/disk[0-9]+$ ]] || ! diskutil info "$DEV" | grep -qE "Protocol: +Disk Image"; then
    echo "refusing to continue: '$DEV' is not the disk image we attached" >&2
    [[ "$DEV" =~ ^/dev/disk[0-9]+$ ]] && hdiutil detach "$DEV" >/dev/null 2>&1 || true
    exit 1
fi
trap 'hdiutil detach "$DEV" >/dev/null 2>&1 || true' EXIT
diskutil eraseDisk "$FS" "$LABEL" MBRFormat "$DEV" >/dev/null
VOL=$(diskutil info "${DEV}s1" | awk -F': +' '/Mount Point/ {print $2}')
[[ "$VOL" == /Volumes/"$LABEL"* ]] || { echo "unexpected mount point '$VOL'" >&2; exit 1; }
mdutil -i off "$VOL" >/dev/null 2>&1 || true
touch "$VOL/.metadata_never_index"
mkdir -p "$VOL/DCIM/100CANON" "$VOL/Videos" "$VOL/Keep"
C=work/corpus
cp -X "$C"/IMG_*.jpg "$C"/*.heic "$VOL/DCIM/100CANON/"
cp -X "$C"/*.png "$C"/*.bmp "$C"/*.tif "$C"/*.gif "$C"/*.webp "$C"/progressive.jpg "$VOL/"
cp -X "$C"/*.mp4 "$C"/*.mov "$C"/*.mkv "$C"/*.webm "$C"/*.avi "$C"/*.wmv "$C"/*.flv "$C"/*.vob "$C"/*.ts "$C"/*.mts "$C"/*.3gp "$VOL/Videos/"
cp -X "$C"/IMG_1000.jpg "$VOL/Keep/still_here.jpg"
sync
# Delete everything except Keep/ (inside the image's own volume only).
rm -rf -- "${VOL:?}/DCIM" "${VOL:?}/Videos"
find "${VOL:?}" -maxdepth 1 -type f ! -name '._*' \( -name '*.png' -o -name '*.bmp' -o -name '*.tif' -o -name '*.gif' -o -name '*.webp' -o -name '*.jpg' \) -delete
sync
echo "$IMG ready"
