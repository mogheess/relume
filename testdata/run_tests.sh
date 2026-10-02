#!/bin/bash
# End-to-end recovery tests on real disk images (macOS: needs hdiutil/diskutil, ffmpeg, cwebp, Pillow).
# NTFS tests need NTFS3G=<ntfs-3g source dir, built> and NTFSOPS=<compiled testdata/ntfsops.c>; skipped otherwise.
set -e
cd "$(dirname "$0")/.."
cargo build --release -p relume-cli
CLI=target/release/relume-cli
[ -d testdata/work/corpus ] || testdata/make_corpus.sh
cargo test --release -p relume-core
fail=0
check() { # image, expected exact matches
    rm -rf -- "testdata/work/out_${1:?}"
    $CLI scan testdata/work/$1.img --out testdata/work/out_$1 2>/dev/null | tail -2
    python3 testdata/check_recovery.py testdata/work/corpus testdata/work/out_$1 | tee /tmp/relume_$1.txt | head -1
    grep -q "exact matches: $2/" /tmp/relume_$1.txt || { echo "FAIL $1"; fail=1; }
}
[ -f testdata/work/fat32.img ] || testdata/make_image.sh fat32 FAT32 900
[ -f testdata/work/exfat.img ] || testdata/make_image.sh exfat ExFAT 900
check fat32 25
check exfat 25
if [ -n "$NTFSOPS" ] || [ -f testdata/work/ntfs.img ]; then
    [ -f testdata/work/ntfs.img ] || testdata/make_ntfs.sh
    check ntfs 11
fi
# Damage detection
$CLI scan testdata/work/fat32.img --json testdata/work/fat32.json >/dev/null 2>&1
cp testdata/work/fat32.img testdata/work/damaged.img && python3 testdata/damage.py
out=$($CLI scan testdata/work/damaged.img 2>/dev/null | tail -1)
echo "$out"
echo "$out" | grep -q "5 damaged, 1 overwritten" || { echo "FAIL damage detection"; fail=1; }
[ $fail = 0 ] && echo "ALL TESTS PASSED" || { echo "SOME TESTS FAILED"; exit 1; }
