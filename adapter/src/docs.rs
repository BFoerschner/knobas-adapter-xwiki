//! The docs projection: a mirrored page drawn in knobas' docs vocabulary,
//! and the docs writes beside it (ADR-0040).
//!
//! knobas calls these with the payload this adapter's sync wrote and
//! nothing else: no instance, no network (the host refuses every request),
//! at most 5 s. Every answer is a plain function of the payload.
//!
//! **The body** is XWiki's rendered page in annotated HTML
//! ([`crate::annotated`]), read by knobas' shared HTML reader. The reader
//! knows plain HTML; this module's [`XWiki`] hook answers for XWiki's own
//! parts, marked in the annotated HTML:
//!
//! | XWiki | knobas |
//! |---|---|
//! | `{{info}}`, `{{warning}}`, `{{error}}`, `{{success}}` | a panel: info, warning, warning, tip |
//! | `{{box}}` | a note panel with the box's title |
//! | `{{code}}` | a code block with its language, or code text inline |
//! | `{{html}}` | an HTML embed, the macro's own markup |
//! | `{{include}}`, `{{display}}` | an include, which knobas fills from the mirror |
//! | `{{mention}}` | a mention |
//! | `{{toc}}`, `{{id}}` | nothing: knobas draws its own outline |
//! | `{{children}}` and other live trees, a macro that failed to render | a placeholder named after the macro |
//! | any other macro (`{{velocity}}`, …) | what it rendered to |
//! | a link to a synced page | a page link; to another page or the web, a link |
//! | an image | a link to the image |
//!
//! **Sections** come from the page's XWiki 2.1 source ([`crate::markup`]),
//! since that is what a section edit splices. The reader numbers sections
//! by the body's top-level headings, so the two must agree: when the
//! source's top-level headings are not the body's (a heading a script
//! printed, a heading inside a `(((group)))`), every section is drawn
//! refused rather than risk editing the wrong one.

use knobas_adapter_sdk::html::{self, Node};
use knobas_adapter_sdk::spi::docs::{
    DocBlock, DocComment, DocContent, DocSection, DocVersion, EditRefused, EmbedContent, Inline,
    PAGE_KIND, PanelKind, SectionEdit, SectionRefusal,
};
use knobas_adapter_sdk::{Hook, Hooked, Reading};
use serde_json::Value;

use crate::annotated::{self, param};
use crate::markup;
use crate::page::{self, Extra};

/// Macros whose output is a live widget the reader cannot draw.
const LIVE: &[&str] = &[
    "children",
    "documentTree",
    "tree",
    "gallery",
    "chart",
    "liveData",
    "liveTable",
    "uiextensions",
    "notifications",
    "dashboard",
    "attachments",
    "activity",
];

/// Macros that only carry other content: a rendering error inside them is
/// the enclosing macro's.
const CARRIERS: &[&str] = &[
    "velocity",
    "groovy",
    "python",
    "template",
    "translation",
    "wikimacrocontent",
];

/// `node`'s attribute `key`, entities decoded.
fn attr(node: &Node, key: &str) -> Option<String> {
    node.attr(key).map(html::decode_entities)
}

fn classes(node: &Node) -> Vec<String> {
    attr(node, "class")
        .map(|c| c.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// The first element under `nodes`, depth first, that `wanted` accepts.
fn find<'a>(nodes: &'a [Node], wanted: &dyn Fn(&Node) -> bool) -> Option<&'a Node> {
    nodes.iter().find_map(|node| {
        if node.name().is_some() && wanted(node) {
            Some(node)
        } else {
            find(node.children(), wanted)
        }
    })
}

/// Whether a macro's own rendering failed: XWiki then renders an error box
/// in its place, inside the macro or inside a carrier macro of it, never
/// inside another macro (which answers for itself).
fn failed(nodes: &[Node]) -> bool {
    nodes.iter().any(|node| match node.name() {
        Some("xw:macro") => {
            let name = attr(node, "name").unwrap_or_default();
            let params = attr(node, "params").unwrap_or_default();
            (name == "box" && params.contains("xwikirenderingerror"))
                || (CARRIERS.contains(&name.as_str()) && failed(node.children()))
        }
        Some(_) => {
            classes(node).iter().any(|c| c == "xwikirenderingerror") || failed(node.children())
        }
        None => false,
    })
}

