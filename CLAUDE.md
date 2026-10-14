# CLAUDE.md – Ritsu

Ritsu lässt **macOS 27 (arm64) mit Metal auf x86-64-PCs** laufen. Dafür übersetzt es die CPU (AArch64 → x86-64,
`ritsu-dbt`) und die GPU (Metal → Vulkan, `ritsu-gpu-host` + `air2spirv`) auf einem minimalen Linux-Host.
Erstes Zielgerät: Gigabyte G5 MF (i5-12500H, RTX 4050 Laptop, Iris Xe).

**Bevor du irgendetwas änderst, lies:** [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) und den
[ADR-Index](docs/adr/README.md). Die Entscheidungen dort gelten. Wenn du gegen eine verstoßen willst, schreib einen
neuen ADR (Status `Proposed`) und hör auf. Bau nicht einfach daran vorbei.

---

## 1. Rollen und Modelle

Die Hauptsession ist der **Orchestrator**. Er läuft auf **Opus 5.5** (`claude-opus-5-5`, gesetzt in
`.claude/settings.json`). Er plant, zerlegt, delegiert, integriert und prüft. Spezialisierte Arbeit geht an die
Subagents in `.claude/agents/`:

| Agent | Modell | Wofür | Darf schreiben in |
|---|---|---|---|
| `architect` | Opus | ADRs, Schnittstellen zwischen Crates, Protokoll-Design, Abwägungen | `docs/adr/`, `docs/subsystems/`, öffentliche APIs nach Freigabe |
| `dbt-engineer` | Opus | AArch64-Decoder, IR, Interpreter, JIT-Stufen, Speichermodell, PAC | `crates/ritsu-dbt/`, `crates/ritsu-mem/` |
| `gpu-engineer` | Opus | Metal-Objektmodell → Vulkan, Synchronisation, Backends, Präsentation | `crates/ritsu-gpu-host/`, `crates/ritsu-gpu-proto/` |
| `shader-compiler-engineer` | Opus | AIR → SPIR-V, LLVM, SPIRV-Tools, Shader-Cache | `compiler/` |
| `reverse-engineer` | Opus | Clean-Room-Spezifikationen von Apple-Schnittstellen (ADR-0008) | **nur** `docs/specs/`, `docs/research/` |
| `platform-engineer` | Sonnet | vmapple-Geräte, virtio, GIC, Timer, Boot, Host-OS-Image, vmm-Verdrahtung | `crates/ritsu-platform/`, `crates/ritsu-virtio/`, `crates/ritsu-vmm/`, `host-os/` |
| `test-engineer` | Sonnet | Tests, Differenztests, Fuzzing, Fixtures, Benchmarks, CI | `tests/`, `bench/`, `fuzz/`, `.github/`, `#[cfg(test)]`-Module |
| `code-reviewer` | Opus | Review jeder Änderung vor dem Commit, nur lesend | nichts (gibt Befunde zurück) |
| `docs-writer` | Haiku | Doku-Pflege, Changelogs, Glossar, Tippfehler | `docs/` (außer `adr/`, `specs/`), `README.md` |

**Faustregel für das Modell:** Opus, wenn ein Fehler subtil und teuer ist (Speichermodell, Codegenerierung,
GPU-Synchronisation, Shader-Semantik, RE, Architektur, Review). Sonnet für klar spezifizierte Umsetzung nach
vorhandener Spezifikation. Haiku für mechanische Text- und Suchaufgaben. Im Zweifel Opus.

## 2. Der Arbeitsablauf (Work Packages)

Arbeit wird in **Work Packages (WP)** geschnitten. Die Liste steht in [docs/ROADMAP.md](docs/ROADMAP.md), Details
stehen im GitHub-Issue. Ein WP ist so klein, dass ein Agent es in einer Sitzung schafft: ein Thema, ein Crate, mit
Tests.

```
 Orchestrator                     Spezialist(en)                         Orchestrator
 ────────────                     ──────────────                         ────────────
 1. WP wählen ─► 2. Brief schreiben ─► 3. Umsetzen + Tests ─► 4. Review ─► 5. Integrieren
    (ROADMAP)      (Vorlage §2.1)       (eigenes Verzeichnis)    (code-reviewer)  (Checks, Commit)
                         │                                         │
                         └── Schnittstellen-Änderung? ─► architect ┘ (ADR/Proposed → Mensch)
```

1. **Wählen.** Nimm nur WPs, deren Abhängigkeiten erledigt sind. Hochrisiko-WPs (★ in der Roadmap) zuerst als Spike.
2. **Brief.** Jeder Subagent-Auftrag folgt der Vorlage unten. Subagents sehen **nicht** den Chatverlauf, also gehört
   alles Nötige in den Brief.
3. **Umsetzen.** Ein Agent pro Verzeichnis gleichzeitig. Wenn mehrere Implementierer parallel laufen, die sich in die
   Quere kommen könnten, bekommen sie `isolation: "worktree"`. Unabhängige Agenten werden **in einer Nachricht
   gemeinsam** gestartet, damit sie parallel laufen.
