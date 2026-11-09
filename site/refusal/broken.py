#!/usr/bin/env python3
"""Which res:// load targets are no longer where their scenes expect them?

Takes the pristine checkout and the applied copy, and counts only targets that
were THERE BEFORE and are gone after.

Both restrictions matter and both were caught by running this:

  * Load sites only -- ext_resource path=, preload(), load(). A res:// in a
    comment is not something Godot resolves.
  * Present-before only. Without it this reported 9 rather than 6, because
    res://Translations/%s.po is a format string built at runtime and never a
    file on disk. Counting targets that never existed would have inflated the
    number the demo turns on, in the demo's favour.
"""
import os, re, sys

pristine = os.path.realpath(sys.argv[1])
root = os.path.realpath(sys.argv[2])
pats = [re.compile(r'\[ext_resource[^\]]*\bpath="res://([^"]+)"'),
        re.compile(r'\bpreload\(\s*"res://([^"]+)"'),
        re.compile(r'\bload\(\s*"res://([^"]+)"')]

sites = {}
for r, _, files in os.walk(root):
    for fn in files:
        if not fn.endswith(('.tscn', '.tres', '.gd', '.godot')):
            continue
        try:
            txt = open(os.path.join(r, fn), encoding='utf-8', errors='ignore').read()
        except OSError:
            continue
        for pat in pats:
            for m in pat.finditer(txt):
                sites.setdefault(m.group(1), []).append(os.path.relpath(os.path.join(r, fn), root))

present = {t: v for t, v in sites.items() if os.path.exists(os.path.join(pristine, t))}
missing = {t: v for t, v in present.items() if not os.path.exists(os.path.join(root, t))}
for t in sorted(missing)[:5]:
    print(f"  MISSING  res://{t}")
    print(f"           loaded by {sorted(set(missing[t]))[0]}")
print(f"\n  {len(missing)} of {len(present)} load targets can no longer be found.")