/// Rename the inline spans XWiki marks monospace and verbatim to `code`,
/// which the reader draws as code text.
fn rewrite(nodes: &mut [Node]) {
    for node in nodes {
        let is_code = node.name() == Some("span")
            && classes(node)
                .iter()
                .any(|c| c == "monospace" || c == "wikimodel-verbatim");
        if let Node::Element { name, children, .. } = node {
            rewrite(children);
            if is_code {
                *name = "code".into();
            }
        }
    }
}

/// Headings inside a macro's output read as bold paragraphs, so they open
/// no section of their own (sections are the source's top-level headings).
fn demote(blocks: Vec<DocBlock>) -> Vec<DocBlock> {
    blocks
        .into_iter()
        .map(|block| match block {
            DocBlock::Heading { text, .. } => DocBlock::Paragraph {
                text: text
                    .into_iter()
                    .map(|inline| match inline {
                        Inline::Text {
                            text,
                            emphasis,
                            code,
                            ..
                        } => Inline::Text {
                            text,
                            strong: true,
                            emphasis,
                            code,
                        },
                        other => other,
                    })
                    .collect(),
            },
            other => other,
        })
        .collect()
}

/// The hook: XWiki's parts of the rendered page.
pub struct XWiki<'a> {
    pub extra: &'a Extra,
}