4. **Review.** Jede Änderung an `crates/`, `compiler/` oder `guest/` geht vor dem Commit an `code-reviewer`. Befunde
   mit Schwere *blocker* müssen behoben werden.
5. **Integrieren.** Der Orchestrator führt die Prüfungen aus §4 selbst aus (er verlässt sich nicht auf den Bericht
   des Agenten), committet und aktualisiert die Roadmap.

### 2.1 Brief-Vorlage für Subagents

```
WP-<id>: <Titel>
Ziel:            <ein Satz, überprüfbar>
Kontext:         <relevante ADRs, Spezifikationen, Dateien, Entscheidungen aus dem Chat>
Scope:           <Verzeichnisse, die geändert werden dürfen>
Nicht-Scope:     <was ausdrücklich nicht angefasst wird>
Schnittstellen:  <öffentliche Typen/Funktionen, die feststehen; was nicht geändert werden darf>
Akzeptanz:       <Tests/Befehle, die grün sein müssen; Messwerte>
Bericht:         Format aus §2.2
```

### 2.2 Bericht-Format (Pflicht für jeden Subagent)

```
## Ergebnis        – erledigt / teilweise / blockiert (ein Satz)
## Änderungen      – Dateien + je ein Satz
## Prüfungen       – exakte Befehle + Ergebnis (bestanden/fehlgeschlagen, Anzahl Tests)
## Offene Fragen   – Entscheidungen, die ein Mensch oder der architect treffen muss
## Risiken/Schulden – bewusst Ausgelassenes, TODOs mit WP-Vorschlag
```

### 2.3 Eskalation

* Ein Agent hängt zweimal am selben Problem → Orchestrator übernimmt oder holt `architect`.
* Eine Schnittstelle zwischen Crates muss sich ändern → `architect` schlägt sie vor, der Orchestrator gibt frei.
  Implementierer ändern keine fremden öffentlichen APIs.
* Rechtliche Fragen oder Fragen zu Apple-Binärdateien → stoppen, den Menschen fragen.
* Etwas lässt sich nicht testen (z. B. braucht echte Hardware oder einen Mac) → im Bericht unter *Offene Fragen*
  melden, nicht stillschweigend überspringen.

## 3. Harte Regeln

1. **Clean Room (ADR-0008).** Keine Apple-Binärdateien, kein disassemblierter oder dekompilierter Apple-Code und keine
   kopierten Apple-Strings im Repo. `reverse-engineer` schreibt nur Spezifikationen. Wer eine Spezifikation geschrieben
   hat, implementiert denselben Bereich nicht.
2. **Gast-Eingaben sind feindlich.** Alles, was aus dem Gast kommt (MMIO, DMA-Deskriptoren, GPU-Befehle, Shader,
   Größenangaben), wird validiert. Kein `panic!`, `unwrap()` oder Index ohne Prüfung auf Gast-Daten. Fehler werden
   zu `ritsu_core::Error::Guest` oder zu einer architekturkonformen Gast-Exception.
3. **`unsafe` nur mit Begründung.** Jeder `unsafe`-Block bekommt `// SAFETY:` (Clippy erzwingt das) und lebt in einem
   Modul, dessen Name das sagt (`jit_mem`, `ffi`, `host_map`).
4. **Kein AVX-512.** Der i5-12500H hat keins. Host-Codegenerierung darf höchstens AVX2 + BMI2 + FMA voraussetzen.
   Feature-Erkennung zur Laufzeit.
5. **Niemals still falsch.** Wenn ein Metal-Feature oder ein Befehl nicht unterstützt wird: Gast-Exception, ehrlich
   reduzierte Feature-Meldung oder `Error::Unimplemented`. Nie still ein falsches Ergebnis liefern.
6. **ADRs sind Gesetz.** `Accepted`-ADRs werden nicht editiert, sondern durch neue ersetzt. Auf `Proposed`-ADRs
   (aktuell ADR-0005) baut niemand Irreversibles.
7. **Keine Abhängigkeitszyklen.** Die Crate-Grafik in ARCHITECTURE.md §4 ist verbindlich. Neue externe
   Abhängigkeiten nur über `[workspace.dependencies]` und mit Begründung im Bericht.
8. **Messen statt raten.** Leistungsbehauptungen nur mit Benchmark aus `bench/` und Vorher/Nachher-Zahlen.
9. **Gemeinsamer Arbeitsbaum.** Mehrere Agenten arbeiten gleichzeitig im selben Checkout. Subagents führen **keine**
   Git-Befehle aus, die den Arbeitsbaum oder Index verändern (`stash`, `checkout`, `switch`, `reset`, `restore`,
   `clean`, `commit`, `merge`, `rebase`, `add`). Lesend (`status`, `diff`, `log`, `show`) ist erlaubt. Für
   Vergleiche mit einem alten Stand: `git worktree add` in das Scratchpad oder `git show <rev>:<pfad>`.

