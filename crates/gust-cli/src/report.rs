//! Self-contained HTML report + JSON run artifacts for a finished Gust run.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use gust_core::{Diagnosis, Knee, SloCapacity, StepSummary, Summary, WindowMetric};
use serde::{Deserialize, Serialize};

/// Bump when the on-disk JSON shape changes in a breaking way.
pub const SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub url: String,
    pub profile: String,
    pub duration_secs: u64,
    pub sent: u64,
    pub started_at: String,
    pub summary: Summary,
    pub steps: Vec<StepSummary>,
    pub windows: Vec<WindowMetric>,
    pub knee: Option<Knee>,
    /// SLO-driven capacity, present when the run was given a `--slo-p99-ms`.
    #[serde(default)]
    pub slo: Option<SloCapacity>,
    /// Plain-English auto-diagnosis of what happened.
    #[serde(default)]
    pub diagnosis: Option<Diagnosis>,
    /// Why the first failed request failed, if any did.
    pub failure_reason: Option<String>,
}

pub fn write_html(path: &Path, report: &RunReport) -> Result<()> {
    ensure_parent(path)?;
    let html = render(report);
    fs::write(path, html).with_context(|| format!("write report {}", path.display()))?;
    Ok(())
}

pub fn write_json(path: &Path, report: &RunReport) -> Result<()> {
    ensure_parent(path)?;
    let json = serde_json::to_string_pretty(report).context("serialize run JSON")?;
    fs::write(path, json).with_context(|| format!("write JSON {}", path.display()))?;
    Ok(())
}

pub fn load_json(path: &Path) -> Result<RunReport> {
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let report: RunReport =
        serde_json::from_str(&raw).with_context(|| format!("parse JSON {}", path.display()))?;
    Ok(report)
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create report dir {}", parent.display()))?;
    }
    Ok(())
}

