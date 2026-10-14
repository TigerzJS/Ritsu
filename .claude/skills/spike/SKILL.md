---
name: spike
description: Führt einen zeitlich begrenzten Machbarkeits-Spike (z. B. S-GPU-1, S-BOOT-1) durch und schreibt einen Bericht nach docs/research/. Verwenden, wenn eine Entscheidung (meist ein ADR mit Status Proposed) von unbekannten Fakten abhängt.
---

# Spike durchführen

1. **Frage festlegen:** Welche Entscheidung hängt davon ab (ADR-Nummer)? Welche Antwort würde sie wie verändern?
2. **Budget festlegen:** maximaler Aufwand. Ein Spike liefert Wissen, keinen Produktionscode.
3. **Rolle wählen:** Analyse undokumentierter Apple-Schnittstellen → nur `reverse-engineer` (ADR-0008). Prototyp-Code
   → der passende Spezialist, in `isolation: "worktree"`. Prototyp-Code wird **nicht** nach `main` gemerged.
4. **Bericht** nach `docs/research/S-<ID>-<kurz>.md` mit: Frage, Methode, Ergebnisse (bestätigt / wahrscheinlich /
   Vermutung), Empfehlung für den ADR, neue Risiken, Folge-WPs.
5. Ergebnis in den betroffenen ADR als Vorschlag einbringen (`/adr` bei neuem ADR) und dem Menschen vorlegen.