## 4. Befehle

```bash
make check          # fmt, clippy -D warnings, alle Tests (Rust + C++), Crate-Graph, Links, Status. Muss vor jedem Commit grün sein.
make test           # nur Tests
make fmt            # formatieren (Rust + C++)
cargo test -p <crate>                              # ein Crate
cargo test -p ritsu-dbt -- --nocapture <filter>    # einzelner Test
cmake -S compiler/air2spirv -B build/air2spirv -G Ninja && cmake --build build/air2spirv && ctest --test-dir build/air2spirv
```

Was in dieser Cloud-/Linux-Umgebung **nicht** geht: echte GPU (kein Vulkan-Gerät), Bau von `guest/` (braucht
macOS-SDK), Boot eines echten macOS-Images (keine Apple-Binärdateien vorhanden). Solche Prüfungen im Bericht als
„nicht ausführbar hier“ markieren.

## 5. Standards

* **Sprache:** Code, Bezeichner, Kommentare, Commit-Nachrichten auf Englisch. Doku in `docs/` auf Deutsch.
* **Commits:** [Conventional Commits](https://www.conventionalcommits.org/) mit Scope = Crate/Bereich:
  `feat(dbt): decode AArch64 logical immediates`, `docs(adr): propose ADR-0009 …`. Ein logisches Thema pro Commit.
* **Branches:** `<typ>/<wp-id>-<kurz>` (z. B. `feat/wp-dbt-03-t1-jit`). Merge in `main` nur über PR mit grüner CI.
* **Tests:** Jede Befehlsklasse im DBT bekommt Differenztests (T0 ↔ T1 ↔ T2 ↔ Referenz). Jeder Gast-Parser bekommt
  einen Fuzz-Target. Jede GPU-Funktion bekommt einen Bildvergleichstest, sobald ein Vulkan-Gerät verfügbar ist.
  Details: [docs/process/testing.md](docs/process/testing.md).
* **Definition of Done:** [docs/process/definition-of-done.md](docs/process/definition-of-done.md).
* **Coding-Standards:** [docs/process/coding-standards.md](docs/process/coding-standards.md).
* **Doku mitziehen:** Ändert sich Verhalten oder Struktur eines Subsystems, wird `docs/subsystems/<x>.md` im selben
  Commit aktualisiert.

## 6. Orientierung

```
CLAUDE.md                 diese Datei
.claude/agents/           Subagent-Definitionen (Rollen aus §1)
.claude/skills/           Projekt-Skills: /adr, /wp, /spike
docs/ARCHITECTURE.md      Gesamtarchitektur, Qualitätsziele, Risiken
docs/ROADMAP.md           Meilensteine M0–M8 und Work Packages
docs/OPEN-QUESTIONS.md    offene Entscheidungen für den Menschen
docs/adr/                 Architekturentscheidungen
docs/subsystems/          Detaildesign pro Subsystem (dbt, gpu, shader-compiler, platform, host-os)
docs/process/             Orchestrierung, Clean Room, Tests, DoD, Coding-Standards
docs/specs/               Clean-Room-Spezifikationen (nur reverse-engineer schreibt hier)
docs/research/            Spike-Berichte
status/                   maschinenlesbarer Projektstand + Quelltext der Live-Übersicht
crates/                   Rust-Host (Workspace)
compiler/air2spirv/       AIR → SPIR-V (C++20, CMake)
guest/                    Gast-Komponenten (nur ADR-0005 Variante G, braucht macOS-SDK)
host-os/                  Appliance-Linux-Image
tests/ bench/ fuzz/       übergreifende Tests, Benchmarks, Fuzzing
legacy/g5mf-sequoia/      alter, funktionierender Hackintosh (macOS 15 + NullMoth). Eingefroren, nicht anfassen.
```

## 7. Dauerauftrag (Autopilot)

Die Agents arbeiten **durchgehend** an der Roadmap, ohne Runden. Der Orchestrator betreibt eine Pipeline mit bis zu
4 parallelen Spuren (CPU, GPU, Shader, Plattform/Qualität). Sobald ein Spezialist fertig ist, wird geprüft, reviewt,
integriert und dieselbe Spur **sofort** mit dem nächsten Work Package belegt. Details:
[docs/process/autopilot.md](docs/process/autopilot.md). Eine stündliche Routine ist nur Wächter und startet die
Pipeline neu, falls sie steht.

* **Aktuelles Ziel:** steht in `status/meta.json` (`goal`) und ist immer der nächste offene Meilenstein.
* **Statusdaten** in `status/*.json` sind Teil der Definition of Done: Jede Integration aktualisiert sie, `make status`
  prüft sie, und danach wird die Live-Übersicht aktualisiert:
  <https://claude.ai/artifact/AEJLPVPPQZCncXeXG5Hr8y> (privat, nur für den Projektinhaber).
* Der Autopilot entscheidet nie, was dem Menschen vorbehalten ist (ADR-Freigaben, `O-*`-Fragen, Lizenz, Apple-Dateien).
