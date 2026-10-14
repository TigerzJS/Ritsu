# ADR-0002: Host-OS ist Linux, Host-Grafik-API ist Vulkan 1.3+

* **Status:** Accepted
* **Datum:** 2026-10-14
* **Betrifft:** `crates/ritsu-vmm`, `crates/ritsu-gpu-host`, `host-os/`

## Kontext

Der Emulator braucht ein Host-Betriebssystem mit Treibern für alle Geräte des Ziel-Laptops und mit einer modernen
GPU-API. Der G5 MF hat keinen MUX: Der interne Bildschirm hängt an der Iris Xe.

## Optionen

1. **Linux.** Reife Treiber für alle Komponenten, Vulkan auf NVIDIA (proprietär, NVK) und Intel (ANV), PRIME
   Render-Offload für Laptops ohne MUX, DRM/KMS für Direkt-Scanout, gute Werkzeuge zum Profilen (perf, RenderDoc).
2. **Windows.** Ebenfalls Vulkan, aber Appliance-Betrieb (Boot direkt in den Emulator) ist schwierig, die Treiber
   sind geschlossen, und Profiling von JIT-Code ist umständlicher.
3. **Eigener Bare-Metal-Hypervisor.** Man müsste alle Gerätetreiber selbst schreiben. Ausgeschlossen.

## Entscheidung

Linux (x86-64) als einziger Host in v1. Vulkan 1.3 ist die Mindestversion, benötigte Erweiterungen pro Feature
stehen in [gpu.md](../subsystems/gpu.md). Kein OpenGL-Fallback.

## Konsequenzen

* Das Host-OS wird als minimales Appliance-Image ausgeliefert (`host-os/`). Für den Nutzer soll es sich wie ein
  Hackintosh-Stick anfühlen.
* Der interne Bildschirm des G5 MF wird beschleunigt (PRIME), was im alten Hackintosh unmöglich war.
* Windows-Host bleibt ein Nicht-Ziel bis auf Widerruf.
