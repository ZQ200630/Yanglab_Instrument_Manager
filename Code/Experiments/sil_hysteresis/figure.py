"""Static, editable comparison figures for offline SIL hysteresis analysis."""

from __future__ import annotations

import logging
import math
from dataclasses import dataclass
from pathlib import Path

import matplotlib as mpl
import matplotlib.pyplot as plt
import numpy as np
from matplotlib.colors import LinearSegmentedColormap, Normalize, TwoSlopeNorm
from matplotlib.figure import Figure

from .analysis import ComparisonResult


LOGGER = logging.getLogger(__name__)


@dataclass(frozen=True)
class OperationMetadata:
    """Operation point displayed with a completed forward/reverse comparison."""

    temperature_c: float
    current_ma: float
    operation_index: int
    run_mode: str = "real"

    def __post_init__(self) -> None:
        for name, value in (("temperature_c", self.temperature_c), ("current_ma", self.current_ma)):
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(float(value)):
                raise ValueError(f"{name} must be finite")
        if isinstance(self.operation_index, bool) or not isinstance(self.operation_index, int) or self.operation_index < 0:
            raise ValueError("operation_index must be a non-negative integer")
        if self.run_mode not in {"real", "similar", "hysteretic"}:
            raise ValueError("run_mode must be real, similar, or hysteretic")
        object.__setattr__(self, "temperature_c", float(self.temperature_c))
        object.__setattr__(self, "current_ma", float(self.current_ma))


def _configure_publication_style() -> None:
    """Set editable-text defaults before any figure is created."""
    plt.rcParams["font.family"] = "sans-serif"
    plt.rcParams["font.sans-serif"] = ["Arial", "DejaVu Sans", "Liberation Sans"]
    plt.rcParams["svg.fonttype"] = "none"
    plt.rcParams["pdf.fonttype"] = 42
    mpl.rcParams.update({"svg.fonttype": "none", "pdf.fonttype": 42})
    plt.rcParams["font.size"] = 7
    plt.rcParams["axes.spines.right"] = False
    plt.rcParams["axes.spines.top"] = False
    plt.rcParams["axes.linewidth"] = 0.7


def _axis_limits(values: np.ndarray) -> tuple[float, float]:
    lower = float(np.min(values))
    upper = float(np.max(values))
    if lower == upper:
        padding = max(abs(lower) * 0.02, 0.01)
        return lower - padding, upper + padding
    return lower, upper


def _uniform_cell_extent(values: np.ndarray, label: str) -> tuple[float, float]:
    """Return outer cell edges for one or more uniform sample centers."""
    coordinates = np.asarray(values, dtype=float)
    if coordinates.ndim != 1 or coordinates.size < 1 or not np.all(np.isfinite(coordinates)):
        raise ValueError(f"{label} coordinates must be a finite one-dimensional array")
    if coordinates.size == 1:
        # No neighbor exists from which to infer spacing; a symmetric unit-width
        # display cell preserves the sole stored coordinate as the exact center.
        center = float(coordinates[0])
        return center - 0.5, center + 0.5
    steps = np.diff(coordinates)
    if not np.all(steps > 0.0):
        raise ValueError(f"{label} coordinates must be strictly increasing")
    if not np.allclose(steps, steps[0], rtol=1e-10, atol=1e-12):
        raise ValueError(f"{label} coordinates must be uniformly spaced")
    half_step = float(steps[0]) / 2.0
    return float(coordinates[0] - half_step), float(coordinates[-1] + half_step)


def _add_panel_label(axis, label: str) -> None:
    axis.text(
        -0.12,
        1.04,
        label,
        transform=axis.transAxes,
        fontsize=8,
        fontweight="bold",
        ha="left",
        va="bottom",
    )


def _spectrogram(
    axis,
    values: np.ndarray,
    v2_extent: tuple[float, float],
    wavelength_extent: tuple[float, float],
    power_norm: Normalize,
):
    return axis.imshow(
        values.T,
        origin="lower",
        aspect="auto",
        extent=(*v2_extent, *wavelength_extent),
        cmap="Blues",
        norm=power_norm,
    )


