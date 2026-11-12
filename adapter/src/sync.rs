//! The sync: pages of the configured spaces into the mirror.
//!
//! **A full sync** (no cursor) lists every page of the configured top-level
//! spaces, reads each one and hands it to the sink. It is exhaustive: a page
//! that stops coming back was deleted (or moved out of scope), and knobas
//! sweeps it after the run.
//!
//! **An incremental sync** reads XWiki's modification list since the
//! cursor's instant and reads only the pages it names. A page the list names
//! that no longer exists is handed over as deleted. A deletion the list does
//! not name (XWiki records none) waits for the next full sync. With nothing
//! modified in scope it hands nothing and answers the cursor it was given,
//! unchanged, even when pages outside the synced spaces changed: those are
//! listed again by the next run, which costs a listing and nothing more.
//!
//! **The cursor** is `{"v":1,"since":<ms>}`: the newest instant of XWiki's
//! modification list the sync has read past. A full sync takes it before
//! its walk; an incremental one moves it to the newest modification in
//! scope it read.
//!
//! Every page costs four requests (the record, its comments, its tags and
//! its rendering), plus one per user whose name it shows, once per run.
//! The host paces requests to the instance (five a second by default).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use knobas_adapter_sdk::html::{self, Node};
use knobas_adapter_sdk::spi::EntityRef;
use knobas_adapter_sdk::spi::docs::PAGE_KIND;
use knobas_adapter_sdk::{Cursor, Sink, SourceError, SyncItem};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::Client;
use crate::page::{self, Ancestor, Extra, Space, Target};

/// The cursor's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub v: u32,
    pub since: i64,
}

impl Position {
    const VERSION: u32 = 1;

    /// Read a cursor; one this adapter did not write is a full sync.
    #[must_use]
    pub fn read(cursor: &str) -> Option<Self> {
        serde_json::from_str::<Self>(cursor)
            .ok()
            .filter(|p| p.v == Self::VERSION)
    }

    #[must_use]
    pub fn write(since: i64) -> Cursor {
        serde_json::to_string(&Self {
            v: Self::VERSION,
            since,
        })
        .expect("a position serializes")
    }
}

/// What one run knows about the synced pages: every page's title, by full
/// name, and the display names it has looked up.
pub struct Index {
    pub titles: BTreeMap<String, String>,
    names: RefCell<HashMap<String, Option<String>>>,
}

/// One source's sync, over its client and configured spaces.
pub struct Run<'a> {
    pub client: &'a Client,
    pub source_id: &'a str,
    /// Top-level spaces; empty is every space.
    pub spaces: &'a [String],
}

