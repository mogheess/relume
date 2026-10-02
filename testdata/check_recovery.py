#!/usr/bin/env python3
"""Compare recovered files against the original corpus by content hash."""
import hashlib, os, sys
corpus, recovered = sys.argv[1], sys.argv[2]
def h(p):
    return hashlib.md5(open(p, 'rb').read()).hexdigest()
orig = {h(os.path.join(corpus, f)): f for f in os.listdir(corpus)}
got = {}
for root, _, files in os.walk(recovered):
    for f in files:
        got.setdefault(h(os.path.join(root, f)), []).append(os.path.relpath(os.path.join(root, f), recovered))
ok = [n for k, n in orig.items() if k in got]
miss = sorted(n for k, n in orig.items() if k not in got)
print(f"exact matches: {len(ok)}/{len(orig)}")
for n in miss:
    print("  MISSING/NOT EXACT:", n)
