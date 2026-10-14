# ADR-0001: macOS 27 wird per Full-System-Translation ausgeführt, nicht nativ

* **Status:** Accepted
* **Datum:** 2026-10-14
* **Entscheider:** Projektinhaber (Freigabe), Orchestrator (Entwurf)
* **Betrifft:** gesamtes Projekt

## Kontext

* macOS 27 „Golden Gate“ unterstützt nur noch Apple-Silicon-Macs. macOS 26 Tahoe war die letzte Version für
  Intel-Macs (Quellen: 9to5mac 2026-05-18, HotHardware, itdaily, siehe [ARCHITECTURE.md](../ARCHITECTURE.md)).
* Ohne Intel-Macs gibt es keinen x86-64-Kernel und keine x86-64-Userland-Binaries mehr. Der klassische Hackintosh
  (OpenCore + Kexts + gespoofte Hardware) braucht aber genau diese.
* Die Zielhardware (i5-12500H) ist x86-64 und hat keine Hardware-Virtualisierung für AArch64-Gäste.
* Das Vorgängerprojekt in diesem Repo (macOS 15 + NullMoth-Treiber) zeigt, dass Metal → SPIR-V → NVK auf der RTX
  4050 grundsätzlich funktioniert, allerdings nativ auf x86.

## Optionen

1. **Bei macOS 26 bleiben (nativer Hackintosh).** Funktioniert, aber macOS 27 ist damit ausgeschlossen. Erfüllt das
   Ziel nicht.
2. **x86-64-Neuübersetzung des macOS-27-Userlands (statische Rekompilierung) + eigener x86-Kernel.** Man müsste XNU
   und Tausende Binaries von arm64e nach x86 umsetzen, inklusive PAC, und das bei jedem Update erneut.
   Wartungsaufwand nicht beherrschbar.
3. **Full-System-Translation:** Den unveränderten arm64-Gast in einer virtuellen Apple-Plattform ausführen, die CPU
   per DBT, die GPU per API-Übersetzung.

## Entscheidung

Option 3. Ritsu ist ein Full-System-Emulator/Übersetzer für macOS 27 arm64 auf x86-64-Hosts mit
Metal→Vulkan-GPU-Pfad.

## Konsequenzen

* Positiv: Der Gast bleibt unverändert, Apple-Updates sind (fast) einfach neue Gast-Images. Echte Hardware wird von
  Linux-Treibern bedient.
* Negativ: Leistung ist das Hauptrisiko (R-2). Es braucht einen eigenen, hochoptimierten DBT.
* Folgearbeiten: ADR-0002, 0004, 0005, 0006, Meilensteine M1 bis M3.

## Überprüfung

Kippt, wenn Q-PERF-1 nach M6 trotz AOT-Cache und T2-JIT unter 15 % bleibt. Dann ist macOS 27 auf x86 nicht sinnvoll
nutzbar, und das Projekt sollte sich auf einen nativen macOS-26-Pfad zurückziehen.