impl Run<'_> {
    /// Whether this source syncs the page `full_name`.
    #[must_use]
    pub fn in_scope(&self, full_name: &str) -> bool {
        self.spaces.is_empty() || self.spaces.contains(&page::top_space(full_name))
    }

    /// Every synced page's title.
    ///
    /// # Errors
    ///
    /// A listing's fault.
    pub fn index(&self) -> Result<Index, SourceError> {
        let mut titles = BTreeMap::new();
        let listings: Vec<Option<&str>> = if self.spaces.is_empty() {
            vec![None]
        } else {
            self.spaces.iter().map(|s| Some(s.as_str())).collect()
        };
        for space in listings {
            for summary in self.client.pages(space)? {
                // `?space=Tide` also lists `Tidewater`: XWiki matches a prefix.
                if self.in_scope(&summary.full_name) {
                    let title = summary
                        .title
                        .filter(|t| !t.trim().is_empty())
                        .unwrap_or_else(|| fallback_title(&summary.full_name));
                    titles.insert(summary.full_name, title);
                }
            }
        }
        Ok(Index {
            titles,
            names: RefCell::new(HashMap::new()),
        })
    }

    /// A user's display name, looked up once per run.
    fn name(&self, index: &Index, reference: &str) -> Result<Option<String>, SourceError> {
        let key = page::local(reference).to_owned();
        if let Some(known) = index.names.borrow().get(&key) {
            return Ok(known.clone());
        }
        let found = self.client.user_name(&key)?;
        index.names.borrow_mut().insert(key, found.clone());
        Ok(found)
    }

    /// One page as the sync emits it, `None` when the wiki has no such page,
    /// or hides it, or it is outside the configured spaces.
    ///
    /// # Errors
    ///
    /// A request's fault.
    pub fn item(&self, index: &Index, full_name: &str) -> Result<Option<SyncItem>, SourceError> {
        if !self.in_scope(full_name) {
            return Ok(None);
        }
        let Some(mut record) = self.client.page(full_name)? else {
            return Ok(None);
        };
        if record.get("hidden").and_then(Value::as_bool) == Some(true) {
            return Ok(None);
        }
        let comments = self.client.comments(full_name)?;
        let tags = self.client.tags(full_name)?;
        let rendered = self.client.render(full_name)?;

        let str_of = |key: &str| record.get(key).and_then(Value::as_str).map(str::to_owned);
        let title = str_of("title")
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| fallback_title(full_name));
        let modifier = str_of("modifier");
        let modified = record.get("modified").and_then(Value::as_i64);
        let web_url = str_of("xwikiAbsoluteUrl");
        let creator = str_of("creator");

        let mut users: BTreeSet<String> = BTreeSet::new();
        users.extend(modifier.iter().cloned());
        users.extend(creator);
        users.extend(comments.iter().filter_map(|c| c.author.clone()));
        for comment in &comments {
            users.extend(mentions_in_source(&comment.text));
        }
        let mut pages = BTreeMap::new();
        if let Some(rendered) = &rendered {
            let nodes = html::parse(&crate::annotated::elements(rendered));
            references(&nodes, &mut |candidate| {
                if let Some(title) = index.titles.get(candidate) {
                    pages.insert(
                        candidate.to_owned(),
                        Target {
                            title: title.clone(),
                            space: page::top_space(candidate),
                        },
                    );
                }
            });
            mentions(&nodes, &mut users);
        }
        let mut people = BTreeMap::new();
        for user in users {
            if let Some(name) = self.name(index, &user)? {
                people.insert(page::local(&user).to_owned(), name);
            }
        }

        let (spaces, name) = page::split(full_name);
        let home = |depth: usize| format!("{}.WebHome", join(&spaces[..depth]));
        // The pages above: the home page of every enclosing space, outermost
        // first, but the page itself when it is one of them.
        let own_depth = if name == "WebHome" {
            spaces.len()
        } else {
            spaces.len() + 1
        };
        let ancestors = (1..own_depth)
            .map(|depth| {
                let full = home(depth.min(spaces.len()));
                let title = index
                    .titles
                    .get(&full)
                    .cloned()
                    .unwrap_or_else(|| spaces[depth - 1].clone());
                Ancestor {
                    full_name: full,
                    title,
                }
            })
            .collect();
        let top = spaces.first().cloned().unwrap_or_else(|| name.clone());
        let space = Space {
            name: index
                .titles
                .get(&format!("{}.WebHome", escape(&top)))
                .cloned()
                .unwrap_or_else(|| top.clone()),
            key: top,
        };
        let extra = Extra {
            space,
            ancestors,
            tags,
            comments,
            people,
            html: rendered,
            origin: self.client.origin(),
            pages,
        };

        if let Some(object) = record.as_object_mut() {
            object.remove("links");
            object.insert(
                page::EXTRA.to_owned(),
                serde_json::to_value(extra).expect("the payload's extra serializes"),
            );
        }
        Ok(Some(SyncItem {
            entity: EntityRef::new(self.source_id, full_name),
            kind: PAGE_KIND.to_owned(),
            title,
            // knobas builds a page's search text from the projection and
            // ignores what an external adapter sends here (ADR-0040).
            body_text: String::new(),
            author: modifier.as_deref().map(page::account),
            updated_at: modified.and_then(chrono::DateTime::from_timestamp_millis),
            web_url,
            payload: record,
            deleted: false,
        }))
    }

    /// A page the wiki no longer has, as a tombstone.
    #[must_use]
    pub fn gone(&self, full_name: &str) -> SyncItem {
        SyncItem {
            entity: EntityRef::new(self.source_id, full_name),
            kind: PAGE_KIND.to_owned(),
            title: fallback_title(full_name),
            body_text: String::new(),
            author: None,
            updated_at: None,
            payload: Value::Null,
            web_url: None,
            deleted: true,
        }
    }

    /// The sync from `cursor`.
    ///
    /// # Errors
    ///
    /// A request's fault, or the sink's, as it came.
    pub fn sync(&self, cursor: Option<&str>, sink: &mut Sink) -> Result<Cursor, SourceError> {
        let position = cursor.and_then(Position::read);
        let (names, mut since): (Vec<String>, i64) = match position {
            None => {
                // The position is taken before the walk, so an edit made
                // while it runs is read again by the next sync, not lost.
                let since = self.client.newest_modification()?.unwrap_or(0);
                let index = self.index()?;
                let names = index.titles.keys().cloned().collect::<Vec<_>>();
                return self.walk(&index, &names, since, sink);
            }
            Some(position) => {
                let changed = self.client.modifications(position.since)?;
                let mut names = Vec::new();
                let mut since = position.since;
                for change in changed {
                    let full = page::local(&change.page_id).to_owned();
                    // Only a change in scope moves the position: a sync
                    // that hands nothing answers the cursor it was given,
                    // even when pages elsewhere in the wiki changed.
                    if self.in_scope(&full) {
                        since = since.max(change.modified);
                        if !names.contains(&full) {
                            names.push(full);
                        }
                    }
                }
                (names, since)
            }
        };
        if names.is_empty() {
            sink.walked(0, Some(0), "pages");
            return Ok(cursor.unwrap_or_default().to_owned());
        }
        let index = self.index()?;
        since = since.max(position.map_or(0, |p| p.since));
        self.walk(&index, &names, since, sink)
    }

    /// Read and hand over every page of `names`, and answer the position
    /// `since`: an instant of the modification list, never a page record's
    /// `modified` (see [`Client::newest_modification`]).
    fn walk(
        &self,
        index: &Index,
        names: &[String],
        since: i64,
        sink: &mut Sink,
    ) -> Result<Cursor, SourceError> {
        let total = names.len() as u64;
        for (done, full_name) in names.iter().enumerate() {
            sink.walked(done as u64, Some(total), "pages");
            match self.item(index, full_name)? {
                Some(item) => sink.item(item)?,
                None => sink.item(self.gone(full_name))?,
            }
        }
        sink.walked(total, Some(total), "pages");
        Ok(Position::write(since))
    }
}

