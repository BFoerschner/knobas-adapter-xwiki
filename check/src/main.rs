//! The adapter author's local gate. knobas has no `knobas adapter check`
//! command, so this runs the same checks with knobas' own code: the WASM
//! host loads the component exactly as the app does, and the checks are the
//! ones the app and knobas' own adapters are held to.
//!
//! ```text
//! cargo run -p check -- static [dist/knobas-adapter.json]
//! cargo run -p check -- live   [dist/knobas-adapter.json]
//! ```
//!
//! **static** is what install runs (ADR-0028, *The checks*): the manifest is
//! well formed, its interface version is hosted, the artifact's hash is the
//! manifest's, the component is a knobas adapter of that version, its
//! descriptor is well formed and agrees with the manifest, its role claims
//! are covered, and a wiki claim comes with the docs projection.
//!
//! **live** also runs, against a real instance:
//!
//! 1. `test_connection`, and a full sync, printed;
//! 2. the **data check** of every claimed role (what knobas runs on a
//!    source's first sync: the declared paths resolve on real records);
//! 3. the **docs contract** (`knobas_wasm_host::docs_contract`): every page
//!    is drawn, sections numbered, edits agree with the projection;
//! 4. the **contract battery** (`knobas_source::contract::battery`), every
//!    clause, with the two faults made real: a login the wiki does not have
//!    for *unauthorized*, a closed port for *unreachable*. Not a wrong
//!    password for the real login: XWiki locks an account behind a captcha
//!    after three failed logins, and basic auth cannot answer a captcha. The
//!    battery never performs a write the adapter declares, so it is safe on
//!    a live wiki.
//!
//! The instance comes from the environment: `XWIKI_URL` (default
//! `http://127.0.0.1:8099/xwiki`), `XWIKI_USER` (`knobas`), `XWIKI_PASSWORD`
//! (`knobas-dev`) and `XWIKI_SPACES` (comma-separated, default `Tidewater`).

use std::path::{Path, PathBuf};

use knobas_source::contract::{self, Fault, VecSink};
use knobas_source::instance::SourceInstance;
use knobas_source::{AuthMethod, Source};
use knobas_wasm_host::manifest::{self, Checked, Manifest};

fn usage() -> ! {
    eprintln!("usage: check <static|live> [path/to/knobas-adapter.json]");
    std::process::exit(2);
}

fn load(path: &Path) -> Checked {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", path.display())));
    let manifest = Manifest::parse(&text).unwrap_or_else(|e| fail(&format!("refused: {e}")));
    let artifact_path = path
        .parent()
        .unwrap_or(Path::new("."))
        .join(&manifest.artifact.file);
    let artifact = std::fs::read(&artifact_path)
        .unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", artifact_path.display())));
    manifest::check(manifest, artifact).unwrap_or_else(|e| fail(&format!("refused: {e}")))
}