def _draw_comparison_figure(
    figure: Figure, result: ComparisonResult, metadata: OperationMetadata
) -> Figure:
    """Populate ``figure`` without creating a second window or changing result data."""
    v_squared = np.square(result.voltage_v)
    v2_extent = _uniform_cell_extent(v_squared, "voltage-squared")
    wavelength_extent = _uniform_cell_extent(result.wavelength_nm, "wavelength")
    all_power = np.concatenate((result.forward_dbm.ravel(), result.reverse_dbm.ravel()))
    power_norm = Normalize(vmin=float(np.min(all_power)), vmax=float(np.max(all_power)))
    delta_limit = float(np.percentile(np.abs(result.difference_db), 99.0))
    delta_limit = max(delta_limit, float(np.finfo(float).eps))
    difference_norm = TwoSlopeNorm(vmin=-delta_limit, vcenter=0.0, vmax=delta_limit)
    difference_cmap = LinearSegmentedColormap.from_list(
        "blue_neutral_gold", ["#1F5A99", "#F7F7F7", "#B2761D"]
    )

    grid = figure.add_gridspec(2, 6, height_ratios=(1.0, 1.25), hspace=0.62, wspace=0.82)
    forward_axis = figure.add_subplot(grid[0, :3])
    reverse_axis = figure.add_subplot(grid[0, 3:])
    difference_axis = figure.add_subplot(grid[1, :4])
    metric_axis = figure.add_subplot(grid[1, 4:])
    figure.subplots_adjust(left=0.08, right=0.93, bottom=0.15, top=0.88)

    forward_image = _spectrogram(
        forward_axis, result.forward_dbm, v2_extent, wavelength_extent, power_norm
    )
    _spectrogram(reverse_axis, result.reverse_dbm, v2_extent, wavelength_extent, power_norm)
    for axis, title, label in (
        (forward_axis, "Forward sweep", "a"),
        (reverse_axis, "Reverse sweep", "b"),
    ):
        axis.set_title(title, fontsize=7, fontweight="bold")
        axis.set_xlabel("V² (V²)")
        axis.set_ylabel("Wavelength (nm)")
        axis.tick_params(labelsize=6)
        _add_panel_label(axis, label)
    power_bar = figure.colorbar(forward_image, ax=(forward_axis, reverse_axis), pad=0.02, shrink=0.88)
    power_bar.set_label("Power (dBm)", fontsize=6)
    power_bar.ax.tick_params(labelsize=6)

    difference_image = difference_axis.imshow(
        result.difference_db.T,
        origin="lower",
        aspect="auto",
        extent=(*v2_extent, *wavelength_extent),
        cmap=difference_cmap,
        norm=difference_norm,
    )
    difference_axis.set_title("Forward − reverse power", fontsize=7, fontweight="bold")
    difference_axis.set_xlabel("V² (V²)")
    difference_axis.set_ylabel("Wavelength (nm)")
    difference_axis.tick_params(labelsize=6)
    _add_panel_label(difference_axis, "c")
    difference_bar = figure.colorbar(difference_image, ax=difference_axis, pad=0.02)
    difference_bar.set_label("Difference (dB)", fontsize=6)
    difference_bar.ax.tick_params(labelsize=6)

    metric_axis.plot(
        v_squared,
        result.signal_mae_db,
        color="#0F4D92",
        marker="o",
        linewidth=1.4,
        markersize=2.2,
        label="Signal-region MAE",
    )
    metric_axis.set_xlabel("V² (V²)")
    metric_axis.set_ylabel("MAE (dB)", color="#0F4D92")
    metric_axis.tick_params(axis="both", labelsize=6)
    metric_axis.tick_params(axis="y", colors="#0F4D92")
    shift_axis = metric_axis.twinx()
    shift_axis.plot(
        v_squared,
        np.abs(result.peak_shift_nm),
        color="#626262",
        marker="s",
        linestyle="None",
        markersize=2.2,
        label="Absolute peak shift",
    )
    shift_axis.set_ylabel("|Peak shift| (nm)", color="#626262")
    shift_axis.tick_params(axis="y", labelsize=6, colors="#626262")
    metric_axis.set_title("Signal discrepancy", fontsize=7, fontweight="bold")
    if v_squared.size == 1:
        # A singleton scan is categorical evidence at its stored coordinate;
        # suppress automatically generated negative V² tick labels.
        for axis in (forward_axis, reverse_axis, difference_axis, metric_axis):
            axis.set_xticks(v_squared)
    largest_index = int(np.argmax(result.signal_mae_db))
    mae_lower, mae_upper = _axis_limits(result.signal_mae_db)
    mae_span = mae_upper - mae_lower
    metric_axis.set_ylim(mae_lower, mae_upper + 0.40 * mae_span)
    shift_lower, shift_upper = _axis_limits(np.abs(result.peak_shift_nm))
    shift_span = shift_upper - shift_lower
    shift_axis.set_ylim(shift_lower, shift_upper + 0.40 * shift_span)
    relative_index = largest_index / max(len(result.signal_mae_db) - 1, 1)
    if relative_index < 0.25:
        annotation_x = max(relative_index, 0.02)
        annotation_alignment = "left"
    elif relative_index > 0.75:
        annotation_x = min(relative_index, 0.98)
        annotation_alignment = "right"
    else:
        annotation_x = relative_index
        annotation_alignment = "center"
    metric_axis.annotate(
        f"Largest\n{result.voltage_v[largest_index]:.2f} V",
        xy=(v_squared[largest_index], result.signal_mae_db[largest_index]),
        xytext=(annotation_x, 0.97),
        textcoords=metric_axis.transAxes,
        fontsize=6,
        color="#0F4D92",
        ha=annotation_alignment,
        va="top",
        arrowprops={"arrowstyle": "-", "color": "#0F4D92", "linewidth": 0.7},
    )
    _add_panel_label(metric_axis, "d")

    provenance_title = "" if metadata.run_mode == "real" else (
        f"SIMULATED — mode: {metadata.run_mode}\n"
    )
    figure.suptitle(
        provenance_title + "SIL forward/reverse — "
        f"T {metadata.temperature_c:.1f} °C, I {metadata.current_ma:.1f} mA — "
        f"Classification: {result.classification}",
        fontsize=8,
        fontweight="bold",
        y=0.985,
    )
    figure.text(
        0.01,
        0.012,
        "One forward turning-point observation excluded; signal mask: " + result.signal_mask_rule,
        fontsize=6,
        ha="left",
        va="bottom",
    )
    figure.patch.set_facecolor("white")
    figure._sil_provenance = (
        "REAL run"
        if metadata.run_mode == "real"
        else f"SIMULATED run; mode={metadata.run_mode}"
    )
    return figure