/// A page's title when XWiki gives none: its name, or its space's for a
/// space home page.
#[must_use]
pub fn fallback_title(full_name: &str) -> String {
    let (spaces, name) = page::split(full_name);
    if name == "WebHome" {
        spaces.last().cloned().unwrap_or(name)
    } else {
        name
    }
}

/// One reference segment with `.` and `\` escaped.
fn escape(segment: &str) -> String {
    segment.replace('\\', "\\\\").replace('.', "\\.")
}

fn join(segments: &[String]) -> String {
    segments
        .iter()
        .map(|s| escape(s))
        .collect::<Vec<_>>()
        .join(".")
}

/// The page a rendered `/bin/view/…` address names, by full name.
#[must_use]
pub fn from_href(href: &str) -> Option<String> {
    let at = href.find("/bin/view/")?;
    let path = &href[at + "/bin/view/".len()..];
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut segments: Vec<String> = path.split('/').map(decode).collect();
    let name = match segments.last().map(String::as_str) {
        Some("") => {
            segments.pop();
            "WebHome".to_owned()
        }
        _ => segments.pop()?,
    };
    if segments.is_empty() {
        return None;
    }
    Some(format!("{}.{}", join(&segments), escape(&name)))
}

/// Percent-decoding of one path segment.
fn decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = segment.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every page reference a rendered page names, in the spellings the
/// projection looks them up by ([`candidates`]).
fn references(nodes: &[Node], found: &mut dyn FnMut(&str)) {
    for node in nodes {
        if matches!(node.name(), Some("xw:link" | "xw:macro")) {
            for candidate in candidates(node) {
                found(&candidate);
            }
        }
        references(node.children(), found);
    }
}

/// The full names a link or an include may name, most certain first: what
/// XWiki resolved it to (the rendered address, an include's source), then
/// the reference as written.
#[must_use]
pub fn candidates(node: &Node) -> Vec<String> {
    let attr = |n: &Node, key: &str| n.attr(key).map(html::decode_entities);
    let mut out = Vec::new();
    match node.name() {
        Some("xw:link") => {
            if let Some(href) = first(node.children(), "a").and_then(|a| attr(a, "href"))
                && let Some(full) = from_href(&href)
            {
                out.push(full);
            }
            if let Some(reference) = attr(node, "ref") {
                out.push(page::local(&reference).to_owned());
            }
        }
        Some("xw:macro") => {
            let name = attr(node, "name").unwrap_or_default();
            if name != "include" && name != "display" {
                return out;
            }
            if let Some(source) = with_attr(node.children(), "data-xwiki-source") {
                out.push(page::local(&source).to_owned());
            }
            let params = attr(node, "params").unwrap_or_default();
            if let Some(reference) = crate::annotated::param(&params, "reference")
                .or_else(|| crate::annotated::param(&params, "page"))
            {
                out.push(page::local(&reference).to_owned());
            }
        }
        _ => {}
    }
    let with_home: Vec<String> = out.iter().map(|r| format!("{r}.WebHome")).collect();
    out.extend(with_home);
    out
}

