#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.13"
# dependencies = ["matplotlib==3.10.6"]
# ///
"""Regenerate the deterministic chart fixture; strip only DTD and metadata."""
from io import StringIO
from pathlib import Path
import xml.etree.ElementTree as ET

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

matplotlib.rcParams["svg.hashsalt"] = "phoenix-present-svg-fixture"
fig, ax = plt.subplots(figsize=(6, 3))
labels = ["Projects", "Downloads", "Media"]
values = [42.5, 18.25, 9.75]
bars = ax.barh(labels, values, color=["#2563eb", "#14b8a6", "#a855f7"])
ax.bar_label(bars, labels=[f"{v:.2f} GiB" for v in values], padding=4)
ax.set_xlim(0, 55)
ax.set_xlabel("Storage (GiB)")
ax.set_title("Largest storage consumers")
fig.tight_layout()
output = StringIO()
fig.savefig(output, format="svg", metadata={"Date": None})
ET.register_namespace("", "http://www.w3.org/2000/svg")
ET.register_namespace("xlink", "http://www.w3.org/1999/xlink")
root = ET.fromstring(output.getvalue())
for metadata in root.findall("{http://www.w3.org/2000/svg}metadata"):
    root.remove(metadata)
Path(__file__).with_name("matplotlib-bars.svg").write_bytes(ET.tostring(root, encoding="utf-8"))
