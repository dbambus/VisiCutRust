//! DXF text decoding: `\U+XXXX` escapes, TEXT control codes (`%%d`, …) and
//! MTEXT inline formatting.

/// Replaces `\U+XXXX` (and `\M+nXXXX`, which is dropped) escapes.
pub fn unicode_escapes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\'
            && matches!(chars.get(i + 1), Some('U' | 'u'))
            && chars.get(i + 2) == Some(&'+')
            && let Some(code) = chars
                .get(i + 3..i + 7)
                .and_then(|h| u32::from_str_radix(&h.iter().collect::<String>(), 16).ok())
        {
            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
            i += 7;
            continue;
        }
        if chars[i] == '\\'
            && matches!(chars.get(i + 1), Some('M' | 'm'))
            && chars.get(i + 2) == Some(&'+')
        {
            // Multibyte "\M+nXXXX" of old Asian code pages: not decodable here.
            out.push('\u{FFFD}');
            i = (i + 8).min(chars.len());
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Decodes a TEXT/ATTRIB value: `%%d` °, `%%p` ±, `%%c` ⌀, `%%nnn` and
/// drops the underline/overline toggles.
pub fn single_line(text: &str) -> String {
    let text = unicode_escapes(text);
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' && chars.get(i + 1) == Some(&'%') {
            match chars.get(i + 2).map(|c| c.to_ascii_lowercase()) {
                Some('d') => out.push('°'),
                Some('p') => out.push('±'),
                Some('c') => out.push('⌀'),
                Some('%') => out.push('%'),
                Some('u' | 'o' | 'k') => {}
                Some(c) if c.is_ascii_digit() => {
                    let digits: String = chars[i + 2..]
                        .iter()
                        .take(3)
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if let Some(c) = digits.parse().ok().and_then(char::from_u32) {
                        out.push(c);
                    }
                    i += 2 + digits.len();
                    continue;
                }
                _ => {
                    out.push('%');
                    i += 1;
                    continue;
                }
            }
            i += 3;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Plain lines of an MTEXT value, without formatting codes.
pub fn mtext_lines(text: &str) -> Vec<String> {
    let text = single_line(text);
    let chars: Vec<char> = text.chars().collect();
    let mut lines = vec![String::new()];
    let mut i = 0;
    let push = |lines: &mut Vec<String>, c: char| lines.last_mut().unwrap().push(c);
    while i < chars.len() {
        let c = chars[i];
        match c {
            '{' | '}' => i += 1,
            '\\' => {
                let Some(code) = chars.get(i + 1).copied() else {
                    break;
                };
                i += 2;
                match code {
                    'P' | 'X' => lines.push(String::new()),
                    '~' => push(&mut lines, '\u{a0}'),
                    '\\' | '{' | '}' => push(&mut lines, code),
                    'L' | 'l' | 'O' | 'o' | 'K' | 'k' | 'N' => {}
                    'S' => {
                        // Stacked fraction "\Sa^b;", "\Sa/b;" or "\Sa#b;".
                        while i < chars.len() && chars[i] != ';' {
                            match chars[i] {
                                '^' | '#' => push(&mut lines, '/'),
                                '\\' => {}
                                other => push(&mut lines, other),
                            }
                            i += 1;
                        }
                        i += 1;
                    }
                    // Codes with a parameter up to ';' (font, height, colour, …).
                    _ => {
                        while i < chars.len() && chars[i] != ';' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
            }
            '\n' => {
                lines.push(String::new());
                i += 1;
            }
            _ => {
                push(&mut lines, c);
                i += 1;
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_control_codes() {
        assert_eq!(single_line("45%%d %%p0,1 %%c8 100%%%"), "45° ±0,1 ⌀8 100%");
        assert_eq!(single_line("%%uUnter%%u \\U+00FCber"), "Unter über");
        assert_eq!(
            mtext_lines("{\\fArial|b1;Hallo}\\PWelt \\S1^2; \\H2.5x;groß"),
            vec!["Hallo", "Welt 1/2 groß"]
        );
    }
}
