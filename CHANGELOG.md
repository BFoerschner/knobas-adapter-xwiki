# Changelog

## Unreleased

Built on `knobas:adapter@1.7.0` (knobas' `ux30-signin` branch, not released
yet). Proven in knobas' Mac window against the test wiki: filled by itself on
the login redirect, signed in, and still signed in after a quit and start. The 0.1.0 pin to the `v3.2.9-ux7` tag is gone: that SDK has no
`web_sign_in`, so the descriptor no longer compiles against it.

- The wiki opens inside knobas signed in, as Confluence does: the descriptor
  declares XWiki's login page (`/bin/login/XWiki/XWikiLogin` after the base
  URL, and `/bin/loginsubmit/…` where a refused password lands), so knobas fills the sign-in you saved in *Settings › Sources › XWiki ›
  Site* into it by itself and ticks *Remember me*, and XWiki keeps the session
  past a restart. The sign-in stays in knobas' keychain; the component never
  sees it.
- `INTERFACE=1.6.0` or `INTERFACE=1.5.0 scripts/build.sh` still builds
  against those frozen worlds (features `v1_6`, `v1_5`), for a knobas that
  hosts no newer one. Such a build's sign-in page is dropped by the host,
  which offers *Fill sign-in* alone.

## 0.1.0

The first release, on `knobas:adapter@1.5.0` (knobas `v3.2.9-ux7`).

- Pages of the configured top-level spaces, with their parent chain, tags,
  comments, authors and the first and latest version.
- The Docs view draws XWiki's rendering of each page: panels, code, tables,
  includes, mentions, embeds, links and images; a live widget or a failed
  macro is a named placeholder.
- Full and incremental syncs; an incremental sync finds deleted pages by
  listing the synced spaces; a refresh of one page after a write.
- Writes: comments, section edits checked against the version read, and new
  child pages.
- Test connection names the XWiki user the credential is served as, so a
  wrong password is refused instead of syncing as the guest.
