// SPDX-License-Identifier: AGPL-3.0-or-later
//! The accreting design page: a `DesignPage` model plus `render_page`, which produces
//! a single self-contained Sotto-style HTML document (inline CSS, no `<script>`, the
//! one permitted external resource being the Google Fonts link). Generating section
//! content from a facilitated session is out of scope here; this module only renders
//! whatever sections it is given.

use crate::svg::{xml_escape, PAGE_CSS};

/// One section of the design page (typically one movement's ratified artifact).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Section {
    /// An uppercase eyebrow label, e.g. "MOVEMENT 1 - FRAME".
    pub eyebrow: String,
    /// The section heading.
    pub heading: String,
    /// Pre-rendered HTML body for the section (the caller supplies HTML; the renderer
    /// inlines it as-is inside the section). Trusted content produced upstream.
    pub body_html: String,
    /// Inline-SVG diagram blocks (each a full `<div class="diagram">...` from
    /// `svg::Diagram::render`).
    pub diagrams: Vec<String>,
}

/// The whole accreting design page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DesignPage {
    pub title: String,
    /// A one-line subtitle/tagline under the title.
    pub subtitle: String,
    pub sections: Vec<Section>,
}

const FONTS_LINK: &str = "<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\
<link rel=\"preconnect\" href=\"https://fonts.gstatic.com\" crossorigin>\
<link href=\"https://fonts.googleapis.com/css2?family=Fraunces:wght@600;900&family=Hanken+Grotesk:wght@400;500;600&family=JetBrains+Mono:wght@400;500&display=swap\" rel=\"stylesheet\">";

/// Render the whole design page as a single self-contained HTML document (inline CSS,
/// no external script, the one permitted external resource being the Google Fonts
/// link). Deterministic: identical input yields identical output.
pub fn render_page(page: &DesignPage) -> String {
    let title = xml_escape(&page.title);
    let subtitle = xml_escape(&page.subtitle);

    let mut out = String::new();
    out.push_str("<!doctype html><html lang=\"en\"><head>");
    out.push_str("<meta charset=\"utf-8\">");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    out.push_str(&format!("<title>{title}</title>"));
    out.push_str(FONTS_LINK);
    out.push_str("<style>");
    out.push_str(PAGE_CSS);
    out.push_str("</style>");
    out.push_str("</head><body>");
    out.push_str("<div class=\"wrap\">");
    out.push_str(&format!("<h1>{title}</h1>"));
    out.push_str(&format!("<p>{subtitle}</p>"));

    for section in &page.sections {
        let eyebrow = xml_escape(&section.eyebrow);
        let heading = xml_escape(&section.heading);
        out.push_str(&format!("<div class=\"movement-num\">{eyebrow}</div>"));
        out.push_str(&format!("<h2>{heading}</h2>"));
        out.push_str(&section.body_html);
        for diagram in &section.diagrams {
            out.push_str(diagram);
        }
    }

    out.push_str("</div>");
    out.push_str("</body></html>");
    out
}

