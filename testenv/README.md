# XWiki test environment

A real XWiki 17.10.15 (current LTS) with PostgreSQL 17 on the `knobas-wsl` box,
seeded with the Tidewater fixture and a docs-reader showcase. It is the backend
for the knobas XWiki adapter.

| | |
|---|---|
| URL (via tunnel) | `http://127.0.0.1:8099/xwiki/` (REST: `/xwiki/rest/`) |
| Admin | `knobas` / `knobas-dev` (member of `XWiki.XWikiAdminGroup`) |
| Fixture people | `MaraLindqvist`, `JonasBecker`, `PriyaNair`, `TomaszWojcik`, `LenaHoffmann` / `tidewater-dev` |
| superadmin | `superadmin` / `knobas-superadmin` (left enabled, see below) |
| Postgres | `xwiki` / `xwiki-dev`, not published |
| Compose project | `knobas-xwiki`: containers `knobas-xwiki-web`, `knobas-xwiki-db`; volumes `knobas-xwiki-data`, `knobas-xwiki-db` |

The port is bound to `127.0.0.1:8099` on the WSL box only. The credentials are
dev-grade.

## Up, seed, down

```sh
export DOCKER_HOST=ssh://knobas-wsl
docker compose -f testenv/docker-compose.yml up -d --build   # build runs on the remote engine
testenv/tunnel.sh up                                         # Mac 127.0.0.1:8099 -> WSL 127.0.0.1:8099
testenv/seed                                                 # waits for the flavor install, then seeds
testenv/seed --verify                                        # exit 1 if something is missing
testenv/record-samples                                       # optional: refresh testenv/samples/

docker compose -f testenv/docker-compose.yml stop            # keep data
docker compose -f testenv/docker-compose.yml down -v         # wipe data (next up is a first start again)
testenv/tunnel.sh down
```

`testenv/seed` is idempotent: it reads before it writes, and a second run
prints `nothing to create`. It does not update pages that already exist; to
pick up a changed body, delete the page (or `down -v`) and seed again.

## Timing and memory (measured 2026-11-12)

- First start on empty volumes: about 2 minutes from `up` to a seeded wiki.
  Tomcat starts in ~25 s, then the Distribution Wizard installs XWiki Standard
  on its own (downloads and imports ~2,000 documents). `testenv/seed` waits
  for that (it polls the flavor install job), then takes ~3 s.
- Restart on the same volumes: ~25 s until `/xwiki/rest/` answers.
- Memory: the JVM (heap capped at 1 GB) sits at 1.5 to 2.0 GB resident;
  Postgres ~80 MB. Limits: web 2560 MB, db 512 MB. 2 GB for web was too
  tight with a 1.25 GB heap.

## How the first start is unattended

`xwiki-image/` wraps the official image (pinned by digest) with
`knobas-entrypoint.sh`, which writes `xwiki.cfg` and `xwiki.properties` into
the permanent directory before the official entrypoint runs (that entrypoint
copies both over `WEB-INF` on every start):

- `distribution.job.interactive=false` and
  `distribution.defaultUI=org.xwiki.platform:xwiki-platform-distribution-flavor-mainwiki/17.10.15`:
  the Distribution Wizard installs the Standard flavor without a browser.
  No first admin user is created by it.
- `xwiki.superadminpassword=knobas-superadmin`: the seed needs one account
  with admin rights before `knobas` exists.
- `openoffice.autoStart=false`: no LibreOffice process.
- `CONTEXT_PATH=xwiki` (compose env) serves XWiki under `/xwiki/` like a stock
  install; the image default is the root context.

To change them on an existing volume, delete
`/usr/local/xwiki/data/xwiki.cfg` / `xwiki.properties` in the
`knobas-xwiki-data` volume and recreate the container.

superadmin stays enabled: the seed bootstraps with it on a fresh volume, and it
is the recovery login if rights get broken. It is reachable only through
the loopback port.

## Wiki settings the seed applies (as superadmin)

- **Guests may not view** (`XWikiPreferences.authenticate_view=1`), like a
  company wiki. Wrong or missing credentials get `401` on REST page resources
  and a `302` to `/xwiki/bin/login/...` on `bin/` URLs.
- **No login-failure lockout**
  (`XWiki.Authentication.Configuration`, `isAuthenticationSecurityEnabled=0`).
  The XWiki default is 3 failed logins in 300 s, then a captcha. Basic auth
  cannot answer a captcha, so the account then gets served as guest even with
  the right password until the window passes. Adapter tests try wrong
  passwords on purpose, so it is off here. A production XWiki has it on: an
  adapter that retries a wrong password locks the account out.

## What the seed creates

Users (`XWiki.<Name>`, `XWiki.XWikiUsers` object, member of `XWikiAllGroup`,
which XWiki does automatically): `knobas` (also `XWikiAdminGroup`) and the
5 fixture people from `knobas/fixtures/tidewater/work.json`, with role as
"about" and `<username>@tidewater.example`.

Pages, all xwiki/2.1, each written by its fixture author (so `creator` and
`author` are real users):

