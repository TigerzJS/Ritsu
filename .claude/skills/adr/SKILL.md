---
name: adr
description: Legt einen neuen Architecture Decision Record in docs/adr/ an (Status Proposed) und trägt ihn in den Index ein. Verwenden, wenn eine Entscheidung mehrere Crates betrifft, eine öffentliche Schnittstelle festlegt oder schwer umkehrbar ist.
---

# Neuen ADR anlegen

1. Nächste freie Nummer bestimmen: `ls docs/adr/ | grep -E '^[0-9]{4}-' | sort | tail -1`.
2. `docs/adr/0000-template.md` nach `docs/adr/NNNN-<kebab-titel>.md` kopieren. Der Titel ist als Entscheidung
   formuliert („X wird mit Y gemacht“), nicht als Frage.
3. Ausfüllen: Kontext mit Quellen, mindestens zwei echte Optionen, Entscheidung, Konsequenzen, **Überprüfung**
   (welche Messung/welcher Spike die Entscheidung kippen würde). Status immer **Proposed**.
4. Zeile in der Tabelle in `docs/adr/README.md` ergänzen.
5. Ersetzt der ADR einen anderen: im neuen ADR `Supersedes ADR-XXXX` angeben. Im alten ADR **nur** die Statuszeile auf
   `Superseded by ADR-NNNN` ändern, sonst nichts.
6. `python3 tools/check_md_links.py` ausführen.
7. Dem Menschen den ADR zur Freigabe vorlegen. Agenten setzen nie `Accepted`.
