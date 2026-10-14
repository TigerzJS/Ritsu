# ADR-0005: GPU-Gastpfad: Paravirt (P) oder eigener Treiber (G)

* **Status:** **Proposed**. Es darf noch nichts Irreversibles darauf gebaut werden.
* **Datum:** 2026-10-14
* **Betrifft:** `crates/ritsu-gpu-proto`, `crates/ritsu-gpu-host`, `guest/`

## Kontext

Metal-Aufrufe im Gast müssen auf die Host-GPU. Unter Virtualization.framework übernimmt das Apples paravirtuelle GPU:
Ein Gast-Treiber, der in macOS enthalten ist, spricht über MMIO und Shared Memory mit dem Host-Framework
`ParavirtualizedGraphics`, das auf dem Host mit Metal rendert. Das Protokoll ist nicht dokumentiert. **Annahme
(ungeprüft):** Es überträgt Befehle auf ungefähr Metal-Ebene, einschließlich Shader als AIR.

## Optionen

* **P – Host-Seite des Apple-Paravirt-Protokolls neu implementieren.**
  + Kein eigener Code im Gast, keine Signatur- oder SIP-Probleme.
  + Apple pflegt den Gast-Treiber.
  − Protokoll per Clean-Room-RE erschließen (ADR-0008), Änderungen pro macOS-Update.
  − Der Metal-Feature-Umfang der virtuellen GPU ist durch Apple festgelegt (**Annahme:** kleiner als „volle“
    Apple-GPU-Familie, z. B. ohne Ray Tracing).
* **G – Eigener Gast-Treiber + eigenes Protokoll (`ritsu-gpu-proto`).**
  + Volle Kontrolle über gemeldete Features, dokumentiertes und versioniertes Protokoll, Host und Gast gemeinsam
    testbar.
  − Eigener Kext + Metal-Treiber-Bundle (private Schnittstellen von Metal.framework), SIP/AMFI im Gast müssen
    gelockert werden, Bau braucht macOS-SDK.
* **Hybrid:** P zuerst (schnell zum ersten Bild), G später für Features, die P nicht kann.

## Vorgeschlagene Entscheidung

Hybrid. **Der Host-Kern (`ritsu-gpu-host`) wird protokollneutral gebaut:** Er arbeitet auf einem internen
Metal-Objektmodell. P und G sind nur zwei Frontends, die dieses Modell füttern. Das ist schon jetzt verbindlich,
weil es unabhängig vom Ausgang der Spikes richtig ist.

## Spikes, die entscheiden

* **S-GPU-1 (P-Machbarkeit):** Lassen sich Befehlsstrom-Format, Ressourcen-Lebenszyklus und Shader-Übergabe des
  Paravirt-Protokolls so weit beschreiben, dass ein Dreieck gerendert werden kann? Welche Metal-Familien meldet der
  Gast-Treiber? Ergebnis: Spezifikation in `docs/specs/gpu-paravirt/` + Bericht in `docs/research/`.
* **S-GPU-2 (G-Machbarkeit):** Lässt sich in einem macOS-27-Gast (SIP reduziert) ein eigenes Metal-Device-Bundle
  laden, das `MTLCreateSystemDefaultDevice()` liefert? Ergebnis: Bericht + Prototyp in `guest/`.

## Überprüfung

Entscheidung fällt in M4. Kriterium: Welcher Pfad bringt zuerst das Pflicht-Szenario „Metal-Dreieck + WindowServer
beschleunigt“ auf **beiden** Ziel-GPUs?