```
Tidewater.WebHome                                        Tidewater                       priya
Tidewater.Engineering.WebHome                            Engineering                     priya
  .SEPAPayoutRetryDesign.WebHome                         SEPA payout retry design        mara   comment by priya (fixture date, mentions Mara), tags payments/sepa/design
  .LedgerReconciliationRunbook.WebHome                   Ledger reconciliation runbook   tomasz
  .OnCallHandbook.WebHome                                On-call handbook                tomasz tags on-call/runbook
  .PaymentsArchitectureOverview.WebHome                  Payments architecture overview  priya
  .StandupProtocols.WebHome                              Standup protocols               mara
  .StandupProtocols.DailyProtocol20260821                Daily protocol 2026-08-21       mara   terminal page (not a WebHome)
Tidewater.Showcase.WebHome                               Docs reader showcase            knobas toc, headings 1-6, inline styles, links (internal, child, missing, other space, external, bare, mailto, attachment), nested lists, definition list, quotes, verbatim, footnote, children macro; 2 comments (one a reply)
  .IncludedSnippet.WebHome                               Included snippet                knobas target of include and display
  .Tables.WebHome                                        Tables                          knobas header row/column, list and paragraphs in cells, colspan, styled cell
  .Macros.WebHome                                        Macros                          knobas code (java, sql, none, inline), info/warning(title)/error/success, box, html, include, display, mention, velocity, unknown macro
  .Images.WebHome                                        Images                          knobas knobas-logo.png attached; plain, sized, by reference, linked image
  .Changelog.WebHome                                     Changelog (several versions)    mara -> jonas -> priya (versions 1.1-3.1 with version comments, 4.1 is the tag edit), tags changelog/showcase
Outside.WebHome, Outside.ProductRoadmap.WebHome          a second space, not synced      lena
```

The fixture has a body only for the SEPA page; the others carry short bodies
written for this testenv.

Not possible on this XWiki: there is no checkbox / task-list syntax in
xwiki/2.1 and no task macro in XWiki Standard 17.10 (the installed macros are
listed under "Quirks"). Page dates cannot be backdated over REST, so pages
carry seed-time dates; comment dates come from the fixture.

## Quirks found while building this

- **A wrong password is not an error by default.** On a public wiki XWiki
  serves the request as guest with `200`. Check the `XWiki-User` response
  header: it is `xwiki:XWiki.knobas` when the login worked and absent for
  guests. Here (guests denied) page resources answer `401`, but
  `GET /xwiki/rest/` and `/xwiki/rest/wikis/xwiki` still answer `200` to
  anyone.
- **CSRF**: `POST` with a form or `text/plain` body needs the
  `XWiki-Form-Token` header (any authenticated response carries it), else
  `403 Invalid or missing form token.` JSON or XML bodies need none, and `PUT`
  is not checked at all.
- **HQL/XWQL over REST is off** (`rest.allowedQueryTypes=solr` default since
  17.10.5): `400 Query type [hql] is not allowed.` Use `type=solr`.
- `GET /rest/wikis/xwiki/pages?space=X` is recursive but `space` is a
  substring match: `space=ewater` also returns Tidewater pages. Filter by
  `fullName` prefix.
- `/rest/.../spaces/A/pages` lists only pages whose space is exactly `A`; a
  nested page `A.B.WebHome` lives in space `A.B`.
- Solr results lag edits by a second or so (indexing is asynchronous). The
  Solr `date` filter takes ISO dates (`date:[2026-11-12T10:56:28Z TO *]`); a
  millisecond range is silently ignored.
- Dates are stored to the second (`modified` always ends in `000`).
- Adding a comment, a tag or an attachment makes a new page version and
  changes `modified` and `author` to whoever did it.
- REST object properties of type Date read back as Java `Date.toString()`
  (`Sat Aug 22 12:05:00 UTC 2026`); the `/comments` resource gives
  milliseconds. Writing one takes the class's format, `dd/MM/yyyy HH:mm:ss`.
- `parent` is empty on REST-created pages; the hierarchy is the reference
  (space path), also given as `hierarchy.items` on the page.
- The `children` macro renders an empty JavaScript tree placeholder
  (`<div class="xtree" data-url=...>`), not a list.
- In xwiki/2.1, `image:` followed by text in a sentence is an image
  reference. "An attached image:" rendered as a broken `<img>`.
- A user created over REST (page plus `XWiki.XWikiUsers` object) can log in
  with the `password` property as given, and is added to `XWikiAllGroup`
  automatically.
- `ssh -fN -L` with the shared `knobas-wsl` ControlMaster hands the forward
  to that master, which drops it when ControlPersist expires. `tunnel.sh`
  runs its own master on its own socket.
- Macros installed: activity async attachmentGalleryPicker attachmentSelector
  box cache chart children code comment container content context dashboard
  display displayIcon documentTree documents error example footnote gallery
  groovy html iconPicker id include info liveData mention menu notifications
  office pdftoc putFootnotes python script success tagcloud template toc
  translation tree uiextension uiextensions useravatar velocity warning
  wikimacrocontent wikimacroparameter wikis workspaces.

## Samples

`testenv/samples/` holds real responses recorded by `testenv/record-samples`;
`samples/_requests.txt` names the request and status behind each file.