fn fail(message: &str) -> ! {
    eprintln!("FAIL {message}");
    std::process::exit(1);
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

/// The instance the live checks run against, with `fault` made real.
fn instance(fault: Fault) -> SourceInstance {
    let spaces: Vec<String> = env("XWIKI_SPACES", "Tidewater")
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let base_url = match fault {
        // Nothing listens on the discard port.
        Fault::Unreachable => "http://127.0.0.1:9".to_owned(),
        _ => env("XWIKI_URL", "http://127.0.0.1:8099/xwiki"),
    };
    let secret = env("XWIKI_PASSWORD", "knobas-dev");
    let username = match fault {
        Fault::Unauthorized => "knobas-check-no-such-user".to_owned(),
        _ => env("XWIKI_USER", "knobas"),
    };
    SourceInstance {
        id: "tidewiki".to_owned(),
        kind: "xwiki".to_owned(),
        display_name: "Tidewater wiki".to_owned(),
        base_url,
        auth: Some(AuthMethod::UserPassword),
        secret: Some(secret),
        account: None,
        config: serde_json::json!({
            "username": username,
            "spaces": spaces,
        }),
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| usage());
    let path = args
        .next()
        .map_or_else(|| PathBuf::from("dist/knobas-adapter.json"), PathBuf::from);

    let checked = load(&path);
    let template = checked.adapter.template();
    println!(
        "ok   static checks: {} {} on knobas:adapter@{}, kinds {:?}, roles {:?}, writes {:?}",
        template.adapter_kind,
        template.adapter_version,
        checked.manifest.interface_version,
        template
            .entity_kinds
            .iter()
            .map(|k| &k.id)
            .collect::<Vec<_>>(),
        template.roles.iter().map(|r| &r.role).collect::<Vec<_>>(),
        template.write_ops,
    );
    match mode.as_str() {
        "static" => {}
        "live" => live(checked).await,
        "writes" => writes(checked).await,
        _ => usage(),
    }
}

/// The writes, end to end on the live wiki: what the battery leaves to the
/// adapter's own suite, since it never performs a declared write. Makes a
/// page under `XWIKI_PARENT` (default the showcase) titled with the time,
/// so each run leaves one page behind.
async fn writes(checked: Checked) {
    use knobas_source::{RefreshAnswer, SourceError, WriteOp};
    let adapter = checked.adapter;
    let source = adapter
        .build(instance(Fault::None))
        .unwrap_or_else(|e| fail(&format!("the source does not build: {e}")));
    let parent = format!(
        "tidewiki:{}",
        env("XWIKI_PARENT", "Tidewater.Showcase.WebHome")
    );
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let title = format!("Check page {stamp}");
    let body = adapter
        .body_from_text("## First\nOne.\n\n## Second\n- a\n- b")
        .unwrap_or_else(|| fail("body_from_text trapped"));
    let receipt = source
        .write(WriteOp::CreatePage {
            parent,
            space: "Tidewater".into(),
            title: title.clone(),
            body,
        })
        .await
        .unwrap_or_else(|e| fail(&format!("create_page: {e}")));
    let key = receipt
        .remote_id
        .unwrap_or_else(|| fail("create_page answered no page id"));
    let entity = format!("tidewiki:{key}");
    println!("ok   create_page: {entity}");

    let read = |entity: String| {
        let source = &source;
        async move {
            match source.refresh(&entity).await {
                Ok(RefreshAnswer::Item(item)) => item,
                other => fail(&format!("refresh of {entity}: {other:?}")),
            }
        }
    };
    let item = read(entity.clone()).await;
    let content = adapter
        .project("page", &item.payload)
        .unwrap_or_else(|| fail("the new page is drawn as no page"));
    let editable: Vec<_> = content.sections.iter().map(|s| s.text.clone()).collect();
    if editable != [Some("One.".to_owned()), Some("- a\n- b".to_owned())] {
        fail(&format!("the new page's sections are {editable:?}"));
    }
    println!(
        "ok   refresh: {} sections, both editable",
        content.sections.len()
    );

    let edit = adapter
        .edit_section("page", &item.payload, 1, "One, edited in knobas.")
        .unwrap_or_else(|| fail("edit_section trapped"))
        .unwrap_or_else(|e| fail(&format!("edit_section refused: {e:?}")));
    source
        .write(WriteOp::UpdatePage {
            entity: entity.clone(),
            base_version: edit.base_version,
            body: edit.body.clone(),
        })
        .await
        .unwrap_or_else(|e| fail(&format!("update_page: {e}")));
    println!("ok   update_page against version {}", edit.base_version);

    match source
        .write(WriteOp::UpdatePage {
            entity: entity.clone(),
            base_version: edit.base_version,
            body: edit.body,
        })
        .await
    {
        Err(SourceError::Moved { base, .. }) => {
            println!("ok   a stale update_page is refused as moved (the wiki holds {base})");
        }
        other => fail(&format!("a stale update_page answered {other:?}")),
    }

    let comment = source
        .write(WriteOp::Comment {
            entity: entity.clone(),
            body: "Checked from knobas: **stars** stay stars.".into(),
        })
        .await
        .unwrap_or_else(|e| fail(&format!("comment: {e}")));
    println!("ok   comment: id {:?}", comment.remote_id);

    let item = read(entity.clone()).await;
    let content = adapter
        .project("page", &item.payload)
        .unwrap_or_else(|| fail("the edited page is drawn as no page"));
    if content.sections[0].text.as_deref() != Some("One, edited in knobas.") {
        fail(&format!("section 1 reads {:?}", content.sections[0].text));
    }
    let said = format!("{:?}", content.comments);
    if content.comments.len() != 1 || !said.contains("**stars** stay stars") {
        fail(&format!("the comments read {said}"));
    }
    println!("ok   read back: the edit and the comment are on the page ({title})");
}

async fn live(checked: Checked) {
    let adapter = checked.adapter;
    let source = adapter
        .build(instance(Fault::None))
        .unwrap_or_else(|e| fail(&format!("the source does not build: {e}")));

    let info = source
        .test_connection()
        .await
        .unwrap_or_else(|e| fail(&format!("test_connection: {e}")));
    println!(
        "ok   test_connection: account {:?}, server {:?}",
        info.account, info.server_version
    );

    let mut sink = VecSink(Vec::new());
    let cursor = source
        .sync(None, &mut sink)
        .await
        .unwrap_or_else(|e| fail(&format!("full sync: {e}")));
    let items = sink.0;
    println!("ok   full sync: {} items, cursor {cursor}", items.len());
    for item in &items {
        println!("       {} {:?}", item.entity, item.title);
        if std::env::var_os("CHECK_PATHS").is_some() {
            let d = adapter.template();
            for paths in &d.payload_paths {
                for list in &paths.labels {
                    let mut at = &item.payload;
                    let mut found = true;
                    for seg in &list.at.0 {
                        match at.get(seg) {
                            Some(v) => at = v,
                            None => {
                                found = false;
                                break;
                            }
                        }
                    }
                    println!(
                        "         labels at {:?} entry {:?}: found={found} value={}",
                        list.at.0, list.entry.0, at
                    );
                }
            }
        }
        if std::env::var_os("CHECK_DUMP").is_some() {
            println!(
                "{}",
                serde_json::to_string_pretty(&item.payload).unwrap_or_default()
            );
        }
    }

    let descriptor = source.descriptor();
    let mut data = knobas_source::role::data::DataCheck::new(&descriptor);
    for item in &items {
        data.observe(&item.kind, &item.payload);
    }
    for verdict in data.finish() {
        if let Some(miss) = verdict.miss {
            fail(&format!(
                "role {} is not filled: no {} record resolves {} at {}",
                miss.role, miss.kind, miss.field, miss.path
            ));
        }
        println!(
            "ok   role data check: {} (seen {:?})",
            verdict.role, verdict.seen
        );
    }

    knobas_wasm_host::docs_contract::check(&adapter, &descriptor, &items)
        .unwrap_or_else(|breach| fail(&breach.to_string()));
    println!("ok   docs contract");

    // The battery panics on the first clause that fails; its message names
    // the clause.
    contract::battery(|fault| -> Box<dyn Source> {
        adapter
            .build(instance(fault))
            .unwrap_or_else(|e| fail(&format!("the source does not build: {e}")))
    })
    .await;
    println!("ok   contract battery");
}
