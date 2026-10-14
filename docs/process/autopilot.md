# Autopilot: Dauerauftrag für die Agents

Ritsu wird von einem Orchestrator (Opus 5.5) in **Runden** vorangetrieben. Eine Routine startet stündlich eine Runde.
Jede Runde folgt exakt diesem Protokoll. Es ergänzt [CLAUDE.md](../../CLAUDE.md) und den Skill `/wp`.

## Ziele

| Ebene | Ziel | Wo gepflegt |
|---|---|---|
| Nordstern | M8 „Reif“: macOS 27 läuft benutzbar mit Metal auf dem G5 MF | [ARCHITECTURE.md](../ARCHITECTURE.md) |
| Aktuelles Ziel | Der nächste offene Meilenstein (zurzeit **M1 „Erster Befehl“**) | `status/meta.json` → `goal` |
| Runde | 1–4 Work Packages, die das aktuelle Ziel am meisten voranbringen | Auswahl nach §2 |

## 1. Ablauf einer Runde

1. **Synchronisieren.** `git fetch` und `git status`. Den Arbeitsbranch auf den neuesten Stand bringen. Ist der
   Arbeitsbaum nicht sauber, zuerst klären, was das ist (abgebrochene Runde?). Nichts Fremdes verwerfen.
2. **Lage prüfen.** `make check` muss grün sein. Ist es rot, ist die Reparatur das einzige WP dieser Runde.
3. **Offene Befunde zuerst.** Unerledigte *blocker*/*major*-Befunde aus Reviews (siehe `status/meta.json` → `runs`
   und `docs/OPEN-QUESTIONS.md`) haben Vorrang vor neuen WPs.
4. **WPs auswählen** nach §2. Höchstens **4 Spezialisten parallel**, nur mit disjunkten Verzeichnissen. Sonst
   `isolation: "worktree"`.
5. **Ausführen** nach Skill `/wp` (Brief → Spezialist → Bericht → eigene Prüfung → `code-reviewer` → Fixes).
6. **Integrieren.** Conventional Commits, `make check` grün, `git push` auf den Arbeitsbranch. Nie auf andere
   Branches, nie Force-Push, keinen PR ohne Auftrag des Menschen.
7. **Status pflegen** (§3) und die Seite aktualisieren.
8. **Kurzbericht** an den Menschen, höchstens 5 Zeilen: was erledigt ist, was blockiert ist, welche Entscheidung
   gebraucht wird.

## 2. Priorität

In dieser Reihenfolge:

1. CI/`make check` rot → reparieren.
2. Offene Review-Befunde *blocker*/*major*.
3. WPs auf dem **kritischen Pfad des aktuellen Meilensteins**, deren Abhängigkeiten erledigt sind.
4. ★-Spikes, die einen späteren Meilenstein entsperren (früh Risiko abbauen).
5. Parallelisierbare WPs anderer Subsysteme, die keine offene Entscheidung brauchen (DBT und GPU laufen unabhängig).
6. Qualität: Fuzz-Targets, Tests, Doku.

**Nie auswählen:** WPs, die an einem ADR mit Status `Proposed` hängen, an einer offenen Frage `O-*`, an Apple-Dateien
oder einem Mac (O-2), oder an echter Hardware. Solche WPs bekommen Status `blocked` mit Notiz.

## 3. Status pflegen

Die Wahrheit über den Projektstand liegt versioniert in `status/`:

* `status/map-*.json`: eine Datei pro Übersichtskarte (Arbeitspakete, AArch64-Befehle, Metal-API, Shader,
  Plattform). Jeder Eintrag hat `status` ∈ `open | wip | done | blocked`.
* `status/meta.json`: Ziel, Meilensteine, `runs` (die **letzten 20** Runden, ältere löschen), `updatedAt`.

Nach jeder Runde:

1. Einträge aktualisieren (`done` nur nach DoD), neue WPs aus Berichten eintragen.
2. `python3 tools/check_status.py` muss grün sein (läuft auch in `make check`).
3. Mit dem Commit pushen.
4. Die Live-Seite aktualisieren: `ArtifactData` mit `action: "batch"` auf
   `https://claude.ai/artifact/AEJLPVPPQZCncXeXG5Hr8y`, je Datei ein `set` auf `maps/<id>` bzw. `meta/summary` mit
   `file_path` und dem zuletzt gelesenen `if_version` (vorher `list`/`get`, um die Versionen zu kennen).

## 4. Wann der Autopilot anhält

* Eine Entscheidung des Menschen blockiert **alle** auswählbaren WPs → im Kurzbericht genau diese Frage stellen und
  die Runde ohne Code-Änderung beenden.
* Zwei Runden hintereinander ohne Fortschritt am selben WP → `architect` holen, Ansatz in `docs/research/` festhalten.
* Der Mensch sagt Stopp → Routine deaktivieren.

## 5. Was der Autopilot nie tut

* `Accepted`-ADRs editieren, ADRs auf `Accepted` setzen, offene Fragen `O-*` selbst entscheiden.
* Apple-Binärdateien beschaffen oder einchecken, Clean-Room-Regeln aufweichen.
* Tests deaktivieren oder überspringen, um grün zu werden.
* Berechtigungen, `.claude/settings.json` oder diese Datei ändern, ohne dass der Mensch es verlangt.