/// Render a movement's ratified artifact (GitHub-flavored markdown) into the section `body_html`.
/// A leading ATX heading is dropped first: the page already renders the movement heading as an
/// `<h2>`, so the artifact's own `# Title` / `## Title` would duplicate it. The output is HTML,
/// inlined verbatim by `render_page`, so it must only ever reach a trusted or sandboxed surface
/// (the app's design preview is a `sandbox=""` iframe); pulldown-cmark escapes text content but
/// passes raw HTML in the markdown through unchanged.
pub fn render_section_body(markdown: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let body = strip_leading_heading(markdown);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(body, opts);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

/// Drop a single leading ATX heading line (`#`..`######` followed by a space) and any blank
/// lines after it, so a section artifact that repeats its own title does not double the heading
/// the page renders. Leaves the text unchanged when it does not start with a heading.
fn strip_leading_heading(markdown: &str) -> &str {
    let trimmed = markdown.trim_start_matches(['\n', '\r']);
    let first_line_end = trimmed.find('\n').unwrap_or(trimmed.len());
    let first_line = trimmed[..first_line_end].trim_end();
    let hashes = first_line.chars().take_while(|&c| c == '#').count();
    let is_atx_heading = (1..=6).contains(&hashes)
        && first_line[hashes..].starts_with(' ')
        && !first_line[hashes..].trim().is_empty();
    if is_atx_heading {
        trimmed[first_line_end..].trim_start_matches(['\n', '\r'])
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svg::{node, Accent, Diagram};

    fn sample_page() -> DesignPage {
        DesignPage {
            title: "SOTTO Design".to_string(),
            subtitle: "The whole design, ratified movement by movement.".to_string(),
            sections: vec![
                Section {
                    eyebrow: "MOVEMENT 1 - FRAME".to_string(),
                    heading: "What is this".to_string(),
                    body_html: "<p>A private voice assistant.</p>".to_string(),
                    diagrams: vec![],
                },
                Section {
                    eyebrow: "MOVEMENT 2 - SHAPE".to_string(),
                    heading: "How it fits together".to_string(),
                    body_html: "<p>Two devices, one link.</p>".to_string(),
                    diagrams: vec!["<div class=\"diagram\">stub</div>".to_string()],
                },
            ],
        }
    }

    #[test]
    fn render_page_is_a_single_self_contained_document() {
        let out = render_page(&sample_page());
        assert!(out.starts_with("<!doctype html"));
        assert!(out.contains("<style>"));
        assert!(out.contains("--amber:#ffb454"));
        assert!(!out.contains("<script"));
        // No external stylesheet other than the fonts.googleapis.com link.
        let stylesheet_links: Vec<&str> = out
            .split("<link")
            .skip(1)
            .filter(|frag| frag.contains("stylesheet") || frag.contains("preconnect"))
            .collect();
        for frag in stylesheet_links {
            assert!(
                frag.contains("fonts.googleapis.com") || frag.contains("fonts.gstatic.com"),
                "unexpected external link: {frag}"
            );
        }
    }

    #[test]
    fn render_page_includes_every_section_and_diagram() {
        let out = render_page(&sample_page());
        assert!(out.contains("What is this"));
        assert!(out.contains("How it fits together"));
        assert!(out.contains("A private voice assistant."));
        assert!(out.contains("Two devices, one link."));
        assert!(out.contains("<div class=\"diagram\">stub</div>"));
    }

    #[test]
    fn render_page_escapes_the_title_but_not_the_body_html() {
        let page = DesignPage {
            title: "A & B".to_string(),
            subtitle: String::new(),
            sections: vec![Section {
                eyebrow: String::new(),
                heading: String::new(),
                body_html: "<p>hi</p>".to_string(),
                diagrams: vec![],
            }],
        };
        let out = render_page(&page);
        assert!(out.contains("<title>A &amp; B</title>"));
        assert!(out.contains("<h1>A &amp; B</h1>"));
        assert!(out.contains("<p>hi</p>"));
        assert!(!out.contains("A & B"));
    }

    #[test]
    fn render_page_is_deterministic() {
        let page = sample_page();
        assert_eq!(render_page(&page), render_page(&page));
    }

    #[test]
    fn render_section_body_renders_markdown_to_html() {
        let md = "**Principles**\n\n- One\n- Two";
        let html = render_section_body(md);
        assert!(html.contains("<strong>Principles</strong>"));
        assert!(html.contains("<ul>"));
        assert!(html.contains("<li>One</li>"));
        // Raw markdown markers must not survive into the output.
        assert!(!html.contains("**Principles**"));
        assert!(!html.contains("- One"));
    }

    #[test]
    fn render_section_body_drops_a_duplicate_leading_heading() {
        // The page renders the movement heading itself, so the artifact's own leading heading is
        // stripped to avoid showing "Constitution" twice.
        let md = "## Constitution\n\nMLB Races is a web app.";
        let html = render_section_body(md);
        assert!(!html.contains("Constitution"));
        assert!(html.contains("MLB Races is a web app."));
    }

    #[test]
    fn render_section_body_keeps_body_when_it_has_no_leading_heading() {
        let html = render_section_body("Just a paragraph.");
        assert!(html.contains("<p>Just a paragraph.</p>"));
    }

    #[test]
    fn strip_leading_heading_ignores_a_hashless_line() {
        // A `#` not followed by a space (e.g. a "#1 seed") is not an ATX heading.
        assert_eq!(strip_leading_heading("#1 seed race"), "#1 seed race");
    }

    #[test]
    fn strip_leading_heading_handles_crlf_and_bounds() {
        // CRLF after the heading and before the body is consumed.
        assert_eq!(strip_leading_heading("## Title\r\n\r\nBody"), "Body");
        // Six hashes is the max ATX level; seven is not a heading.
        assert_eq!(strip_leading_heading("###### H\nX"), "X");
        assert_eq!(
            strip_leading_heading("####### too many\nX"),
            "####### too many\nX"
        );
        // A heading with no body leaves an empty remainder rather than panicking.
        assert_eq!(strip_leading_heading("## Only a heading"), "");
    }

    #[test]
    fn render_page_with_a_real_diagram_is_plausibly_valid() {
        let mut diagram = Diagram::new("Request lifecycle", 400, 200);
        diagram.push(node(10, 10, 120, 40, "Phone", None, Accent::Amber));
        let page = DesignPage {
            title: "Design".to_string(),
            subtitle: "Subtitle".to_string(),
            sections: vec![Section {
                eyebrow: "MOVEMENT 1".to_string(),
                heading: "Heading".to_string(),
                body_html: String::new(),
                diagrams: vec![diagram.render()],
            }],
        };
        let out = render_page(&page);
        assert!(out.contains("<div class=\"diagram\">"));
        assert!(out.contains("Phone"));
    }
}
