// SPDX-License-Identifier: AGPL-3.0-or-later
//! Ported Sotto inline-SVG diagram vocabulary: the house-style page CSS, the SVG
//! primitive builders (node, zone, edge), a `Diagram` composer, and a cheap
//! well-formedness check. Pure string building, no external dependencies.

/// The Sotto design-page CSS, ported verbatim from `docs/index.html`, embedded inline
/// by `page::render_page`.
pub const PAGE_CSS: &str = r#":root{
  --ink:#0b0d10; --ink2:#11151b; --ink3:#161c25; --ink4:#1d2531;
  --line:rgba(255,255,255,.09); --line2:rgba(255,255,255,.16);
  --text:#e9e5d9; --muted:#97a0ab; --faint:#6c7682;
  --amber:#ffb454; --amber2:#ffd9a0; --amberdim:rgba(255,180,84,.14);
  --teal:#5fd3c4; --tealdim:rgba(95,211,196,.13);
  --coral:#ff8c6b; --coraldim:rgba(255,140,107,.13);
  --violet:#b69cff; --violetdim:rgba(182,156,255,.13);
  --maxw:1080px;
}
*{box-sizing:border-box}
body{margin:0; background:var(--ink); color:var(--text); font-family:"Hanken Grotesk",sans-serif; font-size:17px; line-height:1.65; -webkit-font-smoothing:antialiased}
.wrap{max-width:var(--maxw); margin:0 auto; padding:0 22px}
h1,h2,h3{font-family:"Fraunces",serif; font-weight:900; letter-spacing:-.01em; line-height:1.15}
h1{font-size:40px; margin:32px 0 8px}
h2{font-size:27px; margin:40px 0 6px}
h3{font-size:20px; margin:26px 0 4px; font-weight:600}
.movement-num{font-family:"JetBrains Mono",monospace; font-size:12px; letter-spacing:.2em; text-transform:uppercase; color:var(--amber)}
a{color:var(--amber); text-decoration:none; border-bottom:1px solid rgba(255,180,84,.32)}
code,.mono{font-family:"JetBrains Mono",monospace}
code{background:var(--ink3); border:1px solid var(--line); padding:.08em .42em; border-radius:5px; font-size:.84em; color:var(--amber2)}
.diagram{background:radial-gradient(120% 120% at 50% 0%,rgba(255,255,255,.02),transparent 60%),var(--ink2); border:1px solid var(--line); border-radius:16px; padding:26px 22px 16px; margin:22px 0 8px; overflow-x:auto}
.diagram .cap{font-family:"JetBrains Mono",monospace; font-size:11.5px; letter-spacing:.18em; text-transform:uppercase; color:var(--faint); margin:6px 0 18px; display:flex; align-items:center; gap:10px}
.diagram .cap .fig{color:var(--amber)}
.svg-zone{fill:none; stroke-width:1.4}
.svg-node{rx:10}
.nodetext{font-family:"JetBrains Mono",monospace; font-size:13px; font-weight:500}
.nodesub{font-family:"JetBrains Mono",monospace; font-size:10.5px}
.zonelabel{font-family:"JetBrains Mono",monospace; font-size:11px; letter-spacing:.14em; text-transform:uppercase}
.flowlbl{font-family:"JetBrains Mono",monospace; font-size:11px}
"#;

/// The house-style semantic accent palette a diagram primitive is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accent {
    Amber,
    Teal,
    Coral,
    Violet,
    Muted,
}

impl Accent {
    /// The stroke/line hex (with leading `#`).
    pub fn stroke(self) -> &'static str {
        match self {
            Accent::Amber => "#ffb454",
            Accent::Teal => "#5fd3c4",
            Accent::Coral => "#ff8c6b",
            Accent::Violet => "#b69cff",
            Accent::Muted => "#97a0ab",
        }
    }

    /// The translucent node fill (an `rgba(...)` string).
    pub fn fill(self) -> &'static str {
        match self {
            Accent::Amber => "rgba(255,180,84,.10)",
            Accent::Teal => "rgba(95,211,196,.10)",
            Accent::Coral => "rgba(255,140,107,.10)",
            Accent::Violet => "rgba(182,156,255,.10)",
            Accent::Muted => "rgba(151,160,171,.10)",
        }
    }

    /// The node title text color.
    pub fn text(self) -> &'static str {
        match self {
            Accent::Amber => "#ffd9a0",
            Accent::Teal => "#5fd3c4",
            Accent::Coral => "#ffd9a0",
            Accent::Violet => "#d8caff",
            Accent::Muted => "#e9e5d9",
        }
    }
}

