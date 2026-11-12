# knobas-adapter-xwiki

An external [knobas](https://github.com/BFoerschner/knobas) adapter for
[XWiki](https://www.xwiki.org). It syncs the pages of the spaces you pick
into knobas' Docs view, with their comments, tags, authors and page tree,
and writes comments, section edits and new pages back. It is a WASM
component, built with knobas' Rust SDK against `knobas:adapter@1.5.0`, and
it runs in knobas' sandbox: no disk, no keychain, and HTTP only to the wiki
you configure.

It is also the worked example for the adapter guide in knobas,
[`docs/adapters/`](https://github.com/BFoerschner/knobas/tree/main/docs/adapters).
Read the guide for the rules; read this repository to see each rule in code.

Tested against XWiki 17.10.15 (LTS) with the XWiki Standard flavor.

## Install it into knobas

1. In knobas' *Add source* dialog, under *Install an adapter*, paste
   `https://github.com/BFoerschner/knobas-adapter-xwiki`, press *Install…*.
   knobas reads the latest release's `knobas-adapter.json`, checks the
   component and shows what it may reach (the wiki you configure, nothing
   else) before you confirm.
2. Add a source of kind *XWiki*:
   - **Base URL**: the XWiki web app, e.g. `https://wiki.example.com/xwiki`.
     A pasted page address is cut back to it.
   - **Authentication**: user and password. The password goes to the OS
     keychain. knobas attaches it as HTTP basic auth to requests for this
     wiki only; the component never sees it.
   - **Username**: your XWiki login (`MaraLindqvist`, not the display name).
   - **Spaces**: the top-level spaces to sync, e.g. `Tidewater`. Empty syncs
     every space whose pages are not hidden.
3. *Test connection* says whom XWiki served the credential as. A wrong
   password fails here as *unauthorized* rather than syncing what a guest
   may read.

XWiki locks an account behind a captcha after three failed logins in five
minutes (`XWiki.Authentication.Configuration`), and basic auth cannot answer
a captcha. If *Test connection* keeps failing after a password fix, log in
once in a browser to clear it.

## What it maps

| XWiki | knobas |
|---|---|
| a page (`Space.Sub.WebHome` or a terminal page) | a `page` entity, id `<source>:<full name>` |
| the top-level space | the page's project |
| the enclosing pages | the parent chain (Docs tree, `under:`) |
| tags | labels (`label:`) |
| comments | the page's comments, mentions included |
| `{{info}}`, `{{warning}}`, `{{error}}`, `{{success}}`, `{{box}}` | panels |
| `{{code}}` | code blocks, or code text inline |
| `{{include}}`, `{{display}}` | includes, filled by knobas from the mirror |
| `{{html}}` | an HTML embed, run in knobas' sandboxed frame |
| `{{mention}}` | a mention |
| links to synced pages | page links; other links stay links |
| `{{children}}` and other live widgets, a macro that failed | a placeholder naming the macro |
| other macros (`{{velocity}}`, …) | what they rendered to |

The body is XWiki's own rendering of the page in annotated HTML, read by
knobas' shared HTML reader with this adapter's hook for XWiki's macro and
link markers ([`adapter/src/docs.rs`](adapter/src/docs.rs)). Sections and
edits work on the page's XWiki 2.1 source
([`adapter/src/markup.rs`](adapter/src/markup.rs)). A section with a table,
code, a macro or any formatting is edited in XWiki; knobas says why.

Writes: `comment`, `update_page` (one section, against the version you
read; a page someone changed since is held as *moved*) and `create_page`
(a child page under the one you are on).

Limits worth knowing:

- XWiki keeps no record of deletions, so every incremental sync lists the
  synced spaces' pages (one request per 50) and hands over as deleted each
  page the cursor holds that the listing no longer names. The cursor holds
  every synced page's name for that, some 60 bytes a page.
- The version knobas shows is XWiki's major version. A minor edit made in
  XWiki ("minor edit" ticked) does not move it, so an edit against it is not
  caught as moved.
- Velocity output is rendered as the configured user, as XWiki renders it
  for them.

## Build

```sh
rustup show            # installs the toolchain and wasm32-wasip2 from rust-toolchain.toml
cargo test --workspace # the pure parts, on the host
scripts/build.sh       # dist/knobas_adapter_xwiki.wasm and dist/knobas-adapter.json
```

`scripts/build.sh` links the crate as a `cdylib` for `wasm32-wasip2` and
writes the manifest with the artifact's SHA-256. The SDK comes from the
knobas repository by git tag (`Cargo.toml`), and the tag decides the
interface version.

## Testing

`check/` is the author's gate. It loads the built component with knobas' own
WASM host, the code the app installs it with, and runs:

```sh
cargo run -p check -- static   # what install checks: manifest, hash, interface, descriptor, roles
cargo run -p check -- live     # + test_connection, a full sync, the role data check,
                               #   the docs contract and the whole contract battery
cargo run -p check -- writes   # + create a page, edit a section, a stale edit, a comment
CHECK_CURSOR='…' cargo run -p check -- sync   # one run from a cursor: what it hands over
```

`live` and `writes` run against a real XWiki: `XWIKI_URL` (default
`http://127.0.0.1:8099/xwiki`), `XWIKI_USER` (`knobas`), `XWIKI_PASSWORD`
(`knobas-dev`), `XWIKI_SPACES` (`Tidewater`). The battery's *unauthorized*
case logs in as a user that does not exist, never with a wrong password for
yours, so it cannot trip the lockout. `writes` leaves one page,
*Check page <time>*, under `XWIKI_PARENT` (default the showcase).

[`testenv/`](testenv/README.md) runs that XWiki in Docker with Postgres,
installs the Standard flavor unattended and seeds it with the Tidewater
fixture and a showcase of every construct the reader maps.
`testenv/samples/` holds real responses for reference, and
`adapter/testdata/` the rendered pages the unit tests read.

## Release

Bump `version` in `Cargo.toml`, add a `CHANGELOG.md` entry, tag `v<version>`
and push the tag. The release workflow builds the component, runs the static
checks and publishes `knobas-adapter.json` and the `.wasm` as the release's
assets. knobas never updates on its own: a reader sees that a newer release
exists and updates when they choose.

## Layout

| Path | What |
|---|---|
| `adapter/src/lib.rs` | the `Adapter` implementation: config, connection check, writes, export |
| `adapter/src/descriptor.rs` | the descriptor: kind, declared paths, wiki role, writes, config schema |
| `adapter/src/http.rs` | `wasi:http` requests and the fault classes |
| `adapter/src/api.rs` | XWiki's REST API and renderer |
| `adapter/src/sync.rs` | full and incremental syncs, the cursor, a page's payload |
| `adapter/src/page.rs` | the payload's shape |
| `adapter/src/annotated.rs` | annotated HTML markers into elements for the reader's hook |
| `adapter/src/docs.rs` | the docs projection and the section edit |
| `adapter/src/markup.rs` | XWiki 2.1 sections, plain text and escaping |
| `check/` | the gate, on knobas' WASM host |
| `scripts/build.sh` | the component and its manifest |
| `testenv/` | the XWiki the gate runs against |
