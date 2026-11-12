//! An external knobas adapter for XWiki: the pages of the configured spaces
//! in knobas' Docs view, with comments, section edits and new pages written
//! back.
//!
//! Built as a WASM component for `wasm32-wasip2` against
//! `knobas:adapter@1.5.0` with knobas' Rust SDK, and installed into knobas
//! from a GitHub release (`knobas-adapter.json` beside the `.wasm`).
//!
//! | Module | What it does |
//! |---|---|
//! | [`descriptor`] | what the adapter declares: kinds, paths, the wiki role, writes, config |
//! | [`http`] | the one way out, `wasi:http` |
//! | [`api`] | XWiki's REST API and renderer |
//! | [`sync`] | full and incremental syncs, a page's payload |
//! | [`page`] | what a page's payload holds |
//! | [`annotated`] | XWiki's annotated HTML made readable by the shared reader |
//! | [`docs`] | the docs projection and the section edit |
//! | [`markup`] | XWiki 2.1 sections and escaping, for the writes |

pub mod annotated;
pub mod api;
pub mod descriptor;
pub mod docs;
pub mod http;
pub mod markup;
pub mod page;
pub mod sync;

use knobas_adapter_sdk::spi::docs::{DocContent, EditRefused, SectionEdit};
use knobas_adapter_sdk::spi::view::SourceView;
use knobas_adapter_sdk::{
    Adapter, ConnectionInfo, Cursor, Instance, RefreshAnswer, Sink, SourceDescriptor, SourceError,
    WriteOp, WriteReceipt,
};
use serde::Deserialize;

/// The instance's `config`, as the descriptor's `config_schema` describes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub username: Option<String>,
    pub wiki: String,
    pub spaces: Vec<String>,
    pub page_size: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            username: None,
            wiki: "xwiki".into(),
            spaces: Vec::new(),
            page_size: 50,
        }
    }
}

/// One configured XWiki, for one call. The host makes a new one for every
/// call, so nothing is kept between calls but the cursor.
pub struct XWiki {
    id: String,
    name: String,
    /// The config, or why it cannot be read: every call that needs it then
    /// fails with that reason. The descriptor never does, because knobas
    /// reads it with an empty config at install.
    config: Result<Config, String>,
    client: api::Client,
}

impl XWiki {
    fn config(&self) -> Result<&Config, SourceError> {
        self.config
            .as_ref()
            .map_err(|reason| SourceError::protocol(reason.clone()))
    }

    fn run(&self) -> Result<sync::Run<'_>, SourceError> {
        Ok(sync::Run {
            client: &self.client,
            source_id: &self.id,
            spaces: &self.config()?.spaces,
        })
    }

    /// The page an entity id of this source names, by full name.
    fn full_name<'a>(&self, entity: &'a str) -> Result<&'a str, SourceError> {
        entity
            .strip_prefix(self.id.as_str())
            .and_then(|rest| rest.strip_prefix(':'))
            .filter(|key| !key.is_empty())
            .ok_or_else(|| SourceError::protocol(format!("{entity} is not a page of {}", self.id)))
    }

    /// Who the credential is. XWiki serves a request whose basic auth it
    /// refuses as the guest, with `200` where guests may read, so a wrong
    /// password would read as a working one: this asks whom XWiki served
    /// ([`api::Client::current_user`]) and holds it to the configured login.
    fn who_am_i(&self) -> Result<String, SourceError> {
        let username = self
            .config()?
            .username
            .clone()
            .filter(|u| !u.trim().is_empty())
            .ok_or_else(SourceError::unauthorized)?;
        let current = self.client.current_user()?;
        if current.as_deref().map(page::account).as_deref() == Some(username.as_str()) {
            Ok(username)
        } else {
            Err(SourceError::Unauthorized { status: Some(401) })
        }
    }

    fn update_page(
        &self,
        entity: &str,
        base_version: i64,
        body: &str,
    ) -> Result<WriteReceipt, SourceError> {
        let full_name = self.full_name(entity)?;
        let current = self
            .client
            .page(full_name)?
            .ok_or_else(|| SourceError::Protocol {
                status: Some(404),
                message: format!("{full_name} is no longer in the wiki"),
            })?;
        let version = current
            .get("majorVersion")
            .and_then(serde_json::Value::as_i64);
        if version != Some(base_version) {
            return Err(SourceError::Moved {
                base: version.map_or_else(String::new, |v| v.to_string()),
                current: current
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            });
        }
        let title = current
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        self.client.put_page(full_name, title, body)?;
        Ok(WriteReceipt::none())
    }

    fn create_page(
        &self,
        parent: &str,
        title: &str,
        body: &str,
    ) -> Result<WriteReceipt, SourceError> {
        let parent = self.full_name(parent)?;
        let (mut spaces, name) = page::split(parent);
        if name != "WebHome" {
            spaces.push(name);
        }
        let leaf: String = title
            .chars()
            .filter(|c| !matches!(c, '.' | ':' | '\\' | '/' | '?' | '#' | '%'))
            .collect::<String>()
            .trim()
            .to_owned();
        if leaf.is_empty() {
            return Err(SourceError::protocol(
                "a new page needs a title with a letter in it",
            ));
        }
        spaces.push(leaf);
        let full_name = format!(
            "{}.WebHome",
            spaces
                .iter()
                .map(|s| s.replace('\\', "\\\\").replace('.', "\\."))
                .collect::<Vec<_>>()
                .join(".")
        );
        if self.client.page(&full_name)?.is_some() {
            return Err(SourceError::protocol(format!(
                "a page {full_name} already exists; pick another title"
            )));
        }
        self.client.put_page(&full_name, title, body)?;
        Ok(WriteReceipt::id(full_name))
    }

    fn comment(&self, entity: &str, body: &str) -> Result<WriteReceipt, SourceError> {
        let full_name = self.full_name(entity)?;
        let text = body
            .lines()
            .map(markup::escape)
            .collect::<Vec<_>>()
            .join("\n");
        let id = self.client.post_comment(full_name, &text)?;
        Ok(id.map_or_else(WriteReceipt::none, |id| WriteReceipt::id(id.to_string())))
    }
}

