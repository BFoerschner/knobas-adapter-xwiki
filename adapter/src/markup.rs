//! XWiki 2.1 source, as far as the docs writes need it.
//!
//! knobas edits a page one **section** at a time: what follows a top-level
//! heading up to the next one (ADR-0040, the docs writes). The reader draws
//! the page from XWiki's rendered HTML ([`crate::docs`]); the writes work on
//! the page's wiki source, because what is sent back is the whole source
//! with one section replaced and every other byte kept.
//!
//! * [`sections`] finds the top-level headings: `= Title =` to
//!   `====== Title ======` at the start of a line, outside every macro
//!   (`{{info}}…{{/info}}`), verbatim block (`{{{…}}}`) and group
//!   (`(((…)))`). A heading inside one of those is not top-level, exactly as
//!   it is not a top-level block in the rendered HTML.
//! * [`section_text`] is a section as the editor's plain text, or why
//!   knobas does not edit it: a table, code, a macro, or any formatting
//!   (marks, links, images, nested or ordered lists, a paragraph that holds
//!   a line break).
//! * [`from_text`] is the editor's plain text as XWiki 2.1, every character
//!   that could start markup escaped with `~`, so what the reader typed is
//!   what the page says.

/// A top-level heading's line and the section after it, as byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The heading's level, 1 to 6.
    pub level: u8,
    /// The heading's text, markup and all.
    pub title: String,
    /// Where the heading line starts.
    pub heading_start: usize,
    /// Where the body starts: just past the heading line's newline.
    pub body_start: usize,
    /// Where the body ends: the next top-level heading's line, or the end.
    pub body_end: usize,
}

/// Why a section is not edited in knobas, in the docs vocabulary's words.
pub use knobas_adapter_sdk::spi::docs::SectionRefusal as Refusal;

/// The heading on `line`, when it is one: its level and its text.
fn heading(line: &str) -> Option<(u8, String)> {
    let trimmed = line.trim();
    let level = trimmed.bytes().take_while(|b| *b == b'=').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = trimmed[level..].trim_start();
    // `=` alone, or `==` with nothing after it, is no heading.
    if rest.is_empty() || !trimmed[level..].starts_with([' ', '\t']) {
        return None;
    }
    let title = rest.trim_end_matches('=').trim_end();
    Some((u8::try_from(level).ok()?, title.to_owned()))
}

/// What a line opens and closes, for the nesting walk. Returns the depth
/// after the line, given the depth before it, and the stack of open macros.
struct Nesting {
    /// Open macro names, innermost last.
    macros: Vec<String>,
    /// Open `(((` groups.
    groups: usize,
    /// Inside `{{{ … }}}`.
    verbatim: bool,
}

impl Nesting {
    fn top_level(&self) -> bool {
        self.macros.is_empty() && self.groups == 0 && !self.verbatim
    }

    /// Walk one line's markup, updating what is open.
    fn line(&mut self, line: &str) {
        let bytes = line.as_bytes();
        let mut at = 0;
        while at < bytes.len() {
            let rest = &line[at..];
            if self.verbatim {
                match rest.find("}}}") {
                    Some(end) => {
                        self.verbatim = false;
                        at += end + 3;
                        continue;
                    }
                    None => return,
                }
            }
            if bytes[at] == b'~' {
                at += 2;
                continue;
            }
            if rest.starts_with("{{{") {
                self.verbatim = true;
                at += 3;
            } else if let Some(name) = rest.strip_prefix("{{/") {
                let name: String = name
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                if let Some(open) = self.macros.iter().rposition(|m| *m == name) {
                    self.macros.truncate(open);
                }
                at += 3 + name.len();
            } else if let Some(tail) = rest.strip_prefix("{{") {
                let name: String = tail
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                // Inside a macro whose content is not wiki markup (code,
                // html), only its own close counts; we keep it simple and
                // treat every macro the same, which is right for the close.
                let close = tail.find("}}").map(|i| &tail[..i]);
                match close {
                    Some(inner) if inner.trim_end().ends_with('/') => {}
                    _ if !name.is_empty() => self.macros.push(name.clone()),
                    _ => {}
                }
                at += 2 + name.len();
            } else if rest.starts_with("(((") {
                self.groups += 1;
                at += 3;
            } else if rest.starts_with(")))") {
                self.groups = self.groups.saturating_sub(1);
                at += 3;
            } else {
                at += rest.chars().next().map_or(1, char::len_utf8);
            }
        }
    }
}

