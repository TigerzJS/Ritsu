# Clean-Room-Verfahren (Umsetzung von ADR-0008)

**Keine Rechtsberatung.** Dieses Verfahren senkt Risiken, beseitigt sie aber nicht.

## Rollen

| Rolle | Darf | Darf nicht |
|---|---|---|
| Analyst (`reverse-engineer`, Mensch) | Apple-Software beobachten und analysieren, Spezifikationen schreiben | Ritsu-Code für den analysierten Bereich schreiben |
| Implementierer | Spezifikationen lesen, Code schreiben | Apple-Binärcode des Bereichs analysieren, Rohbeobachtungen lesen |
| Reviewer | Beides prüfen | Inhalte zwischen den Seiten weitertragen |

## Was in eine Spezifikation darf

* Registeradressen, Bitfelder, Nachrichtenformate, Zustandsautomaten, Abläufe
* Für die Interoperabilität nötige Namen (z. B. ein Kompatibilitätsstring, den der Gast-Treiber sucht)
* Konstanten, die das Protokoll erfordert

## Was nie ins Repo darf

* Apple-Binärdateien, Firmware, Boot-ROMs, IPSWs, Kernelcaches, Teile davon
* Disassembly, Pseudocode aus Decompilern, kopierte Funktionsstrukturen
* Kopierte Kommentare, Fehlermeldungen oder Strings von Apple, die für Interoperabilität nicht nötig sind
* GPL-Code (z. B. QEMU), solange die Lizenzfrage O-1 offen ist

## Checkliste für Reviewer

- [ ] Enthält der Diff Binärdateien? Wenn ja: Herkunft (nur eigene Fixtures nach ADR-0007 erlaubt)
- [ ] Enthalten Kommentare/Strings Formulierungen, die nach Apple-Code aussehen?
- [ ] Verweist die Implementierung eines undokumentierten Bereichs auf eine Spezifikation in `docs/specs/`?
- [ ] Stammt die Spezifikation von einer anderen Rolle als die Implementierung?