fn render(r: &RunReport) -> String {
    let data = serde_json::to_string(r).unwrap_or_else(|_| "{}".into());
    let dead_target = r.summary.total > 0 && r.summary.success == 0;
    let knee_banner = knee_banner_html(r.knee.as_ref(), dead_target, r.failure_reason.as_deref());
    let slo_banner = slo_banner_html(r.slo.as_ref(), dead_target);

    let diagnosis_panel = match &r.diagnosis {
        Some(d) => {
            let bullets: String = d
                .evidence
                .iter()
                .map(|e| format!("<li>{}</li>", escape(e)))
                .collect();
            format!(
                "<div class=\"diagnosis\">\
                 <div class=\"diag-label\">diagnosis · {}</div>\
                 <strong>{}</strong>\
                 <p class=\"muted\">{}</p>\
                 <ul>{}</ul>\
                 </div>",
                escape(d.cause.label()),
                escape(&d.headline),
                escape(&d.narrative),
                bullets
            )
        }
        None => String::new(),
    };

    let s = &r.summary;
    let pct = |n: u64| {
        if s.total == 0 {
            0.0
        } else {
            n as f64 / s.total as f64 * 100.0
        }
    };

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>Gust report · {url}</title>
<style>
  :root {{
    --bg: #0f1419;
    --panel: #1a222c;
    --text: #e7ecf1;
    --muted: #8b9aab;
    --cyan: #3dd6c6;
    --green: #6bcb77;
    --yellow: #e8c547;
    --red: #f07178;
    --border: #2a3542;
  }}
  * {{ box-sizing: border-box; }}
  body {{
    margin: 0; padding: 2rem;
    font-family: "IBM Plex Sans", "Segoe UI", sans-serif;
    background: var(--bg); color: var(--text);
    line-height: 1.45;
  }}
  .brand {{
    display: flex; align-items: flex-end; gap: 0.85rem;
    margin: 0 0 0.35rem;
  }}
  .brand svg {{
    display: block; height: 36px; width: auto;
    shape-rendering: crispEdges;
  }}
  .brand .kind {{
    color: var(--muted); font-size: 0.8rem; font-weight: 600;
    letter-spacing: 0.18em; text-transform: uppercase;
    padding-bottom: 0.15rem;
  }}
  .sub {{ color: var(--muted); margin-bottom: 1.5rem; font-size: 0.95rem; }}
  .grid {{ display: grid; gap: 1rem; grid-template-columns: 1fr 1fr; }}
  @media (max-width: 900px) {{ .grid {{ grid-template-columns: 1fr; }} }}
  .panel {{
    background: var(--panel); border: 1px solid var(--border);
    border-radius: 8px; padding: 1rem 1.25rem;
  }}
  .knee {{
    background: #243038; border: 1px solid var(--cyan);
    border-radius: 8px; padding: 1rem 1.25rem; margin-bottom: 1rem;
  }}
  .knee-main {{ font-size: 1.15rem; font-weight: 600; }}
  .knee-cap {{ margin-top: 0.2rem; }}
  .knee.none {{ border-color: var(--border); color: var(--muted); }}
  .knee.dead {{ background: #2c2126; border-color: var(--red); }}
  .slo {{
    background: #1e2b2b; border: 1px solid var(--green);
    border-radius: 8px; padding: 0.85rem 1.25rem; margin-bottom: 1rem;
  }}
  .slo.miss {{ background: #2c2126; border-color: var(--yellow); }}
  .diagnosis {{
    background: var(--panel); border: 1px solid var(--border);
    border-radius: 8px; padding: 1rem 1.25rem; margin-bottom: 1rem;
  }}
  .diagnosis .diag-label {{
    color: var(--cyan); font-size: 0.75rem; letter-spacing: 0.06em;
    text-transform: uppercase; margin-bottom: 0.35rem;
  }}
  .diagnosis ul {{ margin: 0.5rem 0 0; padding-left: 1.2rem; color: var(--muted); }}
  .diagnosis li {{ margin: 0.2rem 0; }}
  .muted {{ color: var(--muted); font-size: 0.9rem; }}
  table {{ width: 100%; border-collapse: collapse; font-variant-numeric: tabular-nums; }}
  th, td {{ text-align: right; padding: 0.35rem 0.5rem; border-bottom: 1px solid var(--border); }}
  th:first-child, td:first-child {{ text-align: left; }}
  th {{ color: var(--muted); font-weight: 500; font-size: 0.85rem; }}
  .gap {{ color: var(--red); }}
  svg {{ width: 100%; height: 220px; display: block; }}
  .legend {{ display: flex; gap: 1rem; font-size: 0.85rem; color: var(--muted); margin-top: 0.5rem; }}
  .legend i {{ display: inline-block; width: 10px; height: 10px; border-radius: 2px; margin-right: 0.35rem; }}
  footer {{ margin-top: 2rem; color: var(--muted); font-size: 0.8rem; }}
</style>
</head>
<body>
  <div class="brand">{mark}<span class="kind">report</span></div>
  <div class="sub">{url} · {profile} · {duration}s · {started} · {sent} arrivals</div>
  {knee_banner}
  {slo_banner}
  {diagnosis_panel}
  <div class="grid">
    <div class="panel">
      <strong>Outcomes</strong>
      <table>
        <tr><td>completed</td><td>{total}</td></tr>
        <tr><td>success</td><td>{success} ({ok_pct:.1}%)</td></tr>
        <tr><td>failure</td><td>{failure} ({fail_pct:.1}%)</td></tr>
      </table>
    </div>
    <div class="panel">
      <strong>Latency (ms), raw vs corrected</strong>
      <table>
        <tr><th>pct</th><th>raw</th><th>corrected</th></tr>
        {rows}
      </table>
    </div>
  </div>
  {steps_panel}
  <div class="panel" style="margin-top:1rem">
    <strong>Latency over time (windowed)</strong>
    <div id="lat-chart"></div>
    <div class="legend">
      <span><i style="background:#6bcb77"></i>p50</span>
      <span><i style="background:#e8c547"></i>p90</span>
      <span><i style="background:#f07178"></i>p99</span>
      <span><i style="background:#3dd6c6"></i>knee</span>
    </div>
  </div>
  <div class="panel" style="margin-top:1rem">
    <strong>Throughput vs target</strong>
    <div id="thr-chart"></div>
    <div class="legend">
      <span><i style="background:#8b9aab"></i>target</span>
      <span><i style="background:#3dd6c6"></i>throughput</span>
    </div>
  </div>
  <div class="panel" style="margin-top:1rem">
    <strong>In-flight requests (backpressure)</strong>
    <div id="inflight-chart"></div>
  </div>
  <footer>Gust. This file is the whole report; open it anywhere.</footer>
  <script type="application/json" id="run-data">{data}</script>
  <script>
  (function() {{
    const run = JSON.parse(document.getElementById('run-data').textContent);
    const windows = run.windows || [];
    const kneeT = run.knee ? run.knee.t : null;

    function lineChart(el, series, yLabel) {{
      const w = el.clientWidth || 640, h = 220;
      const pad = {{ l: 48, r: 16, t: 12, b: 28 }};
      const iw = w - pad.l - pad.r, ih = h - pad.t - pad.b;
      let xMax = Math.max(1, ...windows.map(p => p.t), run.duration_secs || 1);
      let yMax = 1;
      series.forEach(s => s.pts.forEach(p => {{ yMax = Math.max(yMax, p[1]); }}));
      yMax *= 1.12;
      const x = t => pad.l + (t / xMax) * iw;
      const y = v => pad.t + ih - (v / yMax) * ih;
      const path = pts => pts.map((p,i) => (i?'L':'M') + x(p[0]).toFixed(1) + ',' + y(p[1]).toFixed(1)).join(' ');
      let svg = '<svg viewBox="0 0 ' + w + ' ' + h + '" preserveAspectRatio="none">';
      svg += '<line x1="'+pad.l+'" y1="'+pad.t+'" x2="'+pad.l+'" y2="'+(pad.t+ih)+'" stroke="#2a3542"/>';
      svg += '<line x1="'+pad.l+'" y1="'+(pad.t+ih)+'" x2="'+(pad.l+iw)+'" y2="'+(pad.t+ih)+'" stroke="#2a3542"/>';
      svg += '<text x="8" y="'+(pad.t+10)+'" fill="#8b9aab" font-size="11">'+yLabel+'</text>';
      svg += '<text x="'+pad.l+'" y="'+(h-6)+'" fill="#8b9aab" font-size="11">0</text>';
      svg += '<text x="'+(pad.l+iw-10)+'" y="'+(h-6)+'" fill="#8b9aab" font-size="11" text-anchor="end">'+xMax.toFixed(0)+'s</text>';
      series.forEach(s => {{
        if (!s.pts.length) return;
        svg += '<path d="'+path(s.pts)+'" fill="none" stroke="'+s.color+'" stroke-width="1.75"/>';
      }});
      if (kneeT != null) {{
        const kx = x(kneeT);
        svg += '<line x1="'+kx+'" y1="'+pad.t+'" x2="'+kx+'" y2="'+(pad.t+ih)+'" stroke="#3dd6c6" stroke-dasharray="4 3" stroke-width="1.5"/>';
      }}
      svg += '</svg>';
      el.innerHTML = svg;
    }}

    const p50 = windows.map(p => [p.t, p.p50_ms]);
    const p90 = windows.map(p => [p.t, p.p90_ms]);
    const p99 = windows.map(p => [p.t, p.p99_ms]);
    lineChart(document.getElementById('lat-chart'), [
      {{ color: '#6bcb77', pts: p50 }},
      {{ color: '#e8c547', pts: p90 }},
      {{ color: '#f07178', pts: p99 }},
    ], 'ms');

    const target = windows.map(p => [p.t, p.target_rps]);
    const thr = windows.map(p => [p.t, p.throughput]);
    lineChart(document.getElementById('thr-chart'), [
      {{ color: '#8b9aab', pts: target }},
      {{ color: '#3dd6c6', pts: thr }},
    ], 'req/s');

    const inflight = windows.map(p => [p.t, p.in_flight || 0]);
    lineChart(document.getElementById('inflight-chart'), [
      {{ color: '#c792ea', pts: inflight }},
    ], 'n');
  }})();
  </script>
</body>
</html>
"##,
        url = escape(&r.url),
        profile = escape(&r.profile),
        duration = r.duration_secs,
        started = escape(&r.started_at),
        sent = r.sent,
        knee_banner = knee_banner,
        slo_banner = slo_banner,
        diagnosis_panel = diagnosis_panel,
        total = s.total,
        success = s.success,
        failure = s.failure,
        ok_pct = pct(s.success),
        fail_pct = pct(s.failure),
        rows = pct_rows(s),
        steps_panel = steps_panel(&r.steps),
        data = escape_json_script(&data),
        mark = pixel_gust_svg(),
    )
}

fn knee_banner_html(
    knee: Option<&Knee>,
    dead_target: bool,
    failure_reason: Option<&str>,
) -> String {
    if dead_target {
        let detail = match failure_reason {
            Some(reason) => escape(reason),
            None => "every request failed".into(),
        };
        return format!(
            "<div class=\"knee dead\"><div class=\"knee-main\">No capacity. \
             <strong>Every request failed.</strong></div>\
             <span class=\"muted\">{detail}</span></div>"
        );
    }
    match knee {
        Some(k) => format!(
            "<div class=\"knee\">\
             <div class=\"knee-main\">Broke at <strong>{:.0} req/s</strong> \
             <span class=\"muted\">at {:.1}s</span></div>\
             <div class=\"knee-cap\">Stay under <strong>{:.0} req/s</strong></div>\
             <span class=\"muted\">{}</span></div>",
            k.target_rps,
            k.t,
            k.recommended_rps,
            escape(&k.reason)
        ),
        None => "<div class=\"knee none\">No knee in this run.</div>".into(),
    }
}

fn slo_banner_html(slo: Option<&gust_core::SloCapacity>, dead_target: bool) -> String {
    match (slo, dead_target) {
        (Some(slo), false) if slo.sustainable_rps > 0.0 => format!(
            "<div class=\"slo\">p99 ≤ <strong>{:.0} ms</strong> holds at \
             <strong>{:.0} req/s</strong> ({:.0} served){}</div>",
            slo.slo_p99_ms,
            slo.sustainable_rps,
            slo.sustainable_throughput,
            if slo.breached {
                ""
            } else {
                " <span class=\"muted\">(top rate, still held)</span>"
            }
        ),
        (Some(slo), false) => format!(
            "<div class=\"slo miss\">p99 ≤ <strong>{:.0} ms</strong> missed at every load</div>",
            slo.slo_p99_ms
        ),
        _ => String::new(),
    }
}

/// Pixel GUST wordmark, inlined so the HTML report stays a single file.
fn pixel_gust_svg() -> String {
    const GLYPHS: [[&str; 8]; 4] = [
        [
            "01111110", "11000011", "11000000", "11000000", "11001111", "11000011", "11000011",
            "01111110",
        ],
        [
            "11000011", "11000011", "11000011", "11000011", "11000011", "11000011", "11000011",
            "01111110",
        ],
        [
            "01111110", "11000011", "11000000", "01111110", "00000011", "00000011", "11000011",
            "01111110",
        ],
        [
            "11111111", "11111111", "00011000", "00011000", "00011000", "00011000", "00011000",
            "00011000",
        ],
    ];
    const PX: i32 = 3;
    const GAP: i32 = 2;
    const FILL: &str = "#3dd6c6";
    const HI: &str = "#c8f6ef";
    const SHADOW: &str = "#1a4f4a";
    let mut rects = String::new();
    let mut ox = 0;
    for glyph in &GLYPHS {
        for (gy, row) in glyph.iter().enumerate() {
            for (gx, ch) in row.chars().enumerate() {
                if ch != '1' {
                    continue;
                }
                let x = ox + gx as i32 * PX;
                let y = gy as i32 * PX;
                rects.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{PX}\" height=\"{PX}\" fill=\"{SHADOW}\"/>",
                    x + 1,
                    y + 1
                ));
                let up = gy == 0 || glyph[gy - 1].as_bytes()[gx] != b'1';
                let left = gx == 0 || row.as_bytes()[gx - 1] != b'1';
                let fill = if up || left { HI } else { FILL };
                rects.push_str(&format!(
                    "<rect x=\"{x}\" y=\"{y}\" width=\"{PX}\" height=\"{PX}\" fill=\"{fill}\"/>"
                ));
            }
        }
        ox += 8 * PX + GAP;
    }
    let w = ox - GAP + 2;
    let h = 8 * PX + 2;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" \
         role=\"img\" aria-label=\"Gust\">{rects}</svg>"
    )
}

