//! JSON syntax highlighting via `syntect`.
//!
//! Caches the default `SyntaxSet` and `ThemeSet` in a `std::sync::OnceLock`
//! so the parser/theme tables load exactly once per process. The v1 call
//! path converts highlighted spans into ratatui [`Span`]s on demand.
//!
//! When `NO_COLOR` is set (or the caller passes `plain = true`), the
//! highlighter returns lines with a single unstyled span — keeping
//! snapshot tests deterministic and respecting user preference (PRD
//! NFR-color).

use std::sync::OnceLock;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

struct Cache {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

static CACHE: OnceLock<Cache> = OnceLock::new();

fn cache() -> &'static Cache {
    CACHE.get_or_init(|| Cache {
        syntax_set: SyntaxSet::load_defaults_newlines(),
        theme_set: ThemeSet::load_defaults(),
    })
}

/// Pick the JSON syntax from the default set, falling back to plain text.
fn json_syntax(set: &SyntaxSet) -> &SyntaxReference {
    set.find_syntax_by_extension("json")
        .unwrap_or_else(|| set.find_syntax_plain_text())
}

/// Pick a dark theme; any of the bundled ones work as a fallback.
/// Returns `None` when syntect's default bundle is unexpectedly empty —
/// the caller falls back to unstyled rendering in that case.
fn pick_theme(themes: &ThemeSet) -> Option<&Theme> {
    // base16-ocean.dark is the conventional ratatui companion; fall back
    // to whatever the ThemeSet returns if the exact name isn't present
    // (syntect bundles may vary across versions).
    themes
        .themes
        .get("base16-ocean.dark")
        .or_else(|| themes.themes.values().next())
}

/// Convert a syntect RGB color into a ratatui [`Color::Rgb`]. The `a`
/// (alpha) channel is discarded — terminals do not render it.
fn to_rgb(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// Render `source` (pretty-printed JSON) into highlighted ratatui lines.
///
/// When `plain` is true, returns one unstyled `Span` per logical line.
/// Otherwise parses with the JSON grammar and maps each syntect style run
/// to a ratatui `Span` with its foreground colour applied.
#[must_use]
pub fn highlight_json_lines(source: &str, plain: bool) -> Vec<Line<'static>> {
    if plain {
        return plain_lines(source);
    }
    let cache = cache();
    let syntax = json_syntax(&cache.syntax_set);
    let Some(theme) = pick_theme(&cache.theme_set) else {
        // syntect bundle is empty — degrade gracefully.
        return plain_lines(source);
    };
    let mut highlighter = HighlightLines::new(syntax, theme);

    let mut out: Vec<Line<'static>> = Vec::new();
    for line in LinesWithEndings::from(source) {
        let ranges: Vec<(SyntectStyle, &str)> =
            match highlighter.highlight_line(line, &cache.syntax_set) {
                Ok(r) => r,
                Err(_) => {
                    // Degradation: fall back to unstyled on a parse error.
                    out.push(Line::from(Span::raw(
                        strip_trailing_newline(line).to_string(),
                    )));
                    continue;
                }
            };
        let spans: Vec<Span<'static>> = ranges
            .into_iter()
            .map(|(style, text)| {
                let text = strip_trailing_newline(text).to_string();
                let rat_style = Style::new().fg(to_rgb(style.foreground));
                Span::styled(text, rat_style)
            })
            .filter(|s| !s.content.is_empty())
            .collect();
        out.push(Line::from(spans));
    }
    out
}

/// Split `source` into lines with no styling. Terminates each line at a
/// trailing `\n`/`\r\n` so the resulting spans match the visual line count.
fn plain_lines(source: &str) -> Vec<Line<'static>> {
    source
        .split('\n')
        .map(|l| Line::from(Span::raw(l.trim_end_matches('\r').to_string())))
        .collect()
}

fn strip_trailing_newline(s: &str) -> &str {
    let s = s.strip_suffix('\n').unwrap_or(s);
    s.strip_suffix('\r').unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_mode_returns_one_span_per_line() {
        let input = "{\n  \"a\": 1\n}\n";
        let lines = highlight_json_lines(input, true);
        // 4 lines because of the trailing newline creating an empty line.
        assert_eq!(lines.len(), 4);
        // First span of first line has no explicit fg colour.
        let first = lines[0].spans.first().expect("at least one span");
        assert!(first.style.fg.is_none());
    }

    #[test]
    fn plain_mode_preserves_content() {
        let input = "{\"x\":42}";
        let lines = highlight_json_lines(input, true);
        assert_eq!(lines.len(), 1);
        let joined: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "{\"x\":42}");
    }

    #[test]
    fn highlight_mode_emits_styled_spans_for_json() {
        let input = "{\"k\": \"v\"}";
        let lines = highlight_json_lines(input, false);
        assert_eq!(lines.len(), 1);
        // At least one styled span must carry an fg colour.
        let styled_count = lines[0]
            .spans
            .iter()
            .filter(|s| s.style.fg.is_some())
            .count();
        assert!(
            styled_count >= 1,
            "syntax mode should set fg on at least one span"
        );
    }

    #[test]
    fn highlight_content_round_trips() {
        let input = "{\n  \"hello\": \"world\"\n}";
        let lines = highlight_json_lines(input, false);
        let joined: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(joined, input);
    }
}