fn first<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Node> {
    nodes.iter().find_map(|n| {
        if n.name() == Some(name) {
            Some(n)
        } else {
            first(n.children(), name)
        }
    })
}

fn with_attr(nodes: &[Node], key: &str) -> Option<String> {
    nodes.iter().find_map(|n| {
        n.attr(key)
            .map(html::decode_entities)
            .or_else(|| with_attr(n.children(), key))
    })
}

/// The users the rendered page mentions.
fn mentions(nodes: &[Node], users: &mut BTreeSet<String>) {
    for node in nodes {
        if node.name() == Some("xw:macro")
            && node.attr("name") == Some("mention")
            && let Some(params) = node.attr("params").map(html::decode_entities)
            && let Some(reference) = crate::annotated::param(&params, "reference")
        {
            users.insert(reference);
        }
        mentions(node.children(), users);
    }
}

/// The users an XWiki 2.1 source mentions.
fn mentions_in_source(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = source;
    while let Some(at) = rest.find("{{mention") {
        let tail = &rest[at + "{{mention".len()..];
        let end = tail.find("}}").unwrap_or(tail.len());
        if let Some(reference) =
            crate::annotated::param(tail[..end].trim_end_matches('/'), "reference")
        {
            out.push(reference);
        }
        rest = &tail[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rendered_address_names_its_page() {
        assert_eq!(
            from_href("/xwiki/bin/view/Tidewater/Engineering/OnCallHandbook/").as_deref(),
            Some("Tidewater.Engineering.OnCallHandbook.WebHome")
        );
        assert_eq!(
            from_href("http://w/xwiki/bin/view/Tidewater/Showcase/Tables/#HRow").as_deref(),
            Some("Tidewater.Showcase.Tables.WebHome")
        );
        assert_eq!(
            from_href(
                "/xwiki/bin/view/Tidewater/Engineering/StandupProtocols/DailyProtocol20260821"
            )
            .as_deref(),
            Some("Tidewater.Engineering.StandupProtocols.DailyProtocol20260821")
        );
        assert_eq!(
            from_href("/xwiki/bin/view/A%2EB/").as_deref(),
            Some(r"A\.B.WebHome")
        );
        assert_eq!(from_href("/xwiki/bin/download/A/B/x.png"), None);
    }

    #[test]
    fn the_showcase_names_the_pages_it_links_and_includes() {
        let html = include_str!("../testdata/showcase.html");
        let nodes = html::parse(&crate::annotated::elements(html));
        let mut found = Vec::new();
        references(&nodes, &mut |c| found.push(c.to_owned()));
        assert!(found.contains(&"Tidewater.Engineering.SEPAPayoutRetryDesign.WebHome".to_owned()));
        assert!(found.contains(&"Outside.WebHome".to_owned()));
        let macros = include_str!("../testdata/macros.html");
        let nodes = html::parse(&crate::annotated::elements(macros));
        let mut found = Vec::new();
        references(&nodes, &mut |c| found.push(c.to_owned()));
        assert!(found.contains(&"Tidewater.Showcase.IncludedSnippet.WebHome".to_owned()));
        let mut users = BTreeSet::new();
        mentions(&nodes, &mut users);
        assert_eq!(
            users.into_iter().collect::<Vec<_>>(),
            ["XWiki.MaraLindqvist"]
        );
    }

    #[test]
    fn a_cursor_round_trips_and_a_foreign_one_is_a_full_sync() {
        let c = Position::write(1_787_400_300_000);
        assert_eq!(c, r#"{"v":1,"since":1787400300000}"#);
        assert_eq!(Position::read(&c).map(|p| p.since), Some(1_787_400_300_000));
        assert_eq!(Position::read("5"), None);
        assert_eq!(Position::read(r#"{"v":2,"since":1}"#), None);
    }

    #[test]
    fn a_comment_names_its_mentions() {
        assert_eq!(
            mentions_in_source(
                "{{mention reference=\"XWiki.MaraLindqvist\" style=\"FULL_NAME\"/}} hi"
            ),
            ["XWiki.MaraLindqvist"]
        );
    }
}