/// Escape `&`, `<`, `>`, `"`, and `'` for safe interpolation into HTML/XML text or
/// attribute content. `&` is escaped first so none of the other substitutions get
/// double-escaped.
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// A rounded node box with a title (`nodetext`) and optional subtitle (`nodesub`).
pub fn node(
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    title: &str,
    subtitle: Option<&str>,
    accent: Accent,
) -> String {
    let title = xml_escape(title);
    let cx = x + w / 2;
    let title_y = if subtitle.is_some() {
        (y + h / 2).saturating_sub(6)
    } else {
        y + h / 2 + 5
    };
    let mut out = format!(
        "<rect class=\"svg-node\" x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" \
         fill=\"{fill}\" stroke=\"{stroke}\" stroke-width=\"1.4\" rx=\"10\"/>\
         <text class=\"nodetext\" x=\"{cx}\" y=\"{title_y}\" text-anchor=\"middle\" fill=\"{text}\">{title}</text>",
        fill = accent.fill(),
        stroke = accent.stroke(),
        text = accent.text(),
    );
    if let Some(sub) = subtitle {
        let sub = xml_escape(sub);
        let sub_y = y + h / 2 + 12;
        out.push_str(&format!(
            "<text class=\"nodesub\" x=\"{cx}\" y=\"{sub_y}\" text-anchor=\"middle\" fill=\"var(--muted)\">{sub}</text>"
        ));
    }
    out
}

/// A labelled zone/boundary rectangle (`svg-zone`) with an uppercase `zonelabel`.
pub fn zone(x: u32, y: u32, w: u32, h: u32, label: &str, accent: Accent) -> String {
    let label = xml_escape(label);
    format!(
        "<rect class=\"svg-zone\" x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" \
         stroke=\"{stroke}\" stroke-width=\"1.4\" fill=\"none\" rx=\"14\" stroke-dasharray=\"4 4\"/>\
         <text class=\"zonelabel\" x=\"{lx}\" y=\"{ly}\" fill=\"{stroke}\">{label}</text>",
        stroke = accent.stroke(),
        lx = x + 14,
        ly = y + 20,
    )
}

/// A directed edge (a line with an arrow marker) with an optional flow label near its
/// start point. The label offset assumes a left-to-right edge (`x2 >= x1`); for a
/// right-to-left edge it collapses to `x1` with no offset rather than going negative.
pub fn edge(x1: u32, y1: u32, x2: u32, y2: u32, label: Option<&str>, accent: Accent) -> String {
    let marker = match accent {
        Accent::Amber => "arrow-amber",
        Accent::Teal => "arrow-teal",
        Accent::Coral => "arrow-coral",
        Accent::Violet => "arrow-violet",
        Accent::Muted => "arrow-muted",
    };
    let mut out = format!(
        "<line x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\" stroke=\"{stroke}\" \
         stroke-width=\"1.4\" marker-end=\"url(#{marker})\"/>",
        stroke = accent.stroke(),
    );
    if let Some(label) = label {
        let label = xml_escape(label);
        let lx = x1 + (x2.saturating_sub(x1)) / 6;
        let ly = if y1 > 12 { y1 - 8 } else { y1 + 16 };
        out.push_str(&format!(
            "<text class=\"flowlbl\" x=\"{lx}\" y=\"{ly}\" fill=\"var(--muted)\">{label}</text>"
        ));
    }
    out
}

fn arrow_marker_defs() -> String {
    let accents = [
        Accent::Amber,
        Accent::Teal,
        Accent::Coral,
        Accent::Violet,
        Accent::Muted,
    ];
    let mut defs = String::from("<defs>");
    for accent in accents {
        let id = match accent {
            Accent::Amber => "arrow-amber",
            Accent::Teal => "arrow-teal",
            Accent::Coral => "arrow-coral",
            Accent::Violet => "arrow-violet",
            Accent::Muted => "arrow-muted",
        };
        defs.push_str(&format!(
            "<marker id=\"{id}\" viewBox=\"0 0 10 10\" refX=\"8\" refY=\"5\" \
             markerWidth=\"7\" markerHeight=\"7\" orient=\"auto-start-reverse\">\
             <path d=\"M 0 0 L 10 5 L 0 10 z\" fill=\"{stroke}\"/></marker>",
            stroke = accent.stroke(),
        ));
    }
    defs.push_str("</defs>");
    defs
}

