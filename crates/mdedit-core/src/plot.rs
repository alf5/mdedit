//! Function plots (```` ```function-plot ````), from the JSON function-plot
//! takes, as ZenNotes writes them:
//!
//! ```json
//! { "title": "Waves", "grid": true,
//!   "xAxis": { "domain": [-6.3, 6.3], "label": "x" }, "yAxis": { "domain": [-2, 2] },
//!   "data": [ { "fn": "sin(x)" }, { "fn": "x^2", "color": "#e5534b" },
//!             { "fnType": "points", "points": [[1, 1], [2, 4]] },
//!             { "fnType": "parametric", "x": "cos(t)", "y": "sin(t)", "range": [0, 6.3] } ] }
//! ```
//!
//! Expressions are parsed by exmex; `pi` and `e` are π and Euler's number,
//! and `log` is the natural logarithm, as in function-plot.

use exmex::prelude::*;
use serde::Deserialize;

use crate::mermaid::{Diagram, DrawOptions, outline_text};

#[derive(Deserialize, Default)]
#[serde(default)]
struct Config {
    title: Option<String>,
    grid: bool,
    #[serde(rename = "xAxis")]
    x_axis: Axis,
    #[serde(rename = "yAxis")]
    y_axis: Axis,
    data: Vec<Series>,
    width: Option<f64>,
    height: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Axis {
    domain: Option<[f64; 2]>,
    label: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Series {
    #[serde(rename = "fn")]
    f: Option<String>,
    #[serde(rename = "fnType")]
    kind: Option<String>,
    points: Vec<[f64; 2]>,
    x: Option<String>,
    y: Option<String>,
    range: Option<[f64; 2]>,
    color: Option<String>,
    #[serde(rename = "graphType")]
    graph: Option<String>,
}

/// The colours a plot is drawn in, as `#rrggbb`.
#[derive(Clone, Debug)]
pub struct PlotColors {
    pub text: String,
    pub muted: String,
    pub grid: String,
    /// One per series, in turn.
    pub series: Vec<String>,
}

/// A series as drawn: its runs of points, whether as dots, and its colour.
type Drawn = (Vec<Vec<[f64; 2]>>, bool, String);

/// An expression of one variable, ready to evaluate.
struct Expr {
    ex: FlatEx<f64>,
    /// Whether it uses its variable at all (`2` is a flat line).
    uses: bool,
}

impl Expr {
    fn parse(src: &str, var: &str) -> Result<Expr, String> {
        // function-plot's (math.js's) names, in exmex's spelling.
        let mut s = String::new();
        let mut word = String::new();
        let flush = |word: &mut String, s: &mut String| {
            s.push_str(match word.as_str() {
                "pi" => "PI",
                "e" => "E",
                w => w,
            });
            word.clear();
        };
        for c in src.chars() {
            if c.is_alphanumeric() || c == '_' {
                word.push(c);
            } else {
                flush(&mut word, &mut s);
                s.push(c);
            }
        }
        flush(&mut word, &mut s);
        let ex = exmex::parse::<f64>(&s).map_err(|e| format!("{src}: {e}"))?;
        let names = ex.var_names();
        if let Some(other) = names.iter().find(|n| *n != var) {
            return Err(format!("{src}: only {var} can vary, not {other}"));
        }
        let uses = !names.is_empty();
        Ok(Expr { ex, uses })
    }

    fn at(&self, v: f64) -> f64 {
        let r = if self.uses {
            self.ex.eval(&[v])
        } else {
            self.ex.eval(&[])
        };
        r.unwrap_or(f64::NAN)
    }
}

/// Steps between ticks: 1, 2 or 5 times a power of ten, about `n` to a span.
fn nice_step(span: f64, n: f64) -> f64 {
    let raw = span / n;
    let mag = 10f64.powf(raw.log10().floor());
    let r = raw / mag;
    mag * if r < 1.5 {
        1.
    } else if r < 3.5 {
        2.
    } else if r < 7.5 {
        5.
    } else {
        10.
    }
}

fn ticks(lo: f64, hi: f64, n: f64) -> Vec<f64> {
    let step = nice_step(hi - lo, n);
    let mut v = (lo / step).ceil() * step;
    let mut out = Vec::new();
    while v <= hi + step * 1e-9 {
        out.push(if v.abs() < step * 1e-9 { 0. } else { v });
        v += step;
    }
    out
}

fn label(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn esc(s: &str) -> String {
    crate::mermaid::escape_text(s).replace('"', "&quot;")
}

/// The SVG for a plot, with live text.
fn svg(src: &str, colors: &PlotColors) -> Result<(String, f64, f64), String> {
    let cfg: Config = serde_json::from_str(src).map_err(|e| format!("The plot's JSON: {e}"))?;
    if cfg.data.is_empty() {
        return Err("The plot has no data: add \"data\": [{ \"fn\": \"x^2\" }].".into());
    }
    let (w, h) = (
        cfg.width.unwrap_or(560.).clamp(160., 1600.),
        cfg.height.unwrap_or(340.).clamp(120., 1200.),
    );
    let [x0, x1] = cfg.x_axis.domain.unwrap_or([-5., 5.]);
    if x1.partial_cmp(&x0) != Some(std::cmp::Ordering::Greater) {
        return Err("xAxis.domain has to run from low to high.".into());
    }
    // Each series as runs of points (a run breaks at gaps and asymptotes).
    const SAMPLES: usize = 600;
    let mut series: Vec<Drawn> = Vec::new();
    for (i, s) in cfg.data.iter().enumerate() {
        let color = s
            .color
            .clone()
            .unwrap_or_else(|| colors.series[i % colors.series.len().max(1)].clone());
        let kind = s.kind.as_deref().unwrap_or("linear");
        let scatter = s.graph.as_deref() == Some("scatter") || kind == "points";
        let runs = match kind {
            "points" | "polyline" => vec![s.points.clone()],
            "parametric" => {
                let (fx, fy) = match (&s.x, &s.y) {
                    (Some(x), Some(y)) => (Expr::parse(x, "t")?, Expr::parse(y, "t")?),
                    _ => {
                        return Err(format!(
                            "Series {}: a parametric plot needs \"x\" and \"y\".",
                            i + 1
                        ));
                    }
                };
                let [t0, t1] = s.range.unwrap_or([0., std::f64::consts::TAU]);
                let pts = (0..=SAMPLES)
                    .map(|k| {
                        let t = t0 + (t1 - t0) * k as f64 / SAMPLES as f64;
                        [fx.at(t), fy.at(t)]
                    })
                    .collect();
                vec![pts]
            }
            _ => {
                let Some(f) = &s.f else {
                    return Err(format!("Series {} has no \"fn\".", i + 1));
                };
                let f = Expr::parse(f, "x").map_err(|e| format!("Series {}: {e}", i + 1))?;
                let [a, b] = s.range.unwrap_or([x0, x1]);
                vec![
                    (0..=SAMPLES)
                        .map(|k| {
                            let x = a + (b - a) * k as f64 / SAMPLES as f64;
                            [x, f.at(x)]
                        })
                        .collect(),
                ]
            }
        };
        series.push((runs, scatter, color));
    }
    // The y range: as given, or what the curves reach (outliers aside).
    let [y0, y1] = match cfg.y_axis.domain {
        Some(d) => d,
        None => {
            let mut ys: Vec<f64> = series
                .iter()
                .flat_map(|(runs, ..)| runs.iter().flatten())
                .filter(|p| p[0] >= x0 && p[0] <= x1)
                .map(|p| p[1])
                .filter(|y| y.is_finite())
                .collect();
            ys.sort_by(f64::total_cmp);
            if ys.is_empty() {
                [-5., 5.]
            } else {
                let lo = ys[ys.len() * 2 / 100];
                let hi = ys[(ys.len() * 98 / 100).min(ys.len() - 1)];
                let pad = ((hi - lo) * 0.1).max(0.5);
                [lo - pad, hi + pad]
            }
        }
    };
    if y1.partial_cmp(&y0) != Some(std::cmp::Ordering::Greater) {
        return Err("yAxis.domain has to run from low to high.".into());
    }

    let top = if cfg.title.is_some() { 30. } else { 12. };
    let (left, right, bottom) = (
        44.,
        12.,
        26. + if cfg.x_axis.label.is_some() { 14. } else { 0. },
    );
    let (pw, ph) = (w - left - right, h - top - bottom);
    let sx = |x: f64| left + (x - x0) / (x1 - x0) * pw;
    let sy = |y: f64| top + (y1 - y) / (y1 - y0) * ph;
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\" font-size=\"11\">"
    );
    if let Some(t) = &cfg.title {
        out.push_str(&format!(
            "<text x=\"{}\" y=\"18\" text-anchor=\"middle\" font-size=\"13\" fill=\"{}\">{}</text>",
            left + pw / 2.,
            colors.text,
            esc(t)
        ));
    }
    let (xt, yt) = (ticks(x0, x1, pw / 70.), ticks(y0, y1, ph / 45.));
    if cfg.grid {
        for x in &xt {
            out.push_str(&format!(
                "<line x1=\"{0}\" y1=\"{top}\" x2=\"{0}\" y2=\"{1}\" stroke=\"{2}\" stroke-width=\"0.6\"/>",
                sx(*x),
                top + ph,
                colors.grid
            ));
        }
        for y in &yt {
            out.push_str(&format!(
                "<line x1=\"{left}\" y1=\"{0}\" x2=\"{1}\" y2=\"{0}\" stroke=\"{2}\" stroke-width=\"0.6\"/>",
                sy(*y),
                left + pw,
                colors.grid
            ));
        }
    }
    // Axes: through 0 when it's in view, else along the edge.
    let ax = if y0 <= 0. && 0. <= y1 {
        sy(0.)
    } else {
        top + ph
    };
    let ay = if x0 <= 0. && 0. <= x1 { sx(0.) } else { left };
    out.push_str(&format!(
        "<line x1=\"{left}\" y1=\"{ax}\" x2=\"{}\" y2=\"{ax}\" stroke=\"{1}\"/>\
         <line x1=\"{ay}\" y1=\"{top}\" x2=\"{ay}\" y2=\"{2}\" stroke=\"{1}\"/>",
        left + pw,
        colors.muted,
        top + ph
    ));
    for x in &xt {
        out.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" fill=\"{}\">{}</text>",
            sx(*x),
            top + ph + 15.,
            colors.muted,
            label(*x)
        ));
    }
    for y in &yt {
        out.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" text-anchor=\"end\" fill=\"{}\">{}</text>",
            left - 6.,
            sy(*y) + 4.,
            colors.muted,
            label(*y)
        ));
    }
    if let Some(l) = &cfg.x_axis.label {
        out.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" text-anchor=\"end\" fill=\"{}\">{}</text>",
            left + pw,
            h - 6.,
            colors.muted,
            esc(l)
        ));
    }
    if let Some(l) = &cfg.y_axis.label {
        out.push_str(&format!(
            "<text x=\"{}\" y=\"{}\" fill=\"{}\">{}</text>",
            left + 4.,
            top + 10.,
            colors.muted,
            esc(l)
        ));
    }
    // The curves, clipped to the plot; a jump of more than the view's
    // height between samples is an asymptote, not a line.
    out.push_str(&format!(
        "<defs><clipPath id=\"plot\"><rect x=\"{left}\" y=\"{top}\" width=\"{pw}\" height=\"{ph}\"/></clipPath></defs><g clip-path=\"url(#plot)\">"
    ));
    for (runs, scatter, color) in &series {
        for run in runs {
            if *scatter {
                for p in run.iter().filter(|p| p[0].is_finite() && p[1].is_finite()) {
                    out.push_str(&format!(
                        "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"3\" fill=\"{color}\"/>",
                        sx(p[0]),
                        sy(p[1])
                    ));
                }
                continue;
            }
            let mut d = String::new();
            let mut prev: Option<[f64; 2]> = None;
            for p in run {
                let ok = p[0].is_finite() && p[1].is_finite();
                let jump = prev.is_some_and(|q| (p[1] - q[1]).abs() > (y1 - y0) * 1.5);
                if ok {
                    // Keep far-off points near the view, so clipping works.
                    let y = p[1].clamp(y0 - (y1 - y0) * 2., y1 + (y1 - y0) * 2.);
                    let cmd = if prev.is_none() || jump { 'M' } else { 'L' };
                    d.push_str(&format!("{cmd}{:.2} {:.2} ", sx(p[0]), sy(y)));
                    prev = Some(*p);
                } else {
                    prev = None;
                }
            }
            if !d.is_empty() {
                out.push_str(&format!(
                    "<path d=\"{d}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2\" stroke-linejoin=\"round\"/>"
                ));
            }
        }
    }
    out.push_str("</g></svg>");
    Ok((out, w, h))
}

