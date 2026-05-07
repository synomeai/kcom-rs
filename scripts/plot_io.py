#!/usr/bin/env python3
import argparse
import csv
import math
import os
import statistics
from collections import defaultdict

METRICS = {
    "throughput_ops_per_s": "ops/s",
    "throughput_bytes_s": "bytes/s",
    "avg_latency_ns": "ns",
    "min_latency_ns": "ns",
    "max_latency_ns": "ns",
}

def parse_args():
    parser = argparse.ArgumentParser(description="Summarize and plot io-bench CSV results")
    parser.add_argument("--in", dest="inputs", nargs="+", required=True, help="input CSV files")
    parser.add_argument("--out-summary", default="io_summary.csv", help="summary CSV output")
    parser.add_argument("--out-svg", default="io_plot.svg", help="SVG plot output")
    parser.add_argument("--metric", choices=METRICS.keys(), default="throughput_bytes_s")
    parser.add_argument("--x", default="size_bytes", help="x axis field (default size_bytes)")
    parser.add_argument("--panel", default="op", help="panel field (default op)")
    return parser.parse_args()

def to_float(value):
    try:
        return float(value)
    except Exception:
        return None

def to_int(value):
    try:
        return int(value)
    except Exception:
        return None

def format_size(value):
    if value is None:
        return ""
    if value % (1024 * 1024) == 0:
        return f"{value // (1024 * 1024)}M"
    if value % 1024 == 0:
        return f"{value // 1024}K"
    return str(value)

def percentile(values, pct):
    if not values:
        return 0.0
    values = sorted(values)
    idx = max(0, min(len(values) - 1, int(math.ceil(pct * len(values))) - 1))
    return values[idx]

def xml_escape(text):
    return (
        text.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
        .replace("'", "&apos;")
    )

def read_rows(paths):
    rows = []
    for path in paths:
        if not os.path.exists(path):
            continue
        with open(path, newline="", encoding="utf-8") as f:
            reader = csv.DictReader(f)
            for row in reader:
                rows.append(row)
    return rows

def summarize(rows, metric):
    groups = defaultdict(list)
    for row in rows:
        device = row.get("device", "")
        mode = row.get("mode", "")
        op = row.get("op", "")
        size = to_int(row.get("size_bytes"))
        value = to_float(row.get(metric))
        if size is None or value is None:
            continue
        key = (device, mode, op, size)
        groups[key].append(value)

    summary = []
    for (device, mode, op, size), values in groups.items():
        mean = statistics.fmean(values) if values else 0.0
        median = statistics.median(values) if values else 0.0
        p90 = percentile(values, 0.90)
        vmin = min(values) if values else 0.0
        vmax = max(values) if values else 0.0
        stdev = statistics.pstdev(values) if len(values) > 1 else 0.0
        summary.append({
            "device": device,
            "mode": mode,
            "op": op,
            "size_bytes": size,
            "metric": metric,
            "count": len(values),
            "mean": mean,
            "median": median,
            "p90": p90,
            "min": vmin,
            "max": vmax,
            "stdev": stdev,
        })
    return summary