impl XWiki<'_> {
    /// `href` as an address a reader can open: absolute as it is, made
    /// absolute against the instance when it starts with `/`, `None` for an
    /// anchor on the same page.
    fn absolute(&self, href: &str) -> Option<String> {
        if href.starts_with('#') || href.is_empty() {
            None
        } else if href.starts_with('/') {
            Some(format!("{}{href}", self.extra.origin))
        } else {
            Some(href.to_owned())
        }
    }

    /// The page `reference` names, if this source syncs it.
    /// The synced page a link or an include names, looked up in the
    /// spellings the sync recorded it under ([`crate::sync::candidates`]).
    fn target(&self, element: &Node) -> Option<&page::Target> {
        crate::sync::candidates(element)
            .iter()
            .find_map(|candidate| self.extra.pages.get(candidate))
    }

    /// The blocks of a message box or a box: its content container, not the
    /// icon and screen-reader label XWiki draws beside it.
    fn box_content(&self, element: &Node, reading: &Reading<'_>) -> Vec<DocBlock> {
        let content = find(element.children(), &|n| {
            classes(n).iter().any(|c| c == "xwiki-metadata-container")
                && n.attr("data-xwiki-parameter-name").is_none()
        });
        match content {
            Some(content) => reading.blocks(content.children()),
            None => reading.blocks(element.children()),
        }
    }

    fn macro_(&self, element: &Node, reading: &Reading<'_>) -> Hooked {
        let name = attr(element, "name").unwrap_or_default();
        let params = attr(element, "params").unwrap_or_default();
        let content = attr(element, "content").unwrap_or_default();
        let placeholder = |name: &str| Hooked::Blocks(vec![html::placeholder(name)]);
        if !CARRIERS.contains(&name.as_str()) && failed(element.children()) {
            return placeholder(&name);
        }
        let panel = |kind: PanelKind| {
            Hooked::Blocks(vec![DocBlock::Panel {
                kind,
                title: param(&params, "title").filter(|t| !t.is_empty()),
                blocks: self.box_content(element, reading),
            }])
        };
        match name.as_str() {
            "info" => panel(PanelKind::Info),
            "warning" | "error" => panel(PanelKind::Warning),
            "success" => panel(PanelKind::Tip),
            "box" => panel(PanelKind::Note),
            "code" => {
                let inline = element
                    .children()
                    .iter()
                    .find(|c| c.name().is_some())
                    .is_some_and(|c| c.name() == Some("span"));
                if inline {
                    let marks = reading.marks();
                    Hooked::Inlines(vec![Inline::Text {
                        text: content,
                        strong: marks.strong,
                        emphasis: marks.emphasis,
                        code: true,
                    }])
                } else {
                    Hooked::Blocks(vec![DocBlock::Code {
                        code: content,
                        language: param(&params, "language").filter(|l| !l.is_empty()),
                    }])
                }
            }
            "html" if content.trim().is_empty() => Hooked::Blocks(Vec::new()),
            "html" => Hooked::Blocks(vec![DocBlock::Embed {
                name: "html".into(),
                content: EmbedContent::Html { html: content },
            }]),
            "include" | "display" => {
                let reference = param(&params, "reference")
                    .or_else(|| param(&params, "page"))
                    .or_else(|| param(&params, "document"))
                    .unwrap_or_default();
                let (title, space) = match self.target(element) {
                    Some(target) => (target.title.clone(), target.space.clone()),
                    None => {
                        let local = page::local(&reference);
                        let (spaces, name) = page::split(local);
                        let title = if name == "WebHome" {
                            spaces.last().cloned().unwrap_or(name)
                        } else {
                            name
                        };
                        (title, page::top_space(local))
                    }
                };
                Hooked::Blocks(vec![DocBlock::include(title, Some(space), false)])
            }
            "mention" => {
                let reference = param(&params, "reference").unwrap_or_default();
                let shown = html::collapse(&element.text());
                let name = self
                    .extra
                    .people
                    .get(page::local(&reference))
                    .cloned()
                    .or_else(|| {
                        let shown = shown.trim_start_matches('@').to_owned();
                        (!shown.is_empty()).then_some(shown)
                    });
                Hooked::Inlines(vec![Inline::Mention {
                    user: (!reference.is_empty()).then(|| page::account(&reference)),
                    name,
                    me: false,
                }])
            }
            "toc" => Hooked::Blocks(Vec::new()),
            "id" => Hooked::Inlines(Vec::new()),
            live if LIVE.contains(&live) => placeholder(live),
            _ => {
                let has_heading = find(element.children(), &|n| {
                    matches!(n.name(), Some("h1" | "h2" | "h3" | "h4" | "h5" | "h6"))
                })
                .is_some();
                if has_heading {
                    Hooked::Blocks(demote(reading.blocks(element.children())))
                } else {
                    Hooked::Unwrap
                }
            }
        }
    }

    /// A link's words: its text, else an image's alt inside it, else the
    /// reference.
    fn words(element: &Node, fallback: &str) -> String {
        let text = html::collapse(&element.text());
        if !text.is_empty() {
            return text;
        }
        find(element.children(), &|n| n.name() == Some("img"))
            .and_then(|img| attr(img, "alt"))
            .filter(|alt| !alt.is_empty())
            .unwrap_or_else(|| fallback.to_owned())
    }

    fn link(&self, element: &Node, reading: &Reading<'_>) -> Hooked {
        let kind = attr(element, "kind").unwrap_or_default();
        let reference = attr(element, "ref").unwrap_or_default();
        let text = Self::words(element, &reference);
        let href = find(element.children(), &|n| n.name() == Some("a"))
            .and_then(|a| attr(a, "href"))
            .and_then(|href| self.absolute(&href));
        if kind == "doc" || kind == "page" || kind == "space" {
            if reference.is_empty() || reference == "null" {
                return Hooked::Inlines(vec![reading.text(text)]);
            }
            if let Some(target) = self.target(element) {
                return Hooked::Inlines(vec![Inline::PageLink {
                    text,
                    title: target.title.clone(),
                    space: Some(target.space.clone()),
                    entity_id: None,
                }]);
            }
            let missing = find(element.children(), &|n| {
                classes(n).iter().any(|c| c == "wikicreatelink")
            })
            .is_some();
            if missing {
                return Hooked::Inlines(vec![reading.text(text)]);
            }
        }
        Hooked::Inlines(vec![match href {
            Some(href) => Inline::Link { text, href },
            None => reading.text(text),
        }])
    }

    fn image(&self, element: &Node) -> Hooked {
        let img = if element.name() == Some("img") {
            Some(element)
        } else {
            find(element.children(), &|n| n.name() == Some("img"))
        };
        let Some(img) = img else {
            return Hooked::Inlines(Vec::new());
        };
        let alt = attr(img, "alt").filter(|a| !a.is_empty());
        let src = attr(img, "src").and_then(|s| self.absolute(&s));
        Hooked::Inlines(match src {
            Some(href) => vec![Inline::Link {
                text: alt.unwrap_or_else(|| "image".into()),
                href,
            }],
            None => Vec::new(),
        })
    }
}

