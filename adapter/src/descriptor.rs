//! What this adapter says about itself: the descriptor knobas generates the
//! Add-source form from, checks at install, and reads every item with.

use knobas_adapter_sdk::{SourceDescriptor, WebSignInPages};

use crate::page;
use knobas_adapter_sdk::spi::docs::PAGE_KIND;
use knobas_adapter_sdk::spi::role::{RoleClaim, WIKI};
use knobas_adapter_sdk::spi::{
    AuthMethod, Capability, KindInfo, KindPaths, ParentPath, PayloadPath,
};

/// The adapter kind: the manifest's `adapter_kind`, and the namespace knobas
/// keeps this adapter's install under. Every source of it has an id of its
/// own, which is the namespace of the entities it emits.
pub const KIND: &str = "xwiki";

/// This adapter's version: the manifest's `adapter_version`, which install
/// holds the descriptor to.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The descriptor of the instance `id` named `name`.
#[must_use]
pub fn descriptor(id: &str, name: &str) -> SourceDescriptor {
    SourceDescriptor {
        id: id.to_owned(),
        adapter_kind: KIND.to_owned(),
        // knobas reads the template (what the Add-source list shows) from
        // an instance whose id and display name are both the adapter kind.
        name: if name.is_empty() || name == KIND {
            "XWiki".to_owned()
        } else {
            name.to_owned()
        },
        // `Write` because `write_ops` is not empty (the two must agree);
        // `Refresh` because `refresh` reads a page by id after a write lands.
        capabilities: vec![Capability::Write, Capability::Refresh],
        adapter_version: VERSION.to_owned(),
        // XWiki's REST API takes HTTP basic auth. The host attaches it from
        // the config's `username` and the secret, to requests for the
        // instance only. Tokens need an XWiki extension, so none here.
        auth_methods: vec![AuthMethod::UserPassword],
        accepts_account: false,
        min_sync_interval_secs: knobas_adapter_sdk::spi::DEFAULT_MIN_SYNC_INTERVAL_SECS,
        // The wiki role's three optional writes, in the order the action
        // bar draws them.
        write_ops: vec![
            "comment".to_owned(),
            "update_page".to_owned(),
            "create_page".to_owned(),
        ],
        entity_kinds: vec![KindInfo {
            id: PAGE_KIND.to_owned(),
            label: "Page".to_owned(),
            plural: "Pages".to_owned(),
            monogram: "PG".to_owned(),
            // A full sync lists every page of the configured spaces, so a
            // page that stopped coming back was deleted and may be swept.
            full_sync_exhaustive: true,
            refreshes: true,
        }],
        config_schema: config_schema(),
        payload_paths: vec![payload_paths()],
        roles: vec![RoleClaim::new(WIKI, ["doc_content"])],
        source_actions: Vec::new(),
        clone_from: Vec::new(),
        url_forms: Vec::new(),
        // knobas' in-app site fills a saved web sign-in into XWiki's own
        // login page by itself (`knobas:adapter@1.7.0`): its path after the
        // base URL, which ends at the wiki's context (`…/xwiki`) or at the
        // host. Its "Remember me" (`j_rememberme`) is ticked by the fill, so
        // XWiki keeps the session past a restart. A refused password lands on
        // `loginsubmit`, declared too, so knobas tells "did not take the
        // saved sign-in" from a sign-in that worked. The sign-in never reaches
        // the component: knobas keeps it and types it into its own web view.
        web_sign_in: Some(WebSignInPages::at(&[
            "/bin/login/XWiki/XWikiLogin",
            "/bin/loginsubmit/XWiki/XWikiLogin",
        ])),
        code_pages: None,
    }
}

