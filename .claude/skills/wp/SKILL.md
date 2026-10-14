---
name: wp
description: Führt ein Work Package aus docs/ROADMAP.md durch den Orchestrierungsablauf (Brief → Subagent(s) → Review → Integration). Verwenden mit der WP-ID als Argument, z. B. "/wp WP-DBT-03".
---

# Work Package ausführen

Argument: WP-ID (z. B. `WP-DBT-03`).

1. **Prüfen:** WP in `docs/ROADMAP.md` und im zugehörigen `docs/subsystems/*.md` nachlesen. Sind alle Abhängigkeiten
   erledigt? Hängt es an einem ADR mit Status `Proposed`? Wenn ja: stoppen und melden.
2. **Brief schreiben** nach der Vorlage in `CLAUDE.md` §2.1. Akzeptanzkriterien als ausführbare Befehle.
3. **Delegieren** an den passenden Agenten (Tabelle `CLAUDE.md` §1). Unabhängige Teil-WPs parallel in **einer**
   Nachricht starten. Bei Überschneidungsgefahr `isolation: "worktree"`.
4. **Bericht prüfen:** Stimmen die gemeldeten Prüfungen? Selbst `make check` ausführen.
5. **Review:** `code-reviewer` mit dem Diff beauftragen. Blocker beheben lassen (zurück an den Spezialisten,
   per SendMessage, damit der Kontext erhalten bleibt).
6. **Integrieren:** Conventional Commit, `docs/ROADMAP.md`-Status aktualisieren, offene Fragen in
   `docs/OPEN-QUESTIONS.md` übernehmen, Folge-WPs eintragen.