/// The lines of `source` with the byte offset each starts at and the offset
/// just past its newline.
fn lines(source: &str) -> impl Iterator<Item = (usize, usize, &str)> {
    let mut at = 0;
    source.split_inclusive('\n').map(move |line| {
        let start = at;
        at += line.len();
        (start, at, line.trim_end_matches(['\n', '\r']))
    })
}

/// The top-level sections of `source`, in order.
#[must_use]
pub fn sections(source: &str) -> Vec<Section> {
    let mut nesting = Nesting {
        macros: Vec::new(),
        groups: 0,
        verbatim: false,
    };
    let mut found: Vec<Section> = Vec::new();
    for (start, end, line) in lines(source) {
        if nesting.top_level()
            && let Some((level, title)) = heading(line)
        {
            if let Some(last) = found.last_mut() {
                last.body_end = start;
            }
            found.push(Section {
                level,
                title,
                heading_start: start,
                body_start: end,
                body_end: source.len(),
            });
            continue;
        }
        nesting.line(line);
    }
    found
}

/// Whether `text` holds anything that is XWiki inline markup rather than
/// words, reading `~x` as the escaped character `x`. Answers the refusal.
fn inline_markup(text: &str) -> Option<Refusal> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '~' {
            i += 2;
            continue;
        }
        let pair = next.map(|n| (c, n));
        match pair {
            Some(('{', '{')) => {
                return Some(if chars.get(i + 2) == Some(&'{') {
                    Refusal::Code
                } else {
                    Refusal::Macro
                });
            }
            Some(('*', '*') | ('/', '/') | ('_', '_') | ('-', '-') | ('#', '#'))
            | Some(('^', '^') | (',', ',') | ('[', '[') | (']', ']') | ('(', '%'))
            | Some(('(', '(') | ('\\', '\\') | ('<', '<') | ('>', '>')) => {
                return Some(Refusal::Formatting);
            }
            _ => {}
        }
        // A free-standing URL is a link in XWiki 2.1.
        if c == ':' && chars.get(i + 1) == Some(&'/') && chars.get(i + 2) == Some(&'/') {
            return Some(Refusal::Formatting);
        }
        i += 1;
    }
    None
}

