# ADR-0006: DBT ist mehrstufig (Interpreter, Basis-JIT, Cranelift-JIT) mit persistentem Cache

* **Status:** Accepted
* **Datum:** 2026-10-14
* **Betrifft:** `crates/ritsu-dbt`, `crates/ritsu-mem`

## Kontext

Der gesamte Gast (Kernel + Userland) läuft durch den DBT. Ein Teil des Codes läuft einmal (Boot, Initialisierung),
ein anderer Teil sehr oft (Systembibliotheken, Spiele-Schleifen). Ein einzelner JIT ist entweder zu langsam beim
Übersetzen oder erzeugt zu schlechten Code. macOS hat einen großen, pro Build unveränderlichen `dyld shared cache`.

## Entscheidung

* **T0 Interpreter:** Vollständige, langsame Referenz-Semantik. Er ist Orakel für alle Differenztests und führt kalten
  Code aus.
* **T1 Basis-JIT:** Template-basiert, ein Basisblock nach dem anderen, Ziel unter 1 µs pro Block. Block-Chaining,
  Inline-TLB-Fastpath.
* **T2 optimierender JIT:** Regionen (Traces/Superblöcke) heißer Pfade → eigene SSA-IR → Optimierungen
  (Flag-Elimination, Registerzuweisung über Blockgrenzen, Redundanz-Elimination von PAC-Checks) → Cranelift.
  Läuft asynchron auf Compiler-Threads (E-Kerne).
* **Persistenter Cache:** T1/T2-Ergebnisse für unveränderliche, signierte Code-Seiten (dyld shared cache, Kernelcache)
  werden auf Platte abgelegt. Schlüssel: (macOS-Build, physischer Seiten-Hash, DBT-Version, CPU-Feature-Satz des
  Hosts).
* **Eine gemeinsame Semantik-Quelle:** Die Befehlssemantik wird **einmal** beschrieben (Decoder → IR-Lowering). T0,
  T1 und T2 leiten sich davon ab. So können die Stufen nicht auseinanderlaufen.

## Konsequenzen

* Differenztests T0 ↔ T1 ↔ T2 ↔ Referenz sind Pflicht für jede neue Befehlsklasse (siehe testing.md).
* Cranelift ist eine externe Abhängigkeit mit eigenem Release-Zyklus. Die Version wird gepinnt, und Updates laufen
  nur mit Benchmark-Vergleich.
* Ob ein eigener x86-Backend später Cranelift ersetzt, entscheidet erst eine Messung (frühestens M6).
