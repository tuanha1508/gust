//! Self-contained HTML report for a finished Gust run.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use gust_core::{Knee, StepSummary, Summary, WindowMetric};
use serde::Serialize;

#[derive(Serialize)]
pub struct RunReport {
    pub url: String,
    pub profile: String,
    pub duration_secs: u64,
    pub sent: u64,
    pub started_at: String,
    pub summary: Summary,
    pub steps: Vec<StepSummary>,
    pub windows: Vec<WindowMetric>,
    pub knee: Option<Knee>,
}

pub fn write_html(path: &Path, report: &RunReport) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create report dir {}", parent.display()))?;
    }
    let html = render(report);
    fs::write(path, html).with_context(|| format!("write report {}", path.display()))?;
    Ok(())
}

fn render(r: &RunReport) -> String {
    let data = serde_json::to_string(r).unwrap_or_else(|_| "{}".into());
    let knee_banner = match &r.knee {
        Some(k) => format!(
            "<div class=\"knee\">Knee ≈ <strong>{:.0} req/s</strong> at t={:.1}s \
             — recommended safe ≈ <strong>{:.0} req/s</strong><br/><span class=\"muted\">{}</span></div>",
            k.target_rps,
            k.t,
            k.recommended_rps,
            escape(&k.reason)
        ),
        None => "<div class=\"knee none\">No clear knee detected in this run.</div>".into(),
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
<title>Gust report — {url}</title>
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
  h1 {{ font-size: 1.6rem; font-weight: 600; margin: 0 0 0.25rem; }}
  h1 span {{ color: var(--cyan); }}
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
  .knee.none {{ border-color: var(--border); color: var(--muted); }}
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
  <h1><span>gust</span> report</h1>
  <div class="sub">{url} · {profile} · {duration}s · {started} · {sent} arrivals</div>
  {knee_banner}
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
      <strong>Latency (ms) — raw vs corrected</strong>
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
  <footer>Generated by Gust — find where your system falls apart. Offline, self-contained report.</footer>
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
        total = s.total,
        success = s.success,
        failure = s.failure,
        ok_pct = pct(s.success),
        fail_pct = pct(s.failure),
        rows = pct_rows(s),
        steps_panel = steps_panel(&r.steps),
        data = escape_json_script(&data),
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
      <strong>By step</strong> <span class="muted">(slowest corrected p99 first — the endpoint holding the pool)</span>
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