def build_comparison_figure(result: ComparisonResult, metadata: OperationMetadata) -> Figure:
    """Build the 180 mm-wide static comparison figure from complete raw metrics."""
    if not isinstance(result, ComparisonResult):
        raise TypeError("result must be a ComparisonResult")
    if not isinstance(metadata, OperationMetadata):
        raise TypeError("metadata must be an OperationMetadata")
    _configure_publication_style()
    fig_width_mm = 180.0
    fig_height_mm = 125.0
    figure = plt.figure(figsize=(fig_width_mm / 25.4, fig_height_mm / 25.4), facecolor="white")
    return _draw_comparison_figure(figure, result, metadata)


def save_comparison_figure(figure: Figure, base_path: Path) -> tuple[Path, ...]:
    """Write editable vector files plus a 300 dpi preview, then release the figure."""
    if not isinstance(figure, Figure):
        raise TypeError("figure must be a matplotlib Figure")
    base = Path(base_path).with_suffix("")
    base.parent.mkdir(parents=True, exist_ok=True)
    suffixes = (".svg", ".pdf", ".png")
    paths = tuple(base.with_suffix(suffix) for suffix in suffixes)
    for path in paths:
        provenance = getattr(figure, "_sil_provenance", "REAL run")
        metadata = {
            ".svg": {"Title": "SIL hysteresis comparison", "Description": provenance},
            ".pdf": {
                "Title": "SIL hysteresis comparison",
                "Subject": provenance,
                "Keywords": provenance,
            },
            ".png": {"Title": "SIL hysteresis comparison", "Description": provenance},
        }[path.suffix]
        figure.savefig(path, dpi=300, facecolor="white", metadata=metadata)
    plt.close(figure)
    return paths


class LatestResultWindow:
    """A best-effort, non-blocking single window for the latest completed comparison."""

    def __init__(self, pyplot=plt) -> None:
        self.pyplot = pyplot
        self.figure: Figure | None = None
        self.enabled = True

    def update(self, result: ComparisonResult, metadata: OperationMetadata) -> None:
        """Refresh the one display figure; GUI failures are isolated from experiment state."""
        if not self.enabled:
            return
        try:
            if self.figure is None:
                _configure_publication_style()
                self.figure = self.pyplot.figure()
                _draw_comparison_figure(self.figure, result, metadata)
                self.pyplot.show(block=False)
            else:
                self.figure.clear()
                _draw_comparison_figure(self.figure, result, metadata)
            self.figure.canvas.draw_idle()
            self.figure.canvas.flush_events()
        except Exception as error:  # GUI backends must never interrupt an experiment.
            self.enabled = False
            LOGGER.warning("Latest-result display disabled after GUI error: %s", error)