impl Hook for XWiki<'_> {
    fn element(&self, element: &Node, reading: &Reading<'_>) -> Hooked {
        match element.name().unwrap_or_default() {
            "xw:macro" => self.macro_(element, reading),
            "xw:link" => self.link(element, reading),
            "xw:image" | "img" => self.image(element),
            // Form controls a macro may render: not words on the page.
            "input" | "button" | "select" | "textarea" | "form" | "svg" | "canvas" => {
                Hooked::Inlines(Vec::new())
            }
            _ => Hooked::Decline,
        }
    }
}

/// The rendered body's blocks.
fn body(extra: &Extra, source: &str) -> Vec<DocBlock> {
    let Some(rendered) = &extra.html else {
        return vec![
            DocBlock::Paragraph {
                text: vec![Inline::text(
                    "XWiki could not render this page; its source:",
                )],
            },
            DocBlock::Code {
                code: source.to_owned(),
                language: Some("xwiki/2.1".into()),
            },
        ];
    };
    let mut nodes = html::parse(&annotated::elements(rendered));
    rewrite(&mut nodes);
    html::blocks(&nodes, &XWiki { extra })
}

/// A comment's XWiki 2.1 source as blocks: a paragraph per blank-line
/// block, a line break per newline, a mention per `{{mention}}`, and the
/// rest as its words.
fn comment_blocks(text: &str, extra: &Extra) -> Vec<DocBlock> {
    text.split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|paragraph| {
            let mut inlines = Vec::new();
            for (i, line) in paragraph.lines().enumerate() {
                if i > 0 {
                    inlines.push(Inline::Break);
                }
                let mut rest = line;
                while let Some(at) = rest.find("{{mention") {
                    if at > 0 {
                        inlines.push(Inline::text(markup_words(&rest[..at])));
                    }
                    let tail = &rest[at..];
                    let end = tail.find("}}").map_or(tail.len(), |e| e + 2);
                    let params = tail["{{mention".len()..end.saturating_sub(2)]
                        .trim_end_matches('/')
                        .trim();
                    let reference = param(params, "reference").unwrap_or_default();
                    inlines.push(Inline::Mention {
                        user: Some(page::account(&reference)),
                        name: extra.people.get(page::local(&reference)).cloned(),
                        me: false,
                    });
                    rest = &tail[end..];
                }
                if !rest.is_empty() {
                    inlines.push(Inline::text(markup_words(rest)));
                }
            }
            DocBlock::Paragraph { text: inlines }
        })
        .collect()
}

/// A line of XWiki 2.1 read as words: escapes undone, the common marks
/// dropped. Comments are short and rarely formatted, so this is enough.
fn markup_words(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut words = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '~' {
            words.extend(next);
            i += 2;
        } else if next == Some(c)
            && matches!(c, '*' | '/' | '_' | '-' | '#')
            // `https://` is an address, not italics.
            && !(c == '/' && i > 0 && chars[i - 1] == ':')
        {
            i += 2;
        } else {
            words.push(c);
            i += 1;
        }
    }
    words
}

/// Milliseconds since the epoch as RFC 3339.
fn rfc3339(ms: Option<i64>) -> Option<String> {
    let ms = ms?;
    chrono::DateTime::from_timestamp_millis(ms).map(|t| t.to_rfc3339())
}

