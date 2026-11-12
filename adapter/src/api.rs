//! XWiki's REST API (`<base>/rest/…`) and its renderer (`<base>/bin/get/…`),
//! as far as this adapter reads and writes them.
//!
//! `base` is the XWiki web application's root, as the reader typed it:
//! `https://wiki.example.com/xwiki`, or the host alone for an XWiki
//! deployed at the server's root. Every call goes through [`crate::http`],
//! so the host attaches the credential and paces the requests.

use knobas_adapter_sdk::SourceError;
use serde::Deserialize;
use serde_json::Value;

use crate::http::{self, Body, Method, encode};
use crate::page;

/// One configured wiki.
pub struct Client {
    /// The web app root, no trailing slash.
    pub base: String,
    /// The wiki of a farm; `xwiki` is the main one.
    pub wiki: String,
    /// Results per listing request.
    pub page_size: u32,
}

/// A page as a listing names it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub full_name: String,
    #[serde(default)]
    pub title: Option<String>,
}

/// One modification, newest first.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Modification {
    pub page_id: String,
    pub modified: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Summaries {
    #[serde(default)]
    page_summaries: Vec<Summary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct History {
    #[serde(default)]
    history_summaries: Vec<Modification>,
}

#[derive(Deserialize)]
struct Comments {
    #[serde(default)]
    comments: Vec<RestComment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestComment {
    id: u64,
    author: Option<String>,
    date: Option<i64>,
    #[serde(default)]
    text: String,
    reply_to: Option<u64>,
}

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    tags: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

impl Client {
    /// A client for the instance at `base_url`, read leniently: a trailing
    /// slash and a pasted page address (`…/xwiki/bin/view/Main/`) are cut.
    #[must_use]
    pub fn new(base_url: &str, wiki: &str, page_size: u32) -> Self {
        let mut base = base_url.trim().trim_end_matches('/').to_owned();
        if let Some(at) = base.find("/bin/") {
            base.truncate(at);
        }
        if let Some(at) = base.find("/rest/") {
            base.truncate(at);
        }
        Self {
            base,
            wiki: wiki.to_owned(),
            page_size: page_size.clamp(1, 100),
        }
    }

    /// `scheme://host[:port]` of the instance.
    #[must_use]
    pub fn origin(&self) -> String {
        match self.base.find("://") {
            Some(at) => {
                let after = &self.base[at + 3..];
                let host_end = after.find('/').map_or(self.base.len(), |i| at + 3 + i);
                self.base[..host_end].to_owned()
            }
            None => self.base.clone(),
        }
    }

    fn rest(&self, path: &str) -> String {
        format!("{}/rest/wikis/{}{path}", self.base, encode(&self.wiki))
    }

    /// The REST path of a page: `/spaces/A/spaces/B/pages/C`.
    fn page_path(full_name: &str) -> String {
        let (spaces, name) = page::split(full_name);
        let mut path = String::new();
        for space in spaces {
            path.push_str("/spaces/");
            path.push_str(&encode(&space));
        }
        path.push_str("/pages/");
        path.push_str(&encode(&name));
        path
    }

    fn get(&self, url: &str, what: &str) -> Result<http::Response, SourceError> {
        http::check(http::send(Method::Get, url, None)?, what)
    }

    /// A GET that answers `None` for a 404.
    fn get_found(&self, url: &str, what: &str) -> Result<Option<http::Response>, SourceError> {
        let response = http::send(Method::Get, url, None)?;
        if response.status == 404 {
            return Ok(None);
        }
        http::check(response, what).map(Some)
    }

    /// The server's version, from the REST root.
    ///
    /// # Errors
    ///
    /// The request's fault, or a protocol error naming a base URL that does
    /// not answer as XWiki's REST API.
    pub fn version(&self) -> Result<String, SourceError> {
        let url = format!("{}/rest/", self.base);
        let not_xwiki = || {
            SourceError::protocol(format!(
                "{url} does not answer as XWiki's REST API: the base URL is the XWiki web app, \
                 usually ending in /xwiki"
            ))
        };
        let response = http::send(Method::Get, &url, None)?;
        if response.status == 404 {
            return Err(not_xwiki());
        }
        let root: Value = http::check(response, "the REST root")?
            .json("the REST root")
            .map_err(|_| not_xwiki())?;
        Ok(root
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned())
    }

    /// Whom XWiki serves this credential as, `None` for the guest.
    ///
    /// XWiki serves a request whose basic auth fails as the guest, with a
    /// `200`, wherever the wiki lets guests read. So no status says a
    /// password is wrong. What does is the `XWiki-User` header, which XWiki
    /// sets on every response to an authenticated request and on none
    /// other; the wiki's own REST resource is the cheapest to ask.
    ///
    /// # Errors
    ///
    /// The request's fault; a `401` or `403` is unauthorized.
    pub fn current_user(&self) -> Result<Option<String>, SourceError> {
        let url = format!("{}/rest/wikis/{}", self.base, encode(&self.wiki));
        let response = self.get(&url, "the wiki")?;
        Ok(response
            .header("xwiki-user")
            .map(str::trim)
            .filter(|user| !user.is_empty())
            .map(str::to_owned))
    }

    /// Every page whose reference starts with `space` (all pages when
    /// `space` is `None`), through every result page.
    ///
    /// # Errors
    ///
    /// The first request's fault.
    pub fn pages(&self, space: Option<&str>) -> Result<Vec<Summary>, SourceError> {
        let mut out = Vec::new();
        let mut start = 0u32;
        loop {
            let mut url = self.rest(&format!("/pages?start={start}&number={}", self.page_size));
            if let Some(space) = space {
                url.push_str("&space=");
                url.push_str(&encode(space));
            }
            let batch: Summaries = self
                .get(&url, "the page listing")?
                .json("the page listing")?;
            let n = batch.page_summaries.len();
            out.extend(batch.page_summaries);
            if n < self.page_size as usize {
                return Ok(out);
            }
            start += self.page_size;
        }
    }

    /// Every modification of the wiki since `since` (milliseconds), newest
    /// first.
    ///
    /// # Errors
    ///
    /// The first request's fault.
    pub fn modifications(&self, since: i64) -> Result<Vec<Modification>, SourceError> {
        let mut out = Vec::new();
        let mut start = 0u32;
        loop {
            let url = self.rest(&format!(
                "/modifications?date={since}&start={start}&number={}",
                self.page_size
            ));
            let batch: History = self
                .get(&url, "the modification list")?
                .json("the modification list")?;
            let n = batch.history_summaries.len();
            out.extend(
                batch
                    .history_summaries
                    .into_iter()
                    .filter(|m| m.modified > since),
            );
            if n < self.page_size as usize {
                return Ok(out);
            }
            start += self.page_size;
        }
    }

    /// The instant of the wiki's newest modification, `None` for a wiki
    /// with none: where a full sync's cursor starts. Read from the
    /// modification list, the clock an incremental sync reads, and not from
    /// a page record's `modified`, which XWiki may stamp a second earlier
    /// than the version the list names.
    ///
    /// # Errors
    ///
    /// The request's fault.
    pub fn newest_modification(&self) -> Result<Option<i64>, SourceError> {
        let url = self.rest("/modifications?start=0&number=1");
        let batch: History = self
            .get(&url, "the modification list")?
            .json("the modification list")?;
        Ok(batch.history_summaries.first().map(|m| m.modified))
    }

    /// One page's record, `None` when the wiki has no such page.
    ///
    /// # Errors
    ///
    /// The request's fault.
    pub fn page(&self, full_name: &str) -> Result<Option<Value>, SourceError> {
        let url = self.rest(&Self::page_path(full_name));
        match self.get_found(&url, "a page")? {
            Some(response) => response.json("a page").map(Some),
            None => Ok(None),
        }
    }

    /// One page's comments, oldest first.
    ///
    /// # Errors
    ///
    /// The request's fault.
    pub fn comments(&self, full_name: &str) -> Result<Vec<page::Comment>, SourceError> {
        let url = self.rest(&format!("{}/comments", Self::page_path(full_name)));
        let Some(response) = self.get_found(&url, "a page's comments")? else {
            return Ok(Vec::new());
        };
        let comments: Comments = response.json("a page's comments")?;
        Ok(comments
            .comments
            .into_iter()
            .map(|c| page::Comment {
                id: c.id,
                author: c.author,
                date: c.date,
                text: c.text,
                reply_to: c.reply_to,
            })
            .collect())
    }

    /// One page's tags.
    ///
    /// # Errors
    ///
    /// The request's fault.
    pub fn tags(&self, full_name: &str) -> Result<Vec<String>, SourceError> {
        let url = self.rest(&format!("{}/tags", Self::page_path(full_name)));
        let Some(response) = self.get_found(&url, "a page's tags")? else {
            return Ok(Vec::new());
        };
        let tags: Tags = response.json("a page's tags")?;
        Ok(tags.tags.into_iter().map(|t| t.name).collect())
    }

    /// One page rendered as annotated HTML, `None` when XWiki answers with
    /// anything but a rendering (a server error from a broken script, a
    /// page it cannot render).
    ///
    /// # Errors
    ///
    /// A transport fault, or a refused credential.
    pub fn render(&self, full_name: &str) -> Result<Option<String>, SourceError> {
        let (spaces, name) = page::split(full_name);
        let mut url = format!("{}/bin/get", self.base);
        for space in spaces {
            url.push('/');
            url.push_str(&encode(&space));
        }
        url.push('/');
        url.push_str(&encode(&name));
        url.push_str("?xpage=plain&outputSyntax=annotatedhtml&outputSyntaxVersion=5.0");
        let response = http::send(Method::Get, &url, None)?;
        match response.status {
            200..=299 => Ok(Some(response.text())),
            401 | 403 => Err(SourceError::Unauthorized {
                status: Some(response.status),
            }),
            _ => Ok(None),
        }
    }

    /// A user's display name, `None` when the profile has none.
    ///
    /// # Errors
    ///
    /// The request's fault.
    pub fn user_name(&self, reference: &str) -> Result<Option<String>, SourceError> {
        let url = self.rest(&format!(
            "{}/objects/XWiki.XWikiUsers/0",
            Self::page_path(page::local(reference))
        ));
        let Some(response) = self.get_found(&url, "a user profile")? else {
            return Ok(None);
        };
        let object: Value = response.json("a user profile")?;
        let property = |name: &str| {
            object
                .get("properties")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
                .and_then(|p| p.get("value").and_then(Value::as_str))
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        let name = [property("first_name"), property("last_name")]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        Ok((!name.is_empty()).then_some(name))
    }

    /// Write a page's title and source, creating it when it does not exist.
    ///
    /// # Errors
    ///
    /// The request's fault. A write is sent once: a timeout after it left is
    /// the transport's to classify.
    pub fn put_page(&self, full_name: &str, title: &str, content: &str) -> Result<(), SourceError> {
        let url = self.rest(&Self::page_path(full_name));
        // JSON, not a form: XWiki asks a form write for its CSRF token, and
        // a JSON one for none (basic auth is no cookie a page could ride).
        let body = serde_json::json!({
            "title": title,
            "syntax": "xwiki/2.1",
            "content": content,
            "comment": "Edited in knobas",
        })
        .to_string();
        let response = http::send(
            Method::Put,
            &url,
            Some(Body {
                content_type: "application/json",
                bytes: body.as_bytes(),
            }),
        )?;
        http::check(response, "writing a page").map(|_| ())
    }

    /// Add a comment to a page; the answer is the new comment's id.
    ///
    /// # Errors
    ///
    /// The request's fault.
    pub fn post_comment(&self, full_name: &str, text: &str) -> Result<Option<u64>, SourceError> {
        let url = self.rest(&format!("{}/comments", Self::page_path(full_name)));
        let body = serde_json::json!({ "text": text }).to_string();
        let response = http::send(
            Method::Post,
            &url,
            Some(Body {
                content_type: "application/json",
                bytes: body.as_bytes(),
            }),
        )?;
        let response = http::check(response, "adding a comment")?;
        Ok(serde_json::from_slice::<Value>(&response.body)
            .ok()
            .and_then(|c| c.get("id").and_then(Value::as_u64)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pasted_address_is_cut_to_the_web_app() {
        let c = Client::new("http://wiki.example/xwiki/bin/view/Main/", "xwiki", 50);
        assert_eq!(c.base, "http://wiki.example/xwiki");
        assert_eq!(c.origin(), "http://wiki.example");
        let c = Client::new("https://wiki.example:8443/", "xwiki", 500);
        assert_eq!(c.base, "https://wiki.example:8443");
        assert_eq!(c.origin(), "https://wiki.example:8443");
        assert_eq!(c.page_size, 100);
    }

    #[test]
    fn a_nested_page_has_one_spaces_segment_per_space() {
        assert_eq!(
            Client::page_path("Tidewater.Engineering.WebHome"),
            "/spaces/Tidewater/spaces/Engineering/pages/WebHome"
        );
        assert_eq!(
            Client::page_path(r"A\.B.Page Name"),
            "/spaces/A.B/pages/Page%20Name"
        );
    }
}
