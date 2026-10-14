# ADR-0008: Reverse Engineering nur im Clean-Room-Verfahren

* **Status:** Accepted
* **Datum:** 2026-10-14
* **Betrifft:** gesamtes Projekt, besonders `docs/specs/`

## Kontext

Für Interoperabilität müssen Schnittstellen von Apple verstanden werden (Paravirt-GPU, Apple-VM-Geräte, AIR-Metadaten,
private Metal-Treiber-Schnittstellen). Das Urheberrecht schützt Apples Code. In der EU erlaubt Art. 6 der
Software-Richtlinie (in Deutschland § 69e UrhG) Dekompilierung in engen Grenzen, wenn sie für Interoperabilität
unerlässlich ist. **Das ist keine Rechtsberatung.**

## Entscheidung

1. **Rollentrennung.** Wer Apple-Binärcode analysiert (Agent-Rolle `reverse-engineer` oder ein Mensch), schreibt
   **nur Spezifikationen** in `docs/specs/<bereich>/` und keinen Ritsu-Code für denselben Bereich. Implementierer
   arbeiten nur mit diesen Spezifikationen.
2. **Spezifikationen beschreiben Verhalten, nicht Code:** Registerlayouts, Nachrichtenformate, Zustandsautomaten,
   beobachtete Sequenzen. Verboten sind Disassembly-Listings, dekompilierter Code, kopierte Symbole über das Nötige
   hinaus und kopierte Kommentare oder Strings.
3. **Bevorzugte Methoden:** Black-Box-Beobachtung (Traces von MMIO/DMA am eigenen Emulator, Protokoll-Mitschnitte),
   öffentliche Dokumentation, öffentliche Header des SDK. Disassembly nur, wenn das nicht reicht, und dann mit Begründung
   im Spec-Dokument.
4. **Keine Apple-Binärdateien im Repo**, auch nicht in Tests oder Fixtures. Ausnahme: selbst geschriebene Shader, die
   mit Apples Compiler zu AIR übersetzt wurden (ADR-0007).
5. Jede Spezifikation hat einen Kopf mit Quelle/Methode, macOS-Build und Autor-Rolle.

## Konsequenzen

* Der Agent `reverse-engineer` hat **keine Schreibrechte** außerhalb von `docs/specs/` und `docs/research/`.
* Reviewer prüfen PRs auf Verstöße (Checkliste in `docs/process/clean-room.md`).
