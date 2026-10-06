#!/usr/bin/env python3
"""Render independent compatibility metrics from a Test262 report."""
import argparse
import html
import json
from pathlib import Path


def render(report, source_name="full-results.json"):
    esc = lambda value: html.escape(str(value))
    total = report["total"]
    passed = report["counts"]["pass"]
    selection = ", ".join(report["selection"])
    scope = "full selected corpus" if report["selection"] == ["."] else "selected subset only"
    nonpassing = [row for row in report["results"] if row["status"] != "pass"]
    rows = "".join(f'<tr><td>{esc(row["test"])}</td><td>{esc(row["variant"])}</td>'
                   f'<td>{esc(row["status"])}</td><td>{esc(row.get("engine", {}).get("message", row.get("message", "")))}</td></tr>'
                   for row in nonpassing[:200])
    counts = " · ".join(f"{esc(name)}: {count}" for name, count in report["counts"].items())
    limits = "".join(f"<li>{esc(item)}</li>" for item in report.get("limitations", []))
    return f'''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>napi-vm compatibility evidence</title><style>body{{font:16px system-ui;max-width:1200px;margin:40px auto;padding:0 20px}}table{{border-collapse:collapse;width:100%;margin:24px 0}}td,th{{border:1px solid #ccc;text-align:left;padding:10px}}th{{background:#eee}}td{{overflow-wrap:anywhere}}.note{{color:#555}}</style>
<h1>napi-vm compatibility evidence</h1>
<table><tr><th>Domain</th><th>Measurement</th></tr>
<tr><td>ECMAScript / Test262</td><td>{passed}/{total} variants ({scope})</td></tr>
<tr><td>Web APIs / WPT</td><td>Unmeasured</td></tr><tr><td>Node tests</td><td>Unmeasured</td></tr>
<tr><td>npm package corpus</td><td>Unmeasured</td></tr></table>
<p>Test262 revision: <code>{esc(report['revision'])}</code><br>Selection: <code>{esc(selection)}</code><br>{counts}</p>
<p class="note">Denominator: {esc(report['denominator'])}. This is a development measurement, not a stable engine claim. Metrics are never combined.</p>
<h2>Runner limitations</h2><ul>{limits}</ul><h2>Non-passing variants</h2>
<p>Showing up to 200 of {len(nonpassing)} non-passing variants. <a href="{esc(source_name)}">Full JSON report</a></p>
<table><tr><th>Test</th><th>Variant</th><th>Outcome</th><th>Details</th></tr>{rows}</table></html>'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--output", type=Path, default=Path("compatibility.html"))
    args = parser.parse_args()
    args.output.write_text(render(json.loads(args.report.read_text(encoding="utf-8")), args.report.name), encoding="utf-8")

if __name__ == "__main__": main()
