//! XWiki's annotated HTML, made readable by knobas' shared HTML reader.
//!
//! The adapter asks XWiki for a page rendered as **annotated HTML**
//! (`outputSyntax=annotatedhtml`): the HTML a browser would get, with the
//! wiki source's structure kept in comments around what each part rendered
//! to.
//!
//! ```text
//! <!--startmacro:info|-|title="Careful"|-|An **info** box.-->…<!--stopmacro-->
//! <!--startwikilink:false|-|doc|-|Tidewater.Runbook.WebHome-->…<!--stopwikilink-->
//! <!--startimage:false|-|attach|-|logo.png-->…<!--stopimage-->
//! ```
//!
//! The shared reader drops comments, so [`elements`] turns each marker pair
//! into an element the reader hands to the adapter's hook:
//!
//! * `<xw:macro name params content>` for a macro, `content` its source;
//! * `<xw:link kind ref params>` for a wiki link (`doc`, `url`, `mailto`,
//!   `attach`, …), `ref` the reference as written;
//! * `<xw:image kind ref>` for an image.
//!
//! Every value is written as an attribute, entities escaped; read it back
//! with [`knobas_adapter_sdk::html::decode_entities`]. Inside a marker XWiki
//! escapes with a backslash (`\{`, `<\div`), which is undone here.
//! Anything that is not a marker is copied through untouched.

/// Undo XWiki's escaping inside a marker: a backslash takes the character
/// after it literally.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `text` as an attribute value between double quotes.
fn attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Split a marker's body on XWiki's field separator `|-|`, honouring
/// backslash escapes, so a separator inside escaped content stays.
fn fields(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut rest = body;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("|-|") {
            out.push(std::mem::take(&mut current));
            rest = tail;
            continue;
        }
        let mut chars = rest.chars();
        let c = chars.next().expect("not empty");
        current.push(c);
        if c == '\\'
            && let Some(next) = chars.next()
        {
            current.push(next);
        }
        rest = chars.as_str();
    }
    out.push(current);
    out
}

/// One marker comment's replacement, or `None` for a comment that is not a
/// marker (which is then copied through, and the reader drops it).
fn replacement(comment: &str) -> Option<String> {
    if comment == "stopmacro" {
        return Some("</xw:macro>".into());
    }
    if comment == "stopwikilink" {
        return Some("</xw:link>".into());
    }
    if comment == "stopimage" {
        return Some("</xw:image>".into());
    }
    if let Some(body) = comment.strip_prefix("startmacro:") {
        let parts = fields(body);
        let name = unescape(parts.first().map_or("", String::as_str));
        let params = unescape(parts.get(1).map_or("", String::as_str));
        let content = parts.get(2).map(|c| unescape(c));
        let mut tag = format!(
            "<xw:macro name=\"{}\" params=\"{}\"",
            attr(&name),
            attr(&params)
        );
        if let Some(content) = content {
            tag.push_str(&format!(" content=\"{}\"", attr(&content)));
        }
        tag.push('>');
        return Some(tag);
    }
    for (prefix, element) in [("startwikilink:", "xw:link"), ("startimage:", "xw:image")] {
        if let Some(body) = comment.strip_prefix(prefix) {
            // freestanding|-|type|-|reference[|-|parameters]
            let parts = fields(body);
            let kind = unescape(parts.get(1).map_or("", String::as_str));
            let reference = unescape(parts.get(2).map_or("", String::as_str));
            let params = unescape(parts.get(3).map_or("", String::as_str));
            return Some(format!(
                "<{element} kind=\"{}\" ref=\"{}\" params=\"{}\">",
                attr(&kind),
                attr(&reference),
                attr(&params)
            ));
        }
    }
    None
}

/// Annotated HTML with every marker pair turned into an element.
#[must_use]
pub fn elements(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(open) = rest.find("<!--") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 4..];
        let Some(close) = after.find("-->") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let comment = &after[..close];
        match replacement(comment) {
            Some(element) => out.push_str(&element),
            None => out.push_str(&rest[open..open + 4 + close + 3]),
        }
        rest = &after[close + 3..];
    }
    out.push_str(rest);
    out
}

/// One `key="value"` parameter of a macro's `params`, unquoted, or `None`.
#[must_use]
pub fn param(params: &str, key: &str) -> Option<String> {
    let mut rest = params;
    while !rest.is_empty() {
        rest = rest.trim_start();
        let eq = rest.find('=')?;
        let name = rest[..eq].trim();
        let value_start = rest[eq + 1..].trim_start();
        let (value, tail) = if let Some(quoted) = value_start.strip_prefix('"') {
            // Up to the closing quote; `~"` and `\"` escape one.
            let mut value = String::new();
            let mut chars = quoted.char_indices();
            let mut end = quoted.len();
            while let Some((i, c)) = chars.next() {
                match c {
                    '~' | '\\' => {
                        if let Some((_, next)) = chars.next() {
                            value.push(next);
                        }
                    }
                    '"' => {
                        end = i + 1;
                        break;
                    }
                    other => value.push(other),
                }
            }
            (value, &quoted[end.min(quoted.len())..])
        } else {
            let end = value_start
                .find(char::is_whitespace)
                .unwrap_or(value_start.len());
            (value_start[..end].to_owned(), &value_start[end..])
        };
        if name == key {
            return Some(value);
        }
        rest = tail;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_macro_marker_becomes_an_element_with_its_source() {
        let html = r#"<p>a</p><!--startmacro:code|-|language="java"|-|class X \{ a <\< b }--><div class="box">x</div><!--stopmacro-->"#;
        assert_eq!(
            elements(html),
            r#"<p>a</p><xw:macro name="code" params="language=&quot;java&quot;" content="class X { a &lt;&lt; b }"><div class="box">x</div></xw:macro>"#
        );
    }

    #[test]
    fn a_macro_without_content_has_no_content_attribute() {
        assert_eq!(
            elements("<!--startmacro:toc|-|--><ul></ul><!--stopmacro-->"),
            r#"<xw:macro name="toc" params=""><ul></ul></xw:macro>"#
        );
    }

    #[test]
    fn links_and_images_keep_their_kind_and_reference() {
        let html = r#"<!--startwikilink:false|-|doc|-|Tidewater.Tables.WebHome|-|anchor="HRow"--><span class="wikilink"><a href="/x">t</a></span><!--stopwikilink--><!--startimage:false|-|attach|-|logo.png--><img src="/l"/><!--stopimage-->"#;
        assert_eq!(
            elements(html),
            r#"<xw:link kind="doc" ref="Tidewater.Tables.WebHome" params="anchor=&quot;HRow&quot;"><span class="wikilink"><a href="/x">t</a></span></xw:link><xw:image kind="attach" ref="logo.png" params=""><img src="/l"/></xw:image>"#
        );
    }

    #[test]
    fn an_ordinary_comment_is_kept() {
        assert_eq!(elements("a<!-- note -->b"), "a<!-- note -->b");
    }

    #[test]
    fn a_parameter_is_read_quoted_or_bare() {
        let params = r#"reference="XWiki.MaraLindqvist" style=FULL_NAME title="say \"hi\"""#;
        assert_eq!(
            param(params, "reference").as_deref(),
            Some("XWiki.MaraLindqvist")
        );
        assert_eq!(param(params, "style").as_deref(), Some("FULL_NAME"));
        assert_eq!(param(params, "title").as_deref(), Some("say \"hi\""));
        assert_eq!(param(params, "missing"), None);
    }
}
