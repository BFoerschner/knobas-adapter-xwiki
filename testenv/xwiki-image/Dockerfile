# The official XWiki image plus a small entrypoint wrapper that makes the first
# start unattended (no Distribution Wizard). Built on the remote engine by
# `docker compose up --build`; a bind mount would not work because the compose
# files live on the Mac while the engine runs on knobas-wsl.
#
# xwiki:17.10.15-postgres-tomcat (17.10.x is the current LTS; tag lts-postgres-tomcat
# pointed at the same digest on 2026-11-12)
FROM xwiki@sha256:3dd6f6642f98ff31c330d513e9801aafafc22c9b5573e68951bf82d839d7256e

COPY knobas-entrypoint.sh /usr/local/bin/knobas-entrypoint.sh
RUN chmod 0755 /usr/local/bin/knobas-entrypoint.sh

ENTRYPOINT ["knobas-entrypoint.sh"]
CMD ["xwiki"]