/// `text` with every `~x` read as `x`.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '~' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// One section's body as the editor's plain text: a paragraph per block, a
/// blank line between blocks, `- ` per item of a flat bulleted list. The
/// first thing a plain text box cannot hold refuses it.
///
/// # Errors
///
/// The [`Refusal`] for the first part that is not plain text.
pub fn section_text(body: &str) -> Result<String, Refusal> {
    let mut blocks: Vec<String> = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut items: Vec<String> = Vec::new();
    let flush = |blocks: &mut Vec<String>,
                 paragraph: &mut Vec<&str>,
                 items: &mut Vec<String>|
     -> Result<(), Refusal> {
        match paragraph.len() {
            0 => {}
            // XWiki 2.1 keeps a newline inside a paragraph as a line break,
            // which a plain paragraph of the editor cannot hold.
            1 => blocks.push(unescape(paragraph[0].trim())),
            _ => return Err(Refusal::Formatting),
        }
        paragraph.clear();
        if !items.is_empty() {
            blocks.push(items.join("\n"));
            items.clear();
        }
        Ok(())
    };
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut items)?;
            continue;
        }
        if trimmed.starts_with("{{{") {
            return Err(Refusal::Code);
        }
        if trimmed.starts_with("{{") {
            return Err(Refusal::Macro);
        }
        if trimmed.starts_with('|') || trimmed.starts_with("!=") {
            return Err(Refusal::Table);
        }
        if let Some(refusal) = inline_markup(trimmed) {
            return Err(refusal);
        }
        if let Some(item) = trimmed.strip_prefix("* ") {
            if !paragraph.is_empty() {
                flush(&mut blocks, &mut paragraph, &mut items)?;
            }
            items.push(format!("- {}", unescape(item.trim())));
            continue;
        }
        // Every other line-start construct: nested or ordered lists,
        // definition lists, quotes, a horizontal line, parameters.
        let first = trimmed.chars().next().unwrap_or(' ');
        let ordered = trimmed.split_once(". ").is_some_and(|(n, _)| {
            !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '*')
        });
        if matches!(first, '*' | '>' | ';' | ':' | '=') || ordered || trimmed.starts_with("----") {
            return Err(Refusal::Formatting);
        }
        if !items.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut items)?;
        }
        paragraph.push(trimmed);
    }
    flush(&mut blocks, &mut paragraph, &mut items)?;
    Ok(blocks.join("\n\n"))
}

/// Plain words as XWiki 2.1 inline text: `~` before every character that
/// could start markup with its neighbour, so nothing the reader typed is
/// read as markup.
#[must_use]
pub fn escape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        let next = chars.get(i + 1).copied();
        let paired = match (c, next) {
            ('~', _) => true,
            (
                '{' | '}' | '*' | '/' | '_' | '-' | '#' | '^' | ',' | '[' | ']' | '\\' | '<' | '>',
                Some(n),
            ) => n == c,
            ('(', Some('(' | '%')) | (')', Some(')')) => true,
            (':', Some('/')) => chars.get(i + 2) == Some(&'/'),
            _ => false,
        };
        // A lone character that starts a construct at the start of a line.
        let leading = i == 0
            && (matches!(c, '*' | '=' | '|' | '>' | ';' | ':' | '!')
                || (c.is_ascii_digit() && text.contains(". ")));
        if paired || leading {
            out.push('~');
        }
        out.push(c);
    }
    out
}

/// The editor's plain text as XWiki 2.1: a `## ` line a level-2 heading,
/// consecutive `- ` lines one bulleted list, other consecutive lines one
/// paragraph (joined by a space), blocks separated by a blank line. What a
/// new page's body and a section edit's replacement are written in.
#[must_use]
pub fn from_text(text: &str) -> String {
    let mut blocks: Vec<String> = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut items: Vec<String> = Vec::new();
    let flush = |blocks: &mut Vec<String>, paragraph: &mut Vec<&str>, items: &mut Vec<String>| {
        if !paragraph.is_empty() {
            blocks.push(escape(&paragraph.join(" ")));
            paragraph.clear();
        }
        if !items.is_empty() {
            blocks.push(items.join("\n"));
            items.clear();
        }
    };
    for line in text.lines().map(str::trim) {
        if let Some(title) = line.strip_prefix("## ") {
            flush(&mut blocks, &mut paragraph, &mut items);
            blocks.push(format!("== {} ==", escape(title.trim())));
        } else if let Some(item) = line.strip_prefix("- ") {
            if !paragraph.is_empty() {
                flush(&mut blocks, &mut paragraph, &mut items);
            }
            items.push(format!("* {}", escape(item.trim())));
        } else if line.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut items);
        } else {
            if !items.is_empty() {
                flush(&mut blocks, &mut paragraph, &mut items);
            }
            paragraph.push(line);
        }
    }
    flush(&mut blocks, &mut paragraph, &mut items);
    blocks.join("\n\n")
}