/// Composes primitives into a complete `<div class="diagram">` block: the `FIG`
/// caption, an `<svg>` with the arrow-marker `<defs>`, and every pushed part.
pub struct Diagram {
    pub caption: String,
    pub view_w: u32,
    pub view_h: u32,
    pub parts: Vec<String>,
}

impl Diagram {
    pub fn new(caption: &str, view_w: u32, view_h: u32) -> Self {
        Self {
            caption: caption.to_string(),
            view_w,
            view_h,
            parts: Vec::new(),
        }
    }

    pub fn push(&mut self, part: String) -> &mut Self {
        self.parts.push(part);
        self
    }

    /// Render the full `<div class="diagram">...<svg>...</svg></div>` block, including
    /// the `<defs>` with amber/teal/coral/violet/muted arrow markers and the caption.
    pub fn render(&self) -> String {
        let caption = xml_escape(&self.caption);
        let mut svg = format!(
            "<svg viewBox=\"0 0 {w} {h}\" xmlns=\"http://www.w3.org/2000/svg\">",
            w = self.view_w,
            h = self.view_h,
        );
        svg.push_str(&arrow_marker_defs());
        for part in &self.parts {
            svg.push_str(part);
        }
        svg.push_str("</svg>");
        format!(
            "<div class=\"diagram\"><div class=\"cap\"><span class=\"fig\">FIG</span> {caption}</div>{svg}</div>"
        )
    }
}

