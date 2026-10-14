# ADR-0004: Gast-Plattform ist die Apple-VM-Plattform (vmapple), kein echtes Apple-SoC

* **Status:** Accepted
* **Datum:** 2026-10-14
* **Betrifft:** `crates/ritsu-platform`, `crates/ritsu-virtio`

## Kontext

macOS arm64 bootet entweder auf echter Apple-Silicon-Hardware (M-Serie mit AIC, DART, ANS, SEP, AGX …) oder in
Apples Virtualisierungsplattform (Virtualization.framework). Für die VM-Plattform gibt es einen eigenen Kernelcache
und einen kleinen, weitgehend standardisierten Gerätesatz: GICv3, ARM Generic Timer, PL011, PL031, virtio-Geräte,
dazu wenige Apple-spezifische Geräte (u. a. AES-Engine, Konfigurationsbereich, Boot-Disk-Schnittstelle,
paravirtuelle GPU). QEMU (ab 10.0) hat dieses Modell als Maschine `vmapple` öffentlich implementiert und dokumentiert,
allerdings nur für Apple-Silicon-Hosts mit HVF und nur mit Apples Host-Framework für die GPU.

## Optionen

1. **Echtes Apple-SoC emulieren** (wie Asahi-Linux es von der anderen Seite her kennt). Dutzende undokumentierte
   Geräte, SEP, Firmware-Blobs. Größenordnungen mehr Aufwand.
2. **vmapple-kompatible Plattform.** Kleiner Gerätesatz, öffentlich nachvollziehbar, und macOS bringt die Gast-Treiber
   (virtio, Paravirt-GPU) mit.

## Entscheidung

Option 2. `ritsu-platform` bildet die Apple-VM-Plattform nach. Gerätebeschreibungen entstehen als Spezifikationen in
`docs/specs/platform/` nach ADR-0008. Öffentlich dokumentiertes Verhalten (z. B. QEMU-Doku, virtio-Spezifikation,
ARM-Architekturhandbuch) darf direkt verwendet werden.

## Konsequenzen

* Das Boot-ROM (`AVPBooter`) ist Apple-Code. Ritsu liefert es nicht aus. Spike S-BOOT-1 prüft, ob ein eigener
  Bootloader den VM-Kernelcache starten kann.
* Die Installation aus einer IPSW läuft bei Apple über Virtualization.framework auf einem Mac. Spike S-BOOT-2 prüft
  einen Host-seitigen Restore-Pfad. Bis dahin gilt als Rückfall: Image einmalig auf einem Mac erzeugen.
* Die Gast-CPU meldet einen bewusst gewählten, minimalen Feature-Satz über die ID-Register (z. B. kein SME, kein AMX).
  Jedes zusätzliche Feature muss der DBT vollständig und getestet unterstützen.