/// `source` with section `index`'s body (0-based) replaced by `text` as
/// XWiki 2.1, every byte outside the body kept. The body keeps a blank line
/// before the next heading.
#[must_use]
pub fn replace_section(source: &str, section: &Section, text: &str) -> String {
    let old = &source[section.body_start..section.body_end];
    let mut body = String::new();
    // Keep the blank line the author left after the heading, if any.
    if old.starts_with('\n') || old.starts_with("\r\n") {
        body.push('\n');
    }
    body.push_str(&from_text(text));
    if section.body_end < source.len() {
        body.push_str("\n\n");
    } else if source.ends_with('\n') {
        body.push('\n');
    }
    format!(
        "{}{}{}",
        &source[..section.body_start],
        body,
        &source[section.body_end..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "Lead paragraph.\n\n= Backoff policy =\n\nBase 30 s, factor 2.\n\n* one\n* two\n\n== Codes ==\n\n{{code language=\"text\"}}\n= not a heading =\n{{/code}}\n\n{{info}}\n= nor this =\n{{/info}}\n\n= Manual review queue =\n\nSLA to be **defined**.\n";

    #[test]
    fn only_top_level_headings_open_sections() {
        let found = sections(PAGE);
        let titles: Vec<_> = found.iter().map(|s| (s.level, s.title.as_str())).collect();
        assert_eq!(
            titles,
            [
                (1, "Backoff policy"),
                (2, "Codes"),
                (1, "Manual review queue")
            ]
        );
        assert_eq!(
            &PAGE[found[0].body_start..found[0].body_end],
            "\nBase 30 s, factor 2.\n\n* one\n* two\n\n"
        );
    }

    #[test]
    fn a_section_reads_as_plain_text_or_says_why_not() {
        let found = sections(PAGE);
        let body = |i: usize| &PAGE[found[i].body_start..found[i].body_end];
        assert_eq!(
            section_text(body(0)),
            Ok("Base 30 s, factor 2.\n\n- one\n- two".into())
        );
        assert_eq!(section_text(body(1)), Err(Refusal::Macro));
        assert_eq!(section_text(body(2)), Err(Refusal::Formatting));
        assert_eq!(section_text("|=a|=b\n|1|2"), Err(Refusal::Table));
        assert_eq!(section_text("{{{verbatim}}}"), Err(Refusal::Code));
        assert_eq!(
            section_text("see https://example.com"),
            Err(Refusal::Formatting)
        );
        assert_eq!(section_text("one\ntwo"), Err(Refusal::Formatting));
        assert_eq!(section_text("1. first"), Err(Refusal::Formatting));
        assert_eq!(section_text("** nested"), Err(Refusal::Formatting));
    }

    #[test]
    fn typed_text_round_trips_through_the_markup() {
        let typed =
            "Spring tides run **fast** -- see [[x]] or {{html}}.\n\n- one // two\n- 1. three";
        let markup = from_text(typed);
        assert_eq!(section_text(&markup), Ok(typed.to_owned()));
        let lead = "* not a list\n\n= not a heading\n\n|not a table";
        assert_eq!(section_text(&from_text(lead)), Ok(lead.to_owned()));
    }

    #[test]
    fn an_edit_replaces_one_section_and_keeps_every_other_byte() {
        let found = sections(PAGE);
        let edited = replace_section(PAGE, &found[0], "Base 60 s.");
        assert_eq!(
            edited,
            PAGE.replace("Base 30 s, factor 2.\n\n* one\n* two\n\n", "Base 60 s.\n\n")
        );
        let last = replace_section(PAGE, &found[2], "Two days.");
        assert!(
            last.ends_with("= Manual review queue =\n\nTwo days.\n"),
            "{last}"
        );
        assert!(last.starts_with(&PAGE[..found[2].body_start]));
    }

    #[test]
    fn a_new_page_body_is_headings_lists_and_paragraphs() {
        assert_eq!(
            from_text("## Docs contract\nA new page.\n\n- one item"),
            "== Docs contract ==\n\nA new page.\n\n* one item"
        );
    }
}