/// Plain letters and digits, lower-cased: what two spellings of a heading
/// share.
fn letters(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The body's top-level headings, as plain text.
fn headings(blocks: &[DocBlock]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|block| match block {
            DocBlock::Heading { text, .. } => Some(
                text.iter()
                    .map(|inline| match inline {
                        Inline::Text { text, .. }
                        | Inline::Link { text, .. }
                        | Inline::PageLink { text, .. } => text.clone(),
                        _ => String::new(),
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

/// One section's verdict, and where it is in the source when it is the
/// source's.
struct Verdict {
    source: Option<markup::Section>,
    text: Result<String, SectionRefusal>,
}

/// The page's sections, one per top-level heading of `blocks`, each with the
/// source section it edits when source and body agree.
fn verdicts(blocks: &[DocBlock], source: &str) -> Vec<Verdict> {
    let drawn = headings(blocks);
    let found = markup::sections(source);
    let agree = drawn.len() == found.len()
        && drawn
            .iter()
            .zip(&found)
            .all(|(d, s)| letters(d) == letters(&s.title.replace('~', "")));
    if agree {
        found
            .into_iter()
            .map(|section| Verdict {
                text: markup::section_text(&source[section.body_start..section.body_end]),
                source: Some(section),
            })
            .collect()
    } else {
        drawn
            .iter()
            .map(|_| Verdict {
                source: None,
                text: Err(SectionRefusal::Formatting),
            })
            .collect()
    }
}

/// The record's XWiki source and version, for the writes.
fn source_of(payload: &Value) -> Option<(&str, i64)> {
    Some((
        payload.get("content")?.as_str()?,
        payload.get("majorVersion")?.as_i64()?,
    ))
}

/// The docs projection: a `page` drawn, `None` for any other kind and a
/// payload that is not one of this adapter's pages.
#[must_use]
pub fn project(kind: &str, payload: &Value) -> Option<DocContent> {
    if kind != PAGE_KIND {
        return None;
    }
    let source = payload.get("content")?.as_str()?;
    payload.get(page::EXTRA)?;
    let extra = page::extra(payload);
    let blocks = body(&extra, source);
    let name = |reference: Option<&str>| {
        reference.map(|r| {
            extra
                .people
                .get(page::local(r))
                .cloned()
                .unwrap_or_else(|| page::account(r))
        })
    };
    let version = payload.get("majorVersion").and_then(Value::as_u64);
    let modifier = payload.get("modifier").and_then(Value::as_str);
    let creator = payload.get("creator").and_then(Value::as_str);
    let mut history = vec![DocVersion {
        number: version,
        by: modifier.map(page::account),
        by_name: name(modifier),
        at: rfc3339(payload.get("modified").and_then(Value::as_i64)),
        message: payload
            .get("comment")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_owned),
    }];
    if version.is_some_and(|v| v > 1) {
        history.push(DocVersion {
            number: Some(1),
            by: creator.map(page::account),
            by_name: name(creator),
            at: rfc3339(payload.get("created").and_then(Value::as_i64)),
            message: None,
        });
    }
    let comments = extra
        .comments
        .iter()
        .map(|comment| DocComment {
            id: Some(comment.id.to_string()),
            author: comment.author.as_deref().map(page::account),
            author_name: name(comment.author.as_deref()),
            at: rfc3339(comment.date),
            blocks: comment_blocks(&comment.text, &extra),
        })
        .collect();
    let sections = verdicts(&blocks, source)
        .into_iter()
        .zip(1u32..)
        .map(|(verdict, number)| DocSection {
            number,
            refusal: verdict.text.as_ref().err().copied(),
            text: verdict.text.ok(),
        })
        .collect();
    Some(DocContent {
        version,
        editor_name: name(modifier),
        labels: extra.tags.clone(),
        blocks,
        comments,
        excerpt: None,
        sections,
        history,
    })
}

/// The section edit: section `number` (from 1) of the page whose payload
/// is `payload`, its body replaced with `text`, against the version the
/// payload holds. The host queues the answer as an `update_page` write.
///
/// # Errors
///
/// [`EditRefused::Missing`] for another kind, no such section or a payload
/// without source or version; [`EditRefused::Section`] with the projection's
/// own reason for a section it draws refused.
pub fn edit_section(
    kind: &str,
    payload: &Value,
    number: u32,
    text: &str,
) -> Result<SectionEdit, EditRefused> {
    if kind != PAGE_KIND || payload.get(page::EXTRA).is_none() {
        return Err(EditRefused::Missing);
    }
    let (source, base_version) = source_of(payload).ok_or(EditRefused::Missing)?;
    let extra = page::extra(payload);
    let blocks = body(&extra, source);
    let index = usize::try_from(number)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .ok_or(EditRefused::Missing)?;
    let verdict = verdicts(&blocks, source)
        .into_iter()
        .nth(index)
        .ok_or(EditRefused::Missing)?;
    verdict.text.map_err(EditRefused::Section)?;
    let section = verdict.source.ok_or(EditRefused::Missing)?;
    Ok(SectionEdit {
        body: markup::replace_section(source, &section, text),
        base_version,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::page::{Comment, Space, Target};

    fn extra(html: &str) -> Extra {
        Extra {
            space: Space {
                key: "Tidewater".into(),
                name: "Tidewater".into(),
            },
            html: Some(html.to_owned()),
            origin: "http://wiki.example".into(),
            pages: BTreeMap::from([
                (
                    "Tidewater.Showcase.IncludedSnippet.WebHome".to_owned(),
                    Target {
                        title: "Included snippet".into(),
                        space: "Tidewater".into(),
                    },
                ),
                (
                    "Tidewater.Engineering.SEPAPayoutRetryDesign.WebHome".to_owned(),
                    Target {
                        title: "SEPA payout retry design".into(),
                        space: "Tidewater".into(),
                    },
                ),
            ]),
            people: BTreeMap::from([(
                "XWiki.MaraLindqvist".to_owned(),
                "Mara Lindqvist".to_owned(),
            )]),
            ..Extra::default()
        }
    }

    fn payload(html: &str, source: &str) -> Value {
        serde_json::json!({
            "fullName": "Tidewater.Showcase.Macros.WebHome",
            "title": "Macros",
            "content": source,
            "majorVersion": 3,
            "modifier": "XWiki.MaraLindqvist",
            "modified": 1_787_400_300_000_i64,
            "creator": "XWiki.knobas",
            "created": 1_787_000_000_000_i64,
            page::EXTRA: serde_json::to_value(extra(html)).unwrap(),
        })
    }

    fn drawn(html: &str) -> Vec<DocBlock> {
        project(PAGE_KIND, &payload(html, "")).unwrap().blocks
    }

    #[test]
    fn the_macro_showcase_draws_every_macro_as_its_block() {
        let blocks = drawn(include_str!("../testdata/macros.html"));
        let code: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                DocBlock::Code { language, code } => Some((language.clone(), code.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(code[0].0.as_deref(), Some("java"));
        assert!(code[0].1.contains("return 30_000L << attempt;"), "{code:?}");
        assert_eq!(code[2], (None, "no language given".into()));
        let inline_code = blocks.iter().any(|b| match b {
            DocBlock::Paragraph { text } => text
                .iter()
                .any(|i| matches!(i, Inline::Text { text, code: true, .. } if text == "just test")),
            _ => false,
        });
        assert!(inline_code, "the inline code macro stays in its sentence");

        let panels: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                DocBlock::Panel {
                    kind,
                    title,
                    blocks,
                } => Some((*kind, title.clone(), blocks.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(panels.len(), 5, "{panels:?}");
        assert_eq!(panels[0].0, PanelKind::Info);
        assert_eq!(
            panels[0].2,
            vec![DocBlock::Paragraph {
                text: vec![
                    Inline::text("An "),
                    Inline::Text {
                        text: "info".into(),
                        strong: true,
                        emphasis: false,
                        code: false
                    },
                    Inline::text(" box."),
                ]
            }],
            "the icon and its screen-reader label are not words of the box"
        );
        assert_eq!(
            panels[1],
            (
                PanelKind::Warning,
                Some("Careful".into()),
                panels[1].2.clone()
            )
        );
        assert_eq!(panels[3].0, PanelKind::Tip);
        assert_eq!(panels[4].0, PanelKind::Note);
        assert_eq!(panels[4].1.as_deref(), Some("Box with a title"));

        assert!(blocks.iter().any(|b| matches!(b, DocBlock::Embed { name, content: EmbedContent::Html { html } } if name == "html" && html.contains("knobas-raw"))));
        let includes: Vec<_> = blocks
            .iter()
            .filter(|b| matches!(b, DocBlock::Include { .. }))
            .collect();
        assert_eq!(
            includes,
            vec![
                &DocBlock::include("Included snippet", Some("Tidewater".into()), false),
                &DocBlock::include("Included snippet", Some("Tidewater".into()), false),
            ],
            "include and display are includes, filled by knobas, not the target's rendered copy"
        );
        let mention = blocks.iter().find_map(|b| match b {
            DocBlock::Paragraph { text } => {
                text.iter().find(|i| matches!(i, Inline::Mention { .. }))
            }
            _ => None,
        });
        assert_eq!(
            mention,
            Some(&Inline::Mention {
                user: Some("MaraLindqvist".into()),
                name: Some("Mara Lindqvist".into()),
                me: false
            })
        );
        let placeholders: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                DocBlock::Placeholder { name } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(placeholders, ["doesnotexist", "doesnotexist"]);
        assert!(
            !format!("{blocks:?}").contains("Unknown macro:"),
            "an unknown macro's error box is not drawn as words"
        );
    }

    #[test]
    fn the_showcase_draws_links_lists_and_marks() {
        let blocks = drawn(include_str!("../testdata/showcase.html"));
        let all = format!("{blocks:?}");
        assert!(
            !all.contains("HHeadinglevel1"),
            "the table of contents is not drawn: knobas draws its own outline"
        );
        let inlines: Vec<&Inline> = blocks
            .iter()
            .flat_map(|b| match b {
                DocBlock::List { items, .. } => items
                    .iter()
                    .flat_map(|i| &i.blocks)
                    .flat_map(|b| match b {
                        DocBlock::Paragraph { text } => text.iter().collect::<Vec<_>>(),
                        _ => Vec::new(),
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect();
        assert!(inlines.contains(&&Inline::PageLink {
            text: "SEPA payout retry design".into(),
            title: "SEPA payout retry design".into(),
            space: Some("Tidewater".into()),
            entity_id: None
        }));
        assert!(
            inlines.contains(&&Inline::Link {
                text: "Outside".into(),
                href: "http://wiki.example/xwiki/bin/view/Outside/".into()
            }),
            "a page outside the synced spaces is a link to the wiki"
        );
        assert!(
            inlines.contains(&&Inline::text(
                "Link to a page that does not exist: Not written yet"
            )),
            "a page that does not exist is words"
        );
        assert!(inlines.contains(&&Inline::Link {
            text: "the knobas logo".into(),
            href: "http://wiki.example/xwiki/bin/download/Tidewater/Showcase/Images/WebHome/knobas-logo.png?rev=1.1".into()
        }));
        assert!(
            all.contains(r#"text: "monospace", strong: false, emphasis: false, code: true"#),
            "{all}"
        );
        assert!(all.contains("Placeholder { name: \"children\" }"));
        let headings = headings(&blocks);
        assert_eq!(headings[0], "Heading level 1");
    }

    #[test]
    fn tables_keep_their_header_cells_spans_and_rich_cells() {
        let blocks = drawn(include_str!("../testdata/tables.html"));
        let tables: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                DocBlock::Table { rows } => Some(rows),
                _ => None,
            })
            .collect();
        assert_eq!(tables.len(), 4);
        assert!(tables[0][0].iter().all(|c| c.header));
        assert!(matches!(tables[2][1][1].blocks[0], DocBlock::List { .. }));
        assert_eq!(tables[3][0][0].colspan, 2);
    }

    #[test]
    fn sections_follow_the_source_and_edits_splice_it() {
        let html = include_str!("../testdata/sepa.html");
        let source = "== Backoff policy ==\n\nBase 30 s, factor 2, max 5 attempts.\n\n== Manual review queue ==\n\n//SLA: to be defined.//\n";
        let payload = payload(html, source);
        let content = project(PAGE_KIND, &payload).unwrap();
        assert_eq!(
            content.sections,
            vec![
                DocSection {
                    number: 1,
                    text: Some("Base 30 s, factor 2, max 5 attempts.".into()),
                    refusal: None
                },
                DocSection {
                    number: 2,
                    text: None,
                    refusal: Some(SectionRefusal::Formatting)
                },
            ]
        );
        let edit = edit_section(PAGE_KIND, &payload, 1, "Base 60 s.").unwrap();
        assert_eq!(edit.base_version, 3);
        assert_eq!(
            edit.body,
            "== Backoff policy ==\n\nBase 60 s.\n\n== Manual review queue ==\n\n//SLA: to be defined.//\n"
        );
        assert_eq!(
            edit_section(PAGE_KIND, &payload, 2, "x"),
            Err(EditRefused::Section(SectionRefusal::Formatting))
        );
        assert_eq!(
            edit_section(PAGE_KIND, &payload, 0, "x"),
            Err(EditRefused::Missing)
        );
        assert_eq!(
            edit_section(PAGE_KIND, &payload, 3, "x"),
            Err(EditRefused::Missing)
        );
        assert_eq!(
            edit_section("other", &payload, 1, "x"),
            Err(EditRefused::Missing)
        );
        assert!(project("other", &payload).is_none());
    }

    #[test]
    fn sections_are_all_refused_when_source_and_body_disagree() {
        let html = "<h2>Printed by a script</h2><p>x</p>";
        let content = project(PAGE_KIND, &payload(html, "Just words.")).unwrap();
        assert_eq!(content.sections.len(), 1);
        assert_eq!(
            content.sections[0].refusal,
            Some(SectionRefusal::Formatting)
        );
    }

    #[test]
    fn comments_meta_and_history_come_from_the_payload() {
        let mut p = payload("<p>x</p>", "x");
        let mut e = extra("<p>x</p>");
        e.tags = vec!["payments".into()];
        e.comments = vec![Comment {
            id: 0,
            author: Some("XWiki.PriyaNair".into()),
            date: Some(1_787_400_300_000),
            text: "{{mention reference=\"XWiki.MaraLindqvist\" style=\"FULL_NAME\"/}} can you add the **SLA**?".into(),
            reply_to: None,
        }];
        p[page::EXTRA] = serde_json::to_value(e).unwrap();
        let content = project(PAGE_KIND, &p).unwrap();
        assert_eq!(content.version, Some(3));
        assert_eq!(content.editor_name.as_deref(), Some("Mara Lindqvist"));
        assert_eq!(content.labels, ["payments"]);
        assert_eq!(content.history.len(), 2);
        assert_eq!(content.history[1].by.as_deref(), Some("knobas"));
        let comment = &content.comments[0];
        assert_eq!(comment.author.as_deref(), Some("PriyaNair"));
        assert_eq!(comment.at.as_deref(), Some("2026-08-22T12:05:00+00:00"));
        assert_eq!(
            comment.blocks,
            vec![DocBlock::Paragraph {
                text: vec![
                    Inline::Mention {
                        user: Some("MaraLindqvist".into()),
                        name: Some("Mara Lindqvist".into()),
                        me: false
                    },
                    Inline::text(" can you add the SLA?"),
                ]
            }]
        );
    }

    #[test]
    fn a_page_xwiki_could_not_render_draws_its_source() {
        let mut p = payload("", "= A =\nb");
        p[page::EXTRA]["html"] = Value::Null;
        let content = project(PAGE_KIND, &p).unwrap();
        assert!(matches!(&content.blocks[1], DocBlock::Code { code, .. } if code == "= A =\nb"));
    }
}
