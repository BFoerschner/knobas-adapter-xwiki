# Changelog

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
