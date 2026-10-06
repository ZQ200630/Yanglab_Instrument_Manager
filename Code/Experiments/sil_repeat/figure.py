"""Per-cycle annotations and aggregate figures for repeated SIL scans."""

from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path
from typing import Any

import matplotlib as mpl
import matplotlib.pyplot as plt
import numpy as np
from matplotlib.figure import Figure


def annotate_cycle(figure: Figure, cycle_index: int, total_cycles: int) -> Figure:
    figure.text(
        0.99,
        0.012,
        f"Cycle {cycle_index + 1}/{total_cycles}",
        fontsize=6,
        ha="right",
        va="bottom",
    )
    return figure


def build_repeat_summary_figure(rows: Sequence[Any]) -> Figure:
    ordered = sorted(rows, key=lambda row: row.cycle_index)
    if not ordered:
        raise ValueError("repeat summary requires at least one cycle")
    plt.rcParams["font.family"] = "sans-serif"
    plt.rcParams["font.sans-serif"] = ["Arial", "DejaVu Sans", "Liberation Sans"]
    plt.rcParams["svg.fonttype"] = "none"
    plt.rcParams["pdf.fonttype"] = 42
    mpl.rcParams.update({"svg.fonttype": "none", "pdf.fonttype": 42})
    cycles = np.asarray([row.cycle_index + 1 for row in ordered], dtype=int)
    mae = np.asarray([row.p95_signal_mae_db for row in ordered], dtype=float)
    shift = np.asarray([row.maximum_absolute_peak_shift_nm for row in ordered], dtype=float)
    figure, axis = plt.subplots(figsize=(7.0866, 3.2), constrained_layout=True)
    axis.plot(cycles, mae, color="#0F4D92", marker="o", markersize=2.5, linewidth=1.2)
    axis.set_xlabel("Cycle")
    axis.set_ylabel("P95 signal-region MAE (dB)", color="#0F4D92")
    axis.tick_params(axis="y", colors="#0F4D92")
    axis.set_xlim(1, max(int(cycles[-1]), 2))
    shift_axis = axis.twinx()
    shift_axis.plot(cycles, shift, color="#626262", marker="s", markersize=2.5, linewidth=1.0)
    shift_axis.set_ylabel("Maximum |peak shift| (nm)", color="#626262")
    shift_axis.tick_params(axis="y", colors="#626262")
    axis.set_title("Repeated SIL forward/reverse consistency", fontweight="bold")
    figure.patch.set_facecolor("white")
    figure._sil_provenance = (
        "REAL run" if ordered[0].run_mode == "real" else f"SIMULATED run; mode={ordered[0].run_mode}"
    )
    return figure


def save_repeat_summary_figure(rows: Sequence[Any], base_path: Path) -> tuple[Path, ...]:
    figure = build_repeat_summary_figure(rows)
    base_path = Path(base_path)
    base_path.parent.mkdir(parents=True, exist_ok=True)
    provenance = str(getattr(figure, "_sil_provenance", "SIL repeat run"))
    paths = tuple(base_path.with_suffix(suffix) for suffix in (".svg", ".pdf", ".png"))
    metadata = {
        ".svg": {"Title": "Repeated SIL cycle summary", "Description": provenance},
        ".pdf": {"Title": "Repeated SIL cycle summary", "Subject": provenance},
        ".png": {"Title": "Repeated SIL cycle summary", "Description": provenance},
    }
    try:
        for path in paths:
            kwargs = {"metadata": metadata[path.suffix]}
            if path.suffix == ".png":
                kwargs["dpi"] = 300
            figure.savefig(path, **kwargs)
    finally:
        plt.close(figure)
    return paths