/// Where a page keeps what knobas reads: all of it in the adapter's own
/// object of the payload ([`crate::page::EXTRA`]), which the sync writes on
/// every page, so the declarations always land. Not XWiki's own `space`
/// field, which is a string: a path into a string reads as the source
/// saying nothing, which no check can tell from a page with no space.
///
/// * The **project** is the page's top-level space (`Tidewater` for
///   `Tidewater.Engineering.Runbook.WebHome`): XWiki's nearest thing to a
///   Confluence space, and what an include or a page link names when it
///   names a space.
/// * The **parent** is the chain of pages above it, outermost first, each
///   answering to its title, which is what the Docs tree and `under:` read.
#[must_use]
pub fn payload_paths() -> KindPaths {
    KindPaths {
        kind: PAGE_KIND.to_owned(),
        project_key: vec![PayloadPath::of([page::EXTRA, "space", "key"])],
        project_name: vec![PayloadPath::of([page::EXTRA, "space", "name"])],
        parent: vec![ParentPath {
            kind: PAGE_KIND.to_owned(),
            at: PayloadPath::of([page::EXTRA, "ancestors"]),
            names: vec![PayloadPath::of(["title"])],
        }],
        labels: vec![knobas_adapter_sdk::spi::ListPath {
            at: PayloadPath::of([page::EXTRA, "tags"]),
            entry: PayloadPath::of(Vec::<String>::new()),
        }],
        ..KindPaths::default()
    }
}

/// The JSON Schema the Add-source form is generated from. No secret goes
/// here: the password lives in the OS keychain.
#[must_use]
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["username"],
        "properties": {
            "username": {
                "type": "string",
                "title": "Username",
                "description": "Your XWiki login. The password goes in the credential field; knobas sends both as HTTP basic auth to this wiki only."
            },
            "wiki": {
                "type": "string", "default": "xwiki", "title": "Wiki",
                "description": "The wiki to read, for a farm. `xwiki` is the main wiki."
            },
            "spaces": {
                "type": "array", "default": [], "title": "Spaces",
                "description": "Top-level spaces to sync, e.g. Tidewater. Leave empty to sync every space whose pages are not hidden.",
                "items": { "type": "string", "minLength": 1 }
            },
            "page_size": {
                "type": "integer", "minimum": 1, "maximum": 100, "default": 50,
                "title": "Pages per request"
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// knobas fills a saved sign-in into XWiki's own login page by itself
    /// (`knobas:adapter@1.7.0`): the path after a base URL that ends at the
    /// wiki's context, in the rules knobas holds a declaration to.
    #[test]
    fn the_login_page_is_declared() {
        let pages = descriptor(KIND, KIND).web_sign_in.expect("declared");
        assert_eq!(
            pages.paths,
            [
                "/bin/login/XWiki/XWikiLogin",
                "/bin/loginsubmit/XWiki/XWikiLogin"
            ]
        );
        assert!(!pages.any_page);
        knobas_adapter_sdk::spi::web_sign_in::check(&pages).expect("knobas accepts it");
    }

    /// The checks install runs on the descriptor alone: every claimed role
    /// is covered by the kinds, paths and reads declared.
    #[test]
    fn the_wiki_claim_is_covered() {
        let d = descriptor("tidewiki", "Tidewater wiki");
        knobas_adapter_sdk::spi::role::check(&d).expect("the wiki claim is covered");
    }

    /// Every declared path lands on a page payload the sync writes. The
    /// payload's top-level `space` is XWiki's string, so a path through it
    /// would read as nothing on every page.
    #[test]
    fn the_declared_paths_land_on_a_page_payload() {
        fn at<'a>(v: &'a serde_json::Value, p: &PayloadPath) -> Option<&'a serde_json::Value> {
            p.0.iter().try_fold(v, |v, seg| v.get(seg))
        }
        let extra = crate::page::Extra {
            space: crate::page::Space {
                key: "T".into(),
                name: "Team".into(),
            },
            ancestors: vec![crate::page::Ancestor {
                full_name: "T.WebHome".into(),
                title: "Team".into(),
            }],
            tags: vec!["x".into()],
            ..Default::default()
        };
        let payload = serde_json::json!({ "space": "T.Sub", page::EXTRA: extra });
        let paths = payload_paths();
        assert_eq!(
            at(&payload, &paths.project_key[0]).and_then(|v| v.as_str()),
            Some("T")
        );
        assert_eq!(
            at(&payload, &paths.project_name[0]).and_then(|v| v.as_str()),
            Some("Team")
        );
        assert!(at(&payload, &paths.parent[0].at).is_some_and(|v| v.is_array()));
        assert!(at(&payload, &paths.labels[0].at).is_some_and(|v| v.is_array()));
    }
}
