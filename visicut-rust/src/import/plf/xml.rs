//! Small text-level XML helpers: documents are edited in place so that
//! everything the importer does not touch stays byte-identical.
use std::ops::Range;

/// Index just after the `>` that ends the start tag beginning at `start`.
pub fn start_tag_end(source: &str, start: usize) -> usize {
    let mut quote = None;
    for (offset, c) in source[start..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return start + offset + 1,
            _ => {}
        }
    }
    source.len()
}

/// Qualified name of the element whose start tag begins at `start`.
pub fn qualified_name(source: &str, start: usize) -> &str {
    let rest = &source[start + 1..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(rest.len());
    &rest[..end]
}

/// Whether the start tag ending at `end` (exclusive) is self-closing.
pub fn is_self_closing(source: &str, end: usize) -> bool {
    source[..end]
        .trim_end_matches('>')
        .trim_end()
        .ends_with('/')
}

/// The start tag `source[start..end]` without the attribute ranges in
/// `removed`, with `added` attributes appended before the closing bracket.
pub fn rewrite_start_tag(
    source: &str,
    start: usize,
    end: usize,
    removed: &[Range<usize>],
    added: &[(String, String)],
) -> String {
    let mut removed: Vec<&Range<usize>> = removed.iter().collect();
    removed.sort_by_key(|r| r.start);
    let mut tag = String::new();
    let mut position = start;
    for range in removed {
        if range.start >= position && range.end <= end {
            // Drop the whitespace before the attribute as well.
            let kept = source[position..range.start].trim_end();
            tag.push_str(kept);
            position = range.end;
        }
    }
    tag.push_str(&source[position..end]);
    let closing = if is_self_closing(&tag, tag.len()) {
        "/>"
    } else {
        ">"
    };
    let mut tag = tag
        .trim_end_matches('>')
        .trim_end()
        .trim_end_matches('/')
        .trim_end()
        .to_string();
    for (name, value) in added {
        tag.push_str(&format!(" {name}=\"{}\"", escape_attribute(value)));
    }
    tag.push_str(closing);
    tag
}

/// Applies non-overlapping replacements to `text`, whose first byte is at
/// `base` in the coordinates of the ranges.
pub fn apply_edits(text: &str, base: usize, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut text = text.to_string();
    for (range, replacement) in edits {
        if range.start >= base && range.end <= base + text.len() {
            text.replace_range(range.start - base..range.end - base, &replacement);
        }
    }
    text
}

/// Declares `xmlns:th` on the root element when the document uses `th:`
/// attributes without declaring the prefix (Java's DOM parser accepts that).
pub fn declare_thymeleaf(source: &str) -> String {
    if source.contains("xmlns:th=") || !source.contains("th:") {
        return source.to_string();
    }
    let mut position = 0;
    while let Some(offset) = source[position..].find('<') {
        let start = position + offset;
        let rest = &source[start..];
        let end = if rest.starts_with("<?") {
            rest.find("?>").map(|i| i + 2)
        } else if rest.starts_with("<!--") {
            rest.find("-->").map(|i| i + 3)
        } else if rest.starts_with("<!") {
            // DOCTYPE, possibly with an internal subset in brackets.
            let close = rest.find(']').unwrap_or(0);
            rest[close..].find('>').map(|i| close + i + 1)
        } else {
            let at = start + 1 + qualified_name(source, start).len();
            return format!(
                "{} xmlns:th=\"http://www.thymeleaf.org\"{}",
                &source[..at],
                &source[at..]
            );
        };
        position = start + end.unwrap_or(rest.len());
    }
    source.to_string()
}

pub fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

pub fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Removes a DOCTYPE and replaces references to the internal entities it
/// declares (Adobe Illustrator declares namespace URIs this way), because the
/// rest of the app parses SVG without DTD support.
pub fn strip_doctype(source: &str) -> String {
    let Some(start) = source.find("<!DOCTYPE") else {
        return source.to_string();
    };
    let mut end = None;
    let mut depth = 0;
    let mut quote = None;
    for (offset, c) in source[start..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[') => depth += 1,
            (None, ']') => depth -= 1,
            (None, '>') if depth == 0 => {
                end = Some(start + offset + 1);
                break;
            }
            _ => {}
        }
    }
    let Some(end) = end else {
        return source.to_string();
    };
    let declarations = &source[start..end];
    let mut entities = Vec::new();
    let mut rest = declarations;
    while let Some(index) = rest.find("<!ENTITY") {
        rest = &rest[index + 8..];
        let trimmed = rest.trim_start();
        if trimmed.starts_with('%') {
            continue;
        }
        let name_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        let name = &trimmed[..name_end];
        let after = trimmed[name_end..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            continue;
        };
        if let Some(close) = after[1..].find(q) {
            entities.push((format!("&{name};"), after[1..1 + close].to_string()));
        }
    }
    let mut body = source[end..].to_string();
    for (reference, value) in entities {
        body = body.replace(&reference, &value);
    }
    format!("{}{body}", &source[..start])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_start_tags() {
        let source = r#"<a x="1" y='>' z="3"/>"#;
        let end = start_tag_end(source, 0);
        assert_eq!(end, source.len());
        assert!(is_self_closing(source, end));
        let tag = rewrite_start_tag(
            source,
            0,
            end,
            std::slice::from_ref(&(3..8)),
            &[("w".into(), "a\"b".into())],
        );
        assert_eq!(tag, r#"<a y='>' z="3" w="a&quot;b"/>"#);
        assert_eq!(qualified_name("<svg:svg a='1'>", 0), "svg:svg");
    }

    #[test]
    fn expands_internal_entities() {
        let source = "<?xml version=\"1.0\"?>\n<!DOCTYPE svg [ <!ENTITY ns_svg \"http://www.w3.org/2000/svg\"> ]>\n<svg xmlns=\"&ns_svg;\"/>";
        let stripped = strip_doctype(source);
        assert!(!stripped.contains("DOCTYPE"));
        assert!(stripped.contains("xmlns=\"http://www.w3.org/2000/svg\""));
    }
}