impl Adapter for XWiki {
    fn new(instance: Instance) -> Self {
        let config = if instance.config.is_null() {
            Ok(Config::default())
        } else {
            serde_json::from_value::<Config>(instance.config.clone())
                .map_err(|e| format!("the xwiki adapter cannot read its config: {e}"))
        };
        let (wiki, page_size) = config
            .as_ref()
            .map_or(("xwiki".to_owned(), 50), |c| (c.wiki.clone(), c.page_size));
        Self {
            client: api::Client::new(&instance.base_url, &wiki, page_size),
            id: instance.id,
            name: instance.display_name,
            config,
        }
    }

    fn descriptor(&self) -> SourceDescriptor {
        descriptor::descriptor(&self.id, &self.name)
    }

    fn test_connection(&self) -> Result<ConnectionInfo, SourceError> {
        let version = self.client.version()?;
        let account = self.who_am_i()?;
        Ok(ConnectionInfo {
            account: Some(account),
            server_version: Some(format!("XWiki {version}")),
            secret_expires_at: None,
            detail: None,
            discovered: Default::default(),
        })
    }

    fn sync(&self, cursor: Option<Cursor>, sink: &mut Sink) -> Result<Cursor, SourceError> {
        // A guest sees what a guest may: refuse rather than mirror a
        // partial wiki under a password that no longer works.
        self.who_am_i()?;
        self.run()?.sync(cursor.as_deref(), sink)
    }

    fn write(&self, op: WriteOp) -> Result<WriteReceipt, SourceError> {
        self.who_am_i()?;
        match op {
            WriteOp::UpdatePage {
                entity,
                base_version,
                body,
            } => self.update_page(&entity, base_version, &body),
            WriteOp::CreatePage {
                parent,
                title,
                body,
                ..
            } => self.create_page(&parent, &title, &body),
            WriteOp::Comment { entity, body } => self.comment(&entity, &body),
            other => Err(SourceError::protocol(format!(
                "the xwiki adapter does not perform {}",
                other.identifier()
            ))),
        }
    }

    fn reachable_transitions(&self, entity: &str) -> Result<Vec<String>, SourceError> {
        let _ = entity;
        Err(SourceError::protocol(format!(
            "the {} adapter has no workflow: source {:?} offers no transitions",
            descriptor::KIND,
            self.id
        )))
    }

    /// Pages are drawn by knobas' Docs view from the projection, not by a
    /// source view.
    fn view(&self, entity: &str, section: Option<&str>) -> Result<Option<SourceView>, SourceError> {
        let _ = (entity, section);
        Ok(None)
    }

    fn refresh(&self, entity: &str) -> Result<RefreshAnswer, SourceError> {
        self.who_am_i()?;
        let full_name = self.full_name(entity)?;
        let run = self.run()?;
        let index = run.index()?;
        Ok(match run.item(&index, full_name)? {
            Some(item) => RefreshAnswer::Item(item),
            None => RefreshAnswer::Gone,
        })
    }

    fn project(kind: &str, payload: &serde_json::Value) -> Option<DocContent> {
        docs::project(kind, payload)
    }

    fn edit_section(
        kind: &str,
        payload: &serde_json::Value,
        number: u32,
        text: &str,
    ) -> Result<SectionEdit, EditRefused> {
        docs::edit_section(kind, payload, number, text)
    }

    fn body_from_text(text: &str) -> String {
        markup::from_text(text)
    }
}

knobas_adapter_sdk::export!(XWiki);
