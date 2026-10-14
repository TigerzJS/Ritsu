# Orchestrierung mit mehreren Agenten

Ergänzt [CLAUDE.md](../../CLAUDE.md) §1–2 um das *Warum* und um Muster für typische Situationen.

## Prinzipien

1. **Der Orchestrator hält das Gesamtbild, Spezialisten halten die Tiefe.** Der Orchestrator (Opus 5.5) liest Berichte
   und Diffs, nicht jede Datei. Er entscheidet, was als Nächstes passiert, und prüft Ergebnisse selbst nach.
2. **Kontext ist teuer und flüchtig.** Ein Subagent sieht nur seinen Brief. Alles Entscheidende (ADRs, Schnittstellen,
   Verbote) steht deshalb im Repo, damit Briefe darauf verweisen können, statt es zu wiederholen.
3. **Schnittstellen vor Parallelität.** Bevor zwei Agenten parallel an benachbarten Crates arbeiten, legt der
   Orchestrator oder `architect` die gemeinsame Schnittstelle fest und friert sie für die Dauer der Arbeit ein.
   (In M0 war das `ritsu_mem::GuestMemory`.)
4. **Trennung der Rollen ist Absicht.** Wer analysiert, implementiert nicht (Clean Room). Wer implementiert, reviewt
   nicht selbst. Wer testet, prüft gegen unabhängige Erwartungen.
5. **Vertrauen, aber prüfen.** Ein Bericht „alle Tests grün“ wird vom Orchestrator mit `make check` nachvollzogen.

## Muster

### Fan-out über Subsysteme (wie in M0)

```
Orchestrator: Schnittstelle einfrieren → N Briefe → N Agenten parallel (eine Nachricht)
            → Berichte sammeln → make check → code-reviewer über alles → Fixes → Commit
```

Voraussetzung: disjunkte Verzeichnisse. Jeder Agent formatiert und testet nur seine Crates.

### Pipeline für eine Befehlsklasse im DBT

```
dbt-engineer (Decoder + IR-Lowering + T0) → test-engineer (Differenztests gegen Referenz)
  → dbt-engineer (T1-Codegen) → test-engineer (T0↔T1) → code-reviewer → Commit
```

### Clean-Room-Pipeline für ein undokumentiertes Gerät

```
reverse-engineer → docs/specs/<bereich>/<gerät>.md → architect (Schnittstelle) → Freigabe Mensch
  → platform-engineer implementiert NUR aus der Spec → code-reviewer prüft auch auf Clean-Room-Verstöße
```

Der Implementierer bekommt im Brief nur den Pfad der Spezifikation, keine Rohbeobachtungen.

### Spike

Zeitlich begrenzt, in `isolation: "worktree"`, Ergebnis ist ein Bericht in `docs/research/`. Prototyp-Code wird nicht
gemerged (Skill `/spike`).

### Konkurrierende Entwürfe

Bei schwierigen Designfragen (z. B. Hazard-Tracking im GPU-Kern) können zwei `architect`-Instanzen unabhängig je
einen Entwurf schreiben. Der Orchestrator vergleicht und legt die Abwägung dem Menschen vor.

## Modellwahl

| Situation | Modell |
|---|---|
| Orchestrierung, Integration, Entscheidungsvorlagen | Opus 5.5 (Hauptsession) |
| Codegenerierung, Speichermodell, GPU-Sync, Shader-Semantik, RE, Architektur, Review | Opus |
| Umsetzung nach fertiger Spezifikation (Geräte, CLI, Tests, CI) | Sonnet |
| Mechanische Doku, Link-Fixes, Suche | Haiku / Explore |

Ein Sonnet-Agent, der zweimal an derselben Stelle scheitert, wird durch Opus ersetzt (Eskalation, CLAUDE.md §2.3).

## Arbeitsstand sichtbar halten

* `docs/ROADMAP.md`: Status pro Meilenstein und WP, gepflegt vom Orchestrator.
* `docs/OPEN-QUESTIONS.md`: alles, was ein Mensch entscheiden muss.
* GitHub-Issues pro WP (Titel = WP-ID), sobald das Projekt Mitwirkende hat.
