//! What a page's payload holds.
//!
//! The payload is XWiki's own page record (`GET …/pages/{page}`), kept as
//! XWiki writes it minus its hypermedia `links`, plus one object of this
//! adapter's, under `knobas`: everything the projection needs that the
//! page record does not carry. A docs projection reads the payload and
//! nothing else (no network, no instance), so whatever it draws must be in
//! here when the sync writes it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The key the adapter's own object sits under in a page's payload.
pub const EXTRA: &str = "knobas";

/// The adapter's part of a page payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extra {
    /// The page's top-level space: its knobas project.
    pub space: Space,
    /// The pages above this one, outermost first, the direct parent last.
    pub ancestors: Vec<Ancestor>,
    /// The page's tags, in XWiki's order.
    pub tags: Vec<String>,
    /// The page's comments, oldest first.
    pub comments: Vec<Comment>,
    /// Display names of the users the page and its comments name, by user
    /// reference (`XWiki.MaraLindqvist` → `Mara Lindqvist`).
    pub people: BTreeMap<String, String>,
    /// The page as XWiki renders it, in annotated HTML; `None` when XWiki
    /// could not render it.
    pub html: Option<String>,
    /// `scheme://host[:port]` of the instance, to make the rendered page's
    /// relative addresses absolute.
    pub origin: String,
    /// Every page reference the rendered page names (links, includes) that
    /// is a page this source syncs, by the reference as written: what an
    /// include and a page link are drawn with. A reference not here is to a
    /// page outside the synced spaces.
    pub pages: BTreeMap<String, Target>,
}

/// A top-level space.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    /// The space's name in references (`Tidewater`).
    pub key: String,
    /// Its home page's title, or the key.
    pub name: String,
}

/// One page above another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ancestor {
    pub full_name: String,
    pub title: String,
}

/// One comment, as `…/pages/{page}/comments` answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    /// The author's user reference (`XWiki.PriyaNair`).
    pub author: Option<String>,
    /// Milliseconds since the epoch.
    pub date: Option<i64>,
    /// XWiki 2.1 source.
    pub text: String,
    pub reply_to: Option<u64>,
}

/// A page a reference resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub title: String,
    /// Its top-level space.
    pub space: String,
}

/// The adapter's part of `payload`, or the default when it has none.
#[must_use]
pub fn extra(payload: &serde_json::Value) -> Extra {
    payload
        .get(EXTRA)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// A user reference as an account name: `xwiki:XWiki.MaraLindqvist` and
/// `XWiki.MaraLindqvist` are `MaraLindqvist`, the login knobas compares
/// with the source's `username`.
#[must_use]
pub fn account(reference: &str) -> String {
    let local = reference.split_once(':').map_or(reference, |(_, r)| r);
    local.strip_prefix("XWiki.").unwrap_or(local).to_owned()
}

/// A document reference without its wiki (`xwiki:A.B` → `A.B`).
#[must_use]
pub fn local(reference: &str) -> &str {
    reference.split_once(':').map_or(reference, |(_, r)| r)
}

/// The space segments of a local document reference, honouring XWiki's
/// `\.` escape: `A.B\.C.WebHome` is `["A", "B.C"]` and `WebHome`.
#[must_use]
pub fn split(reference: &str) -> (Vec<String>, String) {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = reference.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            '.' => parts.push(std::mem::take(&mut current)),
            other => current.push(other),
        }
    }
    let name = current;
    (parts, name)
}

/// The top-level space of a local document reference.
#[must_use]
pub fn top_space(reference: &str) -> String {
    let (spaces, name) = split(reference);
    spaces.into_iter().next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_splits_into_spaces_and_a_name() {
        assert_eq!(
            split("Tidewater.Engineering.WebHome"),
            (
                vec!["Tidewater".into(), "Engineering".into()],
                "WebHome".into()
            )
        );
        assert_eq!(
            split(r"A.B\.C.D"),
            (vec!["A".into(), "B.C".into()], "D".into())
        );
        assert_eq!(top_space("Tidewater.Engineering.WebHome"), "Tidewater");
        assert_eq!(account("xwiki:XWiki.MaraLindqvist"), "MaraLindqvist");
        assert_eq!(local("xwiki:A.B"), "A.B");
    }
}
