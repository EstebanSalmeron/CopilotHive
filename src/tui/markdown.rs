//! Light markdown rendering (headings, lists, **bold**, `code`, fenced blocks) and wrapping.
use super::theme::*;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

fn inline(s: &str, base: Style) -> Vec<Span<'static>> {
    let (mut out, mut buf) = (vec![], String::new());
    let (mut bold, mut code) = (false, false);
    let mut chars = s.chars().peekable();
    let flush = |buf: &mut String, out: &mut Vec<Span<'static>>, bold: bool, code: bool| {
        if buf.is_empty() {
            return;
        }
        let st = if code {
            fg(c_orange()).bg(c_surface())
        } else if bold {
            base.add_modifier(Modifier::BOLD)
        } else {
            base
        };
        out.push(Span::styled(std::mem::take(buf), st));
    };
    while let Some(c) = chars.next() {
        if c == '`' {
            flush(&mut buf, &mut out, bold, code);
            code = !code;
        } else if c == '*' && !code && chars.peek() == Some(&'*') {
            chars.next();
            flush(&mut buf, &mut out, bold, code);
            bold = !bold;
        } else {
            buf.push(c);
        }
    }
    flush(&mut buf, &mut out, bold, code);
    out
}

/// Renders markdown source into styled (not yet wrapped) lines.
pub fn render(src: &str) -> Vec<Line<'static>> {
    let mut in_code = false;
    let mut lines = vec![];
    for raw in src.split('\n') {
        let t = raw.trim_start();
        if t.starts_with("```") {
            in_code = !in_code;
            let lang = t.trim_start_matches('`').trim();
            if in_code && !lang.is_empty() {
                lines.push(Line::styled(format!("  ┌ {lang}"), fg(c_dim())));
            }
            continue;
        }
        let base = fg(c_fg());
        if in_code {
            lines.push(Line::styled(
                format!("  {raw}"),
                fg(c_orange()).bg(c_surface()),
            ));
        } else if let Some(h) = t.strip_prefix('#') {
            let h = h.trim_start_matches('#').trim();
            lines.push(Line::styled(h.to_string(), bold(c_purple())));
        } else if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            let indent = " ".repeat(raw.len() - t.len());
            let mut spans = vec![Span::styled(format!("{indent}• "), fg(c_accent()))];
            spans.extend(inline(item, base));
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(inline(raw, base)));
        }
    }
    lines
}

fn trim_spaces(mut r: &[(char, Style)]) -> &[(char, Style)] {
    while let Some((&(' ', _), rest)) = r.split_last() {
        r = rest;
    }
    r
}

/// Word-wraps a styled line to `width` display columns (long words are hard-split).
pub fn wrap(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| s.content.chars().map(|c| (c, line.style.patch(s.style))))
        .collect();
    let w = |c: char| c.width().unwrap_or(0);
    let mut rows: Vec<&[(char, Style)]> = vec![];
    let (mut start, mut col, mut last_space) = (0, 0, None);
    let mut i = 0;
    while i < cells.len() {
        let c = cells[i].0;
        if c != ' ' && col + w(c) > width && i > start {
            let cut = match last_space {
                Some(sp) if sp >= start => sp + 1,
                _ => i,
            };
            rows.push(trim_spaces(&cells[start..cut]));
            start = cut;
            last_space = None;
            col = cells[start..i].iter().map(|x| w(x.0)).sum();
            continue;
        }
        if c == ' ' {
            last_space = Some(i);
        }
        col += w(c);
        i += 1;
    }
    rows.push(&cells[start..]);
    rows.into_iter()
        .map(|r| {
            let mut spans: Vec<Span<'static>> = vec![];
            for &(c, st) in r {
                match spans.last_mut() {
                    Some(s) if s.style == st => s.content.to_mut().push(c),
                    _ => spans.push(Span::styled(c.to_string(), st)),
                }
            }
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn renders_blocks_and_inline() {
        let l = render("# Title\n- item **b** `c`\n```rs\nlet x;\n```\nplain");
        let t: Vec<_> = l.iter().map(text).collect();
        assert_eq!(t, ["Title", "• item b c", "  ┌ rs", "  let x;", "plain"]);
        assert_eq!(l[0].style.fg, Some(c_purple()));
        assert!(
            l[1].spans[1..]
                .iter()
                .any(|s| s.style.fg == Some(c_orange()))
        );
        assert!(
            l[1].spans
                .iter()
                .any(|s| s.content == "b" && s.style.add_modifier.contains(Modifier::BOLD))
        );
        assert_eq!(l[3].style.bg, Some(c_surface()));
    }

    #[test]
    fn wraps_on_words_and_hard_splits() {
        let rows = wrap(&Line::raw("hello big world"), 9);
        assert_eq!(
            rows.iter().map(text).collect::<Vec<_>>(),
            ["hello big", "world"]
        );
        let rows = wrap(&Line::raw("abcdefgh"), 3);
        assert_eq!(
            rows.iter().map(text).collect::<Vec<_>>(),
            ["abc", "def", "gh"]
        );
        assert_eq!(wrap(&Line::raw(""), 5).len(), 1);
        // wide chars count as 2 columns
        assert_eq!(wrap(&Line::raw("日本語"), 4).len(), 2);
    }
}
