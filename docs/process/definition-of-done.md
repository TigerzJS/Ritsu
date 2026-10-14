# Definition of Done

Ein Work Package ist **erledigt**, wenn alle Punkte gelten:

1. **Akzeptanzkriterien** des Briefs nachweislich erfüllt (Befehle + Ergebnisse im Bericht).
2. **`make check` grün** (fmt, clippy `-D warnings`, alle Tests Rust + C++, Crate-Grafik, Doku-Links), vom
   Orchestrator selbst ausgeführt.
3. **Tests:** neue Logik hat Tests gegen unabhängige Erwartungen. DBT: Differenztests gegen Referenz-Hardware
   bzw. `llvm-objdump` (Pflicht ab M1 mit WP-DBT-04; in M0 genügen handgeprüfte Kodierungen und Orakel-Tests).
   Gast-Parser: Fuzz-Target (ab M1). GPU: Test auf lavapipe, sobald verfügbar.
4. **Review** durch `code-reviewer` ohne offene *blocker*/*major*-Befunde.
5. **Doku:** `docs/subsystems/*.md` aktuell. rustdoc bzw. Header-Kommentare für jede öffentliche Schnittstelle.
6. **Keine stillen Lücken:** Nicht Umgesetztes liefert `Error::Unimplemented` oder eine Gast-Exception und ist als
   Folge-WP eingetragen.
7. **Clean Room** eingehalten (Checkliste in [clean-room.md](clean-room.md)).
8. **Roadmap und Status** aktualisiert (`docs/ROADMAP.md`, `status/*.json`, Live-Übersicht nach
   [autopilot.md](autopilot.md) §3), offene Fragen in [OPEN-QUESTIONS.md](../OPEN-QUESTIONS.md) übertragen.
9. Commit nach Conventional Commits, ein Thema pro Commit.
