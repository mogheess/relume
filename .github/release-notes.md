## Download

| File | What it is |
|---|---|
| **Relume.exe** | The app. Download and double-click, no installer needed. |
| **relume-cli.exe** | Command line version (double-click for a guided mode). Optional. |
| **Relume-windows-x64.zip** | Both programs plus the README and license, in one zip. |
| **SHA256SUMS.txt** | Checksums to verify the downloads. |

Requires 64-bit Windows 10 or 11.

## First run

1. Download **Relume.exe** and double-click it.
2. Windows may show **"Windows protected your PC"** because the app isn't code-signed yet. Click
   **More info**, then **Run anyway**.
3. Allow the **administrator** prompt. Windows only lets administrators read drives directly.
   Relume only reads; it never changes the drive you scan.
4. Pick where your photos or videos were, press **Start scan**, tick what you want back, and
   **Recover** to a different drive (a USB stick works).

Stop using the affected drive until you've recovered your files, so nothing overwrites them.

## What's in this release

- Recovers deleted photos and videos from NTFS, FAT32 and exFAT drives, memory cards, lost or
  deleted partitions and disk images (31 formats, including iPhone HEIC/HEVC and camera RAW).
- Deep scan finds files even when their records are gone (formatted or damaged drives).
- Every file is checked and labelled Excellent, Good, Damaged or Overwritten, with previews
  read straight from the disk.
- Explorer-style browsing of results with original folder names, plus Recycle Bin files restored
  to their original names.
- Live map of the drive while scanning.

Built from this release's source by GitHub Actions.