/// A cheap structural check: tags are balanced (every `<tag ...>` that is not
/// self-closing has a matching `</tag>`), and there is exactly one top-level element.
/// Not a full XML parser; enough to catch a builder that drops a closing tag. The scan
/// for a tag's closing `>` is quote-aware: a `>` inside a single- or double-quoted
/// attribute value is not treated as the tag boundary.
pub fn is_well_formed_svg(svg: &str) -> Result<(), String> {
    let bytes = svg.as_bytes();
    let mut i = 0usize;
    let mut stack: Vec<String> = Vec::new();
    let mut top_level_count = 0usize;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }

        // Find the tag-closing '>', skipping over any '>' that falls inside a
        // single- or double-quoted attribute value.
        let mut quote: Option<u8> = None;
        let mut end = None;
        let mut j = i + 1;
        while j < bytes.len() {
            let b = bytes[j];
            match quote {
                Some(q) if b == q => quote = None,
                Some(_) => {}
                None if b == b'"' || b == b'\'' => quote = Some(b),
                None if b == b'>' => {
                    end = Some(j);
                    break;
                }
                None => {}
            }
            j += 1;
        }
        let end = end.ok_or_else(|| format!("unclosed '<' at byte {i}"))?;
        let tag_content = &svg[i + 1..end];

        if let Some(name) = tag_content.strip_prefix('/') {
            // Closing tag.
            let name = name.trim().to_string();
            match stack.pop() {
                Some(open) if open == name => {}
                Some(open) => {
                    return Err(format!(
                        "mismatched close: expected </{open}>, found </{name}>"
                    ));
                }
                None => {
                    return Err(format!(
                        "unexpected closing tag </{name}> with nothing open"
                    ))
                }
            }
        } else if tag_content.starts_with('!') || tag_content.starts_with('?') {
            // Comment / doctype / processing instruction: ignore.
            // TODO: an XML comment (`<!-- ... -->`) containing its own '>' is not
            // correctly skipped here (the outer quote-aware scan does not know about
            // comment syntax). No current builder emits comments, so this is a known,
            // accepted limitation rather than something exercised in practice.
        } else if tag_content.trim_end().ends_with('/') {
            // Self-closing tag: no stack change.
        } else {
            let name = tag_content
                .split(|c: char| c.is_whitespace())
                .next()
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                return Err(format!("malformed tag at byte {i}"));
            }
            if stack.is_empty() {
                top_level_count += 1;
            }
            stack.push(name);
        }

        i = end + 1;
    }

    if !stack.is_empty() {
        return Err(format!("unclosed tag(s): {stack:?}"));
    }
    if top_level_count != 1 {
        return Err(format!(
            "expected exactly one top-level element, found {top_level_count}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_contains_title_subtitle_and_accent() {
        let out = node(
            10,
            20,
            120,
            60,
            "Orchestrator",
            Some("FastAPI"),
            Accent::Amber,
        );
        assert!(out.contains("<rect class=\"svg-node\""));
        assert!(out.contains("Orchestrator"));
        assert!(out.contains("FastAPI"));
        assert!(out.contains("#ffb454"));
    }

    #[test]
    fn node_escapes_its_text() {
        let out = node(0, 0, 100, 40, "A & B <x>", None, Accent::Teal);
        assert!(out.contains("A &amp; B &lt;x&gt;"));
        assert!(!out.contains("A & B <x>"));
    }

    #[test]
    fn edge_with_label_includes_marker_and_label() {
        let out = edge(0, 0, 100, 0, Some("dials"), Accent::Coral);
        assert!(out.contains("marker-end=\"url(#arrow-coral)\""));
        assert!(out.contains("dials"));
        assert!(out.contains("class=\"flowlbl\""));
    }

    #[test]
    fn zone_renders_a_labelled_boundary() {
        let out = zone(0, 0, 200, 100, "phone", Accent::Violet);
        assert!(out.contains("class=\"svg-zone\""));
        assert!(out.contains("class=\"zonelabel\""));
        assert!(out.contains("phone"));
        assert!(out.contains("#b69cff"));
    }

    #[test]
    fn diagram_render_wraps_svg_with_defs_and_caption() {
        let mut d = Diagram::new("Request lifecycle", 400, 200);
        d.push(node(10, 10, 100, 40, "Phone", None, Accent::Amber));
        d.push(edge(110, 30, 300, 30, None, Accent::Amber));
        let out = d.render();
        assert!(out.contains("<div class=\"diagram\">"));
        assert!(out.contains("<defs>"));
        assert!(out.contains("<marker"));
        assert!(out.contains("Request lifecycle"));
        for part in &d.parts {
            assert!(out.contains(part.as_str()));
        }
    }

    #[test]
    fn diagram_render_is_well_formed() {
        let mut d = Diagram::new("Zones", 300, 150);
        d.push(zone(0, 0, 300, 150, "phone", Accent::Teal));
        d.push(node(20, 20, 100, 40, "Client", None, Accent::Teal));
        let out = d.render();
        assert!(
            is_well_formed_svg(&out).is_ok(),
            "{:?}",
            is_well_formed_svg(&out)
        );
    }

    #[test]
    fn is_well_formed_svg_rejects_an_unclosed_tag() {
        let broken = "<div><svg><rect></svg></div>";
        assert!(is_well_formed_svg(broken).is_err());
    }

    #[test]
    fn node_with_a_subtitle_and_a_short_height_does_not_panic() {
        // h < 12 with a subtitle used to underflow the unchecked `y + h / 2 - 6`.
        let out = node(0, 0, 40, 10, "T", Some("s"), Accent::Amber);
        // node() emits sibling fragments (rect + text elements), so wrap them in a
        // single element the way callers always do (inside an <svg>) before checking
        // well-formedness, which requires exactly one top-level element.
        let wrapped = format!("<svg>{out}</svg>");
        assert!(
            is_well_formed_svg(&wrapped).is_ok(),
            "{:?}",
            is_well_formed_svg(&wrapped)
        );
    }

    #[test]
    fn is_well_formed_svg_ignores_a_gt_inside_a_quoted_attribute() {
        let svg = r#"<svg><rect data-x="a>b"/></svg>"#;
        assert!(
            is_well_formed_svg(svg).is_ok(),
            "{:?}",
            is_well_formed_svg(svg)
        );
    }

    #[test]
    fn xml_escape_escapes_apostrophes() {
        let out = xml_escape("a'b");
        assert!(out.contains("&#39;"));
        assert!(!out.contains('\''));
    }
}