/// Draw a plot for the app: text outlined, as diagrams' is.
pub fn draw(src: &str, colors: &PlotColors, opts: &DrawOptions) -> Result<Diagram, String> {
    let (s, w, h) = svg(src, colors)?;
    Ok(Diagram {
        svg: outline_text(&s, &opts.font_family, &opts.fontdb)?,
        width: w,
        height: h,
    })
}

/// An exported page's plot, in light colours with live text.
pub fn render_block(src: &str) -> String {
    let colors = PlotColors {
        text: "#1f2328".into(),
        muted: "#59636e".into(),
        grid: "#d1d9e0".into(),
        series: ["#0969da", "#cf222e", "#1a7f37", "#8250df", "#bc4c00"]
            .map(String::from)
            .to_vec(),
    };
    let attr = crate::mermaid::escape_attr(src);
    match svg(src, &colors) {
        Ok((s, ..)) => {
            format!("<div class=\"function-plot\" data-function-plot=\"{attr}\">\n{s}\n</div>")
        }
        Err(e) => format!(
            "<div class=\"function-plot mermaid-error\" data-function-plot=\"{attr}\">\n<p class=\"mermaid-error-msg\">{}</p>\
             <pre><code class=\"language-function-plot\">{}</code></pre>\n</div>",
            crate::mermaid::escape_text(&e),
            crate::mermaid::escape_text(src)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> PlotColors {
        PlotColors {
            text: "#000".into(),
            muted: "#555".into(),
            grid: "#ddd".into(),
            series: vec!["#00f".into(), "#f00".into()],
        }
    }

    #[test]
    fn expressions_read_like_function_plot() {
        let f = Expr::parse("2*pi + e - log(e)", "x").unwrap();
        assert!((f.at(0.) - (2. * std::f64::consts::PI + std::f64::consts::E - 1.)).abs() < 1e-9);
        let g = Expr::parse("x^2 + sin(x)", "x").unwrap();
        assert!((g.at(2.) - (4. + 2f64.sin())).abs() < 1e-9);
        assert!(Expr::parse("x + y", "x").err().unwrap().contains("only x"));
    }

    #[test]
    fn ticks_are_round_numbers() {
        assert_eq!(ticks(-5., 5., 5.), vec![-4., -2., 0., 2., 4.]);
        assert_eq!(label(0.30000000000000004), "0.3");
    }

    #[test]
    fn plots_draw_and_break_at_asymptotes() {
        let (s, ..) = svg(
            r#"{"data":[{"fn":"1/x"}],"yAxis":{"domain":[-5,5]}}"#,
            &colors(),
        )
        .unwrap();
        let path = &s[s.find("<path d=\"").unwrap()..];
        assert!(
            path[9..].matches('M').count() >= 2,
            "a break at x = 0: {path}"
        );
        let (s, ..) = svg(
            r#"{"title":"T","grid":true,"data":[{"fn":"x"},{"fnType":"points","points":[[1,1]]},{"fnType":"parametric","x":"cos(t)","y":"sin(t)"}]}"#,
            &colors(),
        )
        .unwrap();
        assert!(
            s.contains(">T</text>") && s.contains("<circle") && s.matches("<path").count() == 2,
            "{s}"
        );
        assert!(svg("{}", &colors()).unwrap_err().contains("no data"));
        assert!(
            svg(r#"{"data":[{"fn":"x +"}]}"#, &colors())
                .unwrap_err()
                .contains("Series 1")
        );
        assert!(svg("not json", &colors()).unwrap_err().contains("JSON"));
    }
}