fn steps_panel(steps: &[StepSummary]) -> String {
    if steps.len() <= 1 {
        return String::new();
    }
    let mut rows = String::new();
    for (i, st) in steps.iter().enumerate() {
        let ss = &st.summary;
        let hot = if i == 0 { " gap" } else { "" };
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{:.1}</td><td>{:.1}</td><td class=\"{}\">{:.1}</td></tr>\n",
            escape(&st.name),
            ss.total,
            ss.raw.p50_ms,
            ss.raw.p99_ms,
            hot,
            ss.corrected.p99_ms,
        ));
    }
    format!(
        r#"<div class="panel" style="margin-top:1rem">
      <strong>By step</strong> <span class="muted">(slowest corrected p99 first)</span>
      <table>
        <tr><th>step</th><th>n</th><th>p50</th><th>p99</th><th>p99 corr</th></tr>
        {rows}
      </table>
    </div>"#
    )
}

fn pct_rows(s: &Summary) -> String {
    let mut out = String::new();
    // Corrected min is omitted — CO backfill can invent values below observed min.
    out.push_str(&format!(
        "<tr><td>min</td><td>{:.2}</td><td class=\"muted\">—</td></tr>\n",
        s.raw.min_ms
    ));
    for (label, raw, corr) in [
        ("p50", s.raw.p50_ms, s.corrected.p50_ms),
        ("p90", s.raw.p90_ms, s.corrected.p90_ms),
        ("p99", s.raw.p99_ms, s.corrected.p99_ms),
        ("p99.9", s.raw.p999_ms, s.corrected.p999_ms),
        ("max", s.raw.max_ms, s.corrected.max_ms),
    ] {
        let cls = if corr > raw * 1.5 { " gap" } else { "" };
        out.push_str(&format!(
            "<tr><td>{label}</td><td>{raw:.2}</td><td class=\"{cls}\">{corr:.2}</td></tr>\n"
        ));
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_json_script(s: &str) -> String {
    // Prevent `</script>` breakouts inside the JSON payload.
    s.replace('<', "\\u003c")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gust_core::Knee;

    fn sample_knee() -> Knee {
        Knee {
            t: 13.0,
            target_rps: 806.0,
            recommended_rps: 604.0,
            reason: "p99 49.3ms rose to 4× service floor (11.2ms)".into(),
        }
    }

    #[test]
    fn knee_banner_reads_like_an_operator() {
        let html = knee_banner_html(Some(&sample_knee()), false, None);
        assert!(html.contains("Broke at"));
        assert!(html.contains("Stay under"));
        assert!(html.contains("806"));
        assert!(html.contains("604"));
        assert!(!html.contains("recommended safe"));
        assert!(!html.contains('—'));
    }

    #[test]
    fn pixel_wordmark_is_inline_svg() {
        let svg = pixel_gust_svg();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("aria-label=\"Gust\""));
        assert!(svg.contains("#3dd6c6"));
    }
}
