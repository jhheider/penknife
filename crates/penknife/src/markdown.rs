//! Markdown to HTML rendering, shared by the `p` clipboard action (the TUI)
//! and the `render` CLI command.

use pulldown_cmark::{Options, Parser, html};

/// Render markdown to an HTML fragment (no `<html>`/`<head>` wrapper), with
/// the common extensions enabled (tables, strikethrough, task lists,
/// footnotes) so a rich paste keeps the structure a writer expects.
pub fn render_html(markdown: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    let parser = Parser::new_ext(markdown, options);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

/// Wrap a rendered fragment in a minimal standalone HTML document, with a
/// UTF-8 charset (so non-ASCII survives opening the file directly) and a
/// title taken from the first H1, falling back to `fallback_title`.
pub fn standalone(fragment: &str, markdown: &str, fallback_title: &str) -> String {
    let title = first_heading(markdown).unwrap_or(fallback_title);
    let title = html_escape(title);
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>{title}</title>\n</head>\n<body>\n{fragment}</body>\n</html>\n"
    )
}

/// The text of the first ATX H1 (`# ...`) in the source, if any.
fn first_heading(markdown: &str) -> Option<&str> {
    markdown.lines().find_map(|line| {
        let rest = line.strip_prefix("# ")?;
        let t = rest.trim();
        (!t.is_empty()).then_some(t)
    })
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_headings_and_emphasis() {
        let html = render_html("# Title\n\nSome **bold** and *italic*.");
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
    }

    #[test]
    fn renders_lists_and_links() {
        let html = render_html("- a\n- b\n\n[x](https://e.com)");
        assert!(html.contains("<ul>"));
        assert!(html.contains("<li>a</li>"));
        assert!(html.contains("href=\"https://e.com\""));
    }

    #[test]
    fn renders_tables_via_extension() {
        let html = render_html("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(html.contains("<table>"));
    }

    #[test]
    fn standalone_wraps_with_charset_and_title_from_h1() {
        let md = "# My Note\n\ntext";
        let doc = standalone(&render_html(md), md, "penknife");
        assert!(doc.starts_with("<!doctype html>"));
        assert!(doc.contains("<meta charset=\"utf-8\">"));
        assert!(doc.contains("<title>My Note</title>"));
        assert!(doc.contains("<body>"));
    }

    #[test]
    fn standalone_falls_back_when_no_h1() {
        let md = "just a paragraph";
        let doc = standalone(&render_html(md), md, "penknife");
        assert!(doc.contains("<title>penknife</title>"));
    }
}