def write_summary(path, summary):
    fields = [
        "device",
        "mode",
        "op",
        "size_bytes",
        "metric",
        "count",
        "mean",
        "median",
        "p90",
        "min",
        "max",
        "stdev",
    ]
    with open(path, "w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        for row in summary:
            writer.writerow(row)


def build_series(summary, metric, panel_field):
    panels = defaultdict(lambda: defaultdict(list))
    for row in summary:
        if row["metric"] != metric:
            continue
        panel = row.get(panel_field, "")
        series_key = (row.get("mode", ""), row.get("device", ""))
        size = row.get("size_bytes")
        value = row.get("mean", 0.0)
        panels[panel][series_key].append((size, value))

    for panel, series in panels.items():
        for key, points in series.items():
            points.sort(key=lambda item: item[0])
    return panels


def svg_plot(summary, metric, panel_field, out_path):
    panels = build_series(summary, metric, panel_field)
    if not panels:
        return

    panel_names = list(panels.keys())
    panel_count = len(panel_names)

    width = 1000
    panel_height = 320
    height = panel_height * panel_count
    margin_left = 70
    margin_right = 20
    margin_top = 30
    margin_bottom = 60

    colors = [
        "#1f77b4",
        "#ff7f0e",
        "#2ca02c",
        "#d62728",
        "#9467bd",
        "#8c564b",
        "#e377c2",
        "#7f7f7f",
        "#bcbd22",
        "#17becf",
    ]

    metric_label = METRICS.get(metric, "")
    svg = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}">',
        f'<rect width="100%" height="100%" fill="white" />',
    ]

    for idx, panel_name in enumerate(panel_names):
        top = idx * panel_height
        left = margin_left
        right = width - margin_right
        bottom = top + panel_height - margin_bottom
        top_inner = top + margin_top

        series = panels[panel_name]
        x_values = sorted({size for points in series.values() for size, _ in points})
        if not x_values:
            continue

        y_max = 0.0
        for points in series.values():
            for _, value in points:
                y_max = max(y_max, value)
        if y_max <= 0.0:
            y_max = 1.0

        def x_pos(value):
            if len(x_values) == 1:
                return (left + right) / 2
            min_x = x_values[0]
            max_x = x_values[-1]
            if max_x == min_x:
                return (left + right) / 2
            log_min = math.log2(min_x)
            log_max = math.log2(max_x)
            log_val = math.log2(value)
            ratio = (log_val - log_min) / (log_max - log_min) if log_max > log_min else 0.0
            return left + ratio * (right - left)

        def y_pos(value):
            ratio = value / y_max if y_max > 0 else 0.0
            return bottom - ratio * (bottom - top_inner)

        svg.append(f'<text x="{left}" y="{top + 18}" font-size="14" font-family="Arial">{xml_escape(panel_name)}</text>')
        svg.append(f'<text x="{left}" y="{top + 36}" font-size="12" font-family="Arial">metric: {xml_escape(metric)} ({xml_escape(metric_label)})</text>')

        svg.append(f'<line x1="{left}" y1="{top_inner}" x2="{left}" y2="{bottom}" stroke="#444" />')
        svg.append(f'<line x1="{left}" y1="{bottom}" x2="{right}" y2="{bottom}" stroke="#444" />')

        # Y ticks
        for t in range(5):
            value = y_max * t / 4.0
            y = y_pos(value)
            svg.append(f'<line x1="{left - 4}" y1="{y}" x2="{left}" y2="{y}" stroke="#444" />')
            svg.append(f'<text x="{left - 8}" y="{y + 4}" font-size="10" font-family="Arial" text-anchor="end">{value:.2g}</text>')

        # X ticks
        for value in x_values:
            x = x_pos(value)
            label = format_size(value)
            svg.append(f'<line x1="{x}" y1="{bottom}" x2="{x}" y2="{bottom + 4}" stroke="#444" />')
            svg.append(f'<text x="{x}" y="{bottom + 18}" font-size="10" font-family="Arial" text-anchor="middle">{xml_escape(label)}</text>')

        # Lines
        legend_y = top_inner + 10
        legend_x = right - 10
        legend_items = []
        for line_idx, (series_key, points) in enumerate(sorted(series.items())):
            color = colors[line_idx % len(colors)]
            coords = [f"{x_pos(size):.2f},{y_pos(value):.2f}" for size, value in points]
            svg.append(
                f'<polyline fill="none" stroke="{color}" stroke-width="2" points="{" ".join(coords)}" />'
            )
            for size, value in points:
                svg.append(
                    f'<circle cx="{x_pos(size):.2f}" cy="{y_pos(value):.2f}" r="3" fill="{color}" />'
                )
            label = f"{series_key[0]} | {series_key[1]}"
            legend_items.append((color, label))

        # Legend
        legend_items = legend_items[:8]
        for item_idx, (color, label) in enumerate(legend_items):
            y = legend_y + item_idx * 14
            svg.append(f'<rect x="{legend_x - 140}" y="{y - 8}" width="8" height="8" fill="{color}" />')
            svg.append(
                f'<text x="{legend_x - 125}" y="{y}" font-size="10" font-family="Arial" text-anchor="start">{xml_escape(label)}</text>'
            )

    svg.append("</svg>")
    with open(out_path, "w", encoding="utf-8") as f:
        f.write("\n".join(svg))


def main():
    args = parse_args()
    rows = read_rows(args.inputs)
    summary = summarize(rows, args.metric)
    write_summary(args.out_summary, summary)
    svg_plot(summary, args.metric, args.panel, args.out_svg)

if __name__ == "__main__":
    main()
