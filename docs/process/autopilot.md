# Autopilot: durchgehende Arbeit an der Roadmap

Ritsu wird **ohne Pause** vorangetrieben. Es gibt keine Runden. Der Orchestrator (Opus 5.5) betreibt eine
**Pipeline mit parallelen Spuren**: Sobald ein Spezialist fertig ist, wird sein Ergebnis geprüft und integriert, und
dieselbe Spur bekommt **sofort** das nächste Arbeitspaket. Ergänzt [CLAUDE.md](../../CLAUDE.md) und den Skill `/wp`.

## Ziele

| Ebene | Ziel | Wo gepflegt |
|---|---|---|
| Nordstern | M8 „Reif“: macOS 27 läuft benutzbar mit Metal auf dem G5 MF | [ARCHITECTURE.md](../ARCHITECTURE.md) |
| Aktuelles Ziel | Der nächste offene Meilenstein (zurzeit **M1 „Erster Befehl“**). Ist er erreicht, rückt das Ziel automatisch weiter | `status/meta.json` → `goal` |
| Spur | Immer genau ein laufendes Work Package pro Spur | §1 |

## 1. Spuren

Bis zu **4 Spuren laufen gleichzeitig**, jede mit eigenem Verzeichnis-Scope, damit sich Agenten nicht in die Quere
kommen:

| Spur | Scope | Typische Agenten |
|---|---|---|
| CPU | `crates/ritsu-dbt`, `crates/ritsu-mem` | dbt-engineer (Opus) |
| GPU | `crates/ritsu-gpu-*` | gpu-engineer (Opus) |
| Shader | `compiler/` | shader-compiler-engineer (Opus) |
| Plattform/Qualität | `crates/ritsu-platform`, `-virtio`, `-vmm`, `host-os/`, `fuzz/`, `tests/`, `bench/`, `.github/` | platform-engineer, test-engineer (Sonnet) |

`architect` und `reverse-engineer` schreiben nur in `docs/` und können zusätzlich zu den 4 Spuren laufen. Braucht ein
WP mehrere Scopes, wird es in der Spur mit dem größten Anteil geführt, und die anderen Spuren meiden diese
Verzeichnisse, bis es integriert ist.

## 2. Der Takt einer Spur

```
Spur frei ─► nächstes WP wählen (§3) ─► Brief ─► Spezialist (Hintergrund)
   ▲                                                    │ fertig
   └── Status + Live-Übersicht ◄─ Commit + Push ◄─ make check ◄─ code-reviewer ◄─ Bericht prüfen
```

1. **Bericht kommt an** → Ergebnis selbst prüfen (`make check`), `code-reviewer` beauftragen.
2. **Review-Befunde** *blocker*/*major* gehen per SendMessage an denselben Spezialisten zurück (Kontext bleibt
   erhalten). Erst wenn sie behoben sind, wird integriert.
3. **Integrieren:** nur die Pfade dieser Spur stagen, Conventional Commit, `git push` auf den Arbeitsbranch.
4. **Status pflegen** (§4).
5. **Sofort** das nächste WP für diese Spur wählen und starten. Eine Spur bleibt nur leer, wenn es für ihren Scope
   kein auswählbares WP gibt. Dann übernimmt sie Qualitätsarbeit (Tests, Fuzzing, Doku) oder einen Spike.

Der Orchestrator wartet nie aktiv. Er reagiert auf Fertigmeldungen der Agenten. Zwischen zwei Meldungen beantwortet er
den Menschen oder bereitet Briefe vor.

## 3. Priorität bei der WP-Wahl

1. CI/`make check` rot → reparieren (blockiert alle Integrationen).
2. Offene Review-Befunde *blocker*/*major* im Scope der Spur.
3. WPs auf dem **kritischen Pfad des aktuellen Meilensteins**, deren Abhängigkeiten erledigt sind.
4. ★-Spikes, die einen späteren Meilenstein entsperren (Risiko früh abbauen).
5. WPs späterer Meilensteine im Scope der Spur, die keine offene Entscheidung brauchen.
6. Qualität: Fuzz-Targets, Tests, Benchmarks, Doku.

**Nie auswählen:** WPs, die an einem ADR mit Status `Proposed` hängen, an einer offenen Frage `O-*`, an
Apple-Dateien, einem Mac (O-2) oder echter Hardware. Solche WPs bekommen Status `blocked` mit Notiz, und die Frage
landet beim Menschen.

## 4. Status pflegen

Die Wahrheit über den Projektstand liegt versioniert in `status/`:

* `status/map-*.json`: eine Datei pro Übersichtskarte. Jeder Eintrag hat `status` ∈ `open | wip | done | blocked`.
  Ein WP wird beim Start auf `wip` gesetzt, nach DoD auf `done`.
* `status/meta.json`: Ziel, Meilensteine, `runs` (Protokoll der letzten **20** integrierten WPs), `updatedAt`.

Nach jeder Integration:

1. Einträge aktualisieren, neue WPs aus Berichten eintragen, `runs` ergänzen.
2. `make status` grün, mit dem Commit pushen.
3. Live-Übersicht aktualisieren: `ArtifactData` `batch` auf `https://claude.ai/artifact/AEJLPVPPQZCncXeXG5Hr8y`, je
   geänderte Datei ein `set` auf `maps/<id>` bzw. `meta/summary` mit `file_path` und dem zuletzt gelesenen
   `if_version`.

## 5. Wächter

Eine stündliche Routine prüft, ob die Pipeline läuft. Laufen Spezialisten, tut sie nichts. Steht die Pipeline (z. B.
nach einem Neustart der Sitzung), startet sie die Spuren nach §2 neu. Der Wächter ist kein Takt, sondern nur ein
Sicherheitsnetz.

## 6. Wann die Pipeline anhält

* Eine Entscheidung des Menschen blockiert **alle** auswählbaren WPs → genau diese Frage stellen, Spuren ruhen lassen.
* Zwei Fehlversuche am selben WP → `architect` holen, Ansatz in `docs/research/` festhalten.
* Der Mensch sagt Stopp → laufende Agenten fertig werden lassen, nichts Neues starten, Wächter deaktivieren.

## 7. Was der Autopilot nie tut

* `Accepted`-ADRs editieren, ADRs auf `Accepted` setzen, offene Fragen `O-*` selbst entscheiden.
* Apple-Binärdateien beschaffen oder einchecken, Clean-Room-Regeln aufweichen.
* Tests deaktivieren oder überspringen, um grün zu werden.
* PRs ohne Auftrag öffnen, auf andere Branches pushen, Force-Push.
* Berechtigungen, `.claude/settings.json` oder diese Datei ändern, ohne dass der Mensch es verlangt.
