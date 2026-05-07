#!/usr/bin/env python3
import argparse
import csv
import os
import re


def parse_args():
    parser = argparse.ArgumentParser(description="Split io-bench CSVs by mode")
    parser.add_argument(
        "--in",
        dest="inputs",
        nargs="+",
        default=["io_kernel.csv", "io_user.csv"],
        help="input CSV files (default: io_kernel.csv io_user.csv)",
    )
    parser.add_argument(
        "--out-dir",
        default="artifacts",
        help="output directory (default: artifacts)",
    )
    parser.add_argument(
        "--prefix",
        default="io_",
        help="output filename prefix (default: io_)",
    )
    return parser.parse_args()


def sanitize(name: str) -> str:
    if not name:
        return "unknown"
    name = name.strip().lower()
    name = re.sub(r"[^a-z0-9._-]+", "-", name)
    name = re.sub(r"-{2,}", "-", name).strip("-")
    return name or "unknown"


def read_rows(paths):
    rows = []
    fieldnames = None
    for path in paths:
        if not os.path.exists(path):
            continue
        with open(path, newline="", encoding="utf-8") as f:
            reader = csv.DictReader(f)
            if fieldnames is None:
                fieldnames = reader.fieldnames or []
            for row in reader:
                if None in row:
                    row.pop(None, None)
                rows.append(row)
    return fieldnames or [], rows


def write_split(out_dir, prefix, fieldnames, rows):
    os.makedirs(out_dir, exist_ok=True)
    buckets = {}
    for row in rows:
        mode = sanitize(row.get("mode", ""))
        buckets.setdefault(mode, []).append(row)

    for mode, items in buckets.items():
        out_path = os.path.join(out_dir, f"{prefix}{mode}.csv")
        with open(out_path, "w", newline="", encoding="utf-8") as f:
            writer = csv.DictWriter(f, fieldnames=fieldnames)
            writer.writeheader()
            for row in items:
                writer.writerow(row)


def main():
    args = parse_args()
    fieldnames, rows = read_rows(args.inputs)
    if not fieldnames:
        return
    write_split(args.out_dir, args.prefix, fieldnames, rows)


if __name__ == "__main__":
    main()
