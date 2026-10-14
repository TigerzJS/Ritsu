# Offene Fragen an den Projektinhaber

Entscheidungen, die kein Agent treffen darf. Neue Fragen aus Agentenberichten werden hier gesammelt.

| ID | Frage | Optionen / Empfehlung | Blockiert |
|---|---|---|---|
| O-1 | **Lizenz** des Projekts? | *Empfehlung:* GPL-2.0-or-later, wenn Code aus QEMU/Linux übernommen werden soll. Apache-2.0 OR MIT, wenn maximale Wiederverwendbarkeit wichtiger ist (dann kein GPL-Code). Bis zur Entscheidung: `UNLICENSED`, keine externen Beiträge | externe Beiträge, Veröffentlichung von Releases |
| O-2 | Darf ein **Mac** für Einmal-Aufgaben genutzt werden (AIR-Fixtures kompilieren, Referenzbilder rendern, Spike S-GPU-2, Fallback-Installation)? | Ohne Mac fehlen Referenzdaten für Shader- und Bildtests | M4 |
| O-3 | Soll ein **self-hosted CI-Runner auf dem G5 MF** laufen (echte RTX 4050 / Iris Xe)? | Empfohlen ab M4 | GPU-Hardware-Tests |
| O-4 | **Zusammenarbeit mit NullMoth?** Deren Treiber übersetzt bereits Metal → SPIR-V → NVK (nativ, macOS 15). Lizenz PolyForm Noncommercial verbietet Übernahme in ein Projekt mit freier Lizenz | Erkenntnisse nur über öffentliche Aussagen nutzen, ggf. Kontakt aufnehmen | – |
| O-5 | Zielgruppe v1: nur G5 MF, oder von Anfang an „beliebiger x86-PC mit Vulkan-1.3-GPU“? | *Empfehlung:* G5 MF als Referenz, Architektur generisch (ist sie bereits) | Host-OS-Image |
| O-6 | Sollen **RTX 4050 und Iris Xe dieselbe Metal-Fähigkeitsmenge** melden (einfacher für Apps beim GPU-Wechsel) oder jede GPU ihre eigene (mehr Features auf der RTX)? | Siehe [gpu.md](subsystems/gpu.md) G-4 | M4 |
| O-7 | Ist **ASTC → BC7-Transkodierung** (verlustbehaftet) als Option erlaubt, oder nur verlustfrei nach RGBA8 (mehr VRAM)? | Siehe gpu.md G-5 | M5 |
| O-8 | Neue Abhängigkeiten für den GPU-Pfad: `ash` (Vulkan), `gpu-allocator` oder eigener Allocator? | Siehe gpu.md G-12 | WP-GPU-03 |
| O-9 | **PAC-Algorithmus** im Gast: echtes QARMA (langsam, exakt) oder schnelle eigene Funktion (Gast braucht nur Selbstkonsistenz)? | Siehe [dbt.md](subsystems/dbt.md) O-DBT-4 | WP-DBT (PAC) |
| O-10 | Ist die **ABA-Abweichung** der `LDXR/STXR`-Emulation über `CMPXCHG` akzeptabel? | Siehe dbt.md O-DBT-5 | M3 |
| O-11 | Neue Abhängigkeiten für den DBT: Software-FP (`rustc_apfloat`), x86-Emitter (`iced-x86`/`dynasmrt` oder eigener), BLAKE3 für den Cache | Siehe dbt.md O-DBT-8…10 | M2 |
| O-12 | **SPIRV-Tools-Bezug:** Ubuntu-Pakete (umgesetzt, einfach) oder fest gepinnter Commit (bitgleiche Shader-Ausgabe auf jeder Maschine)? Der Cache bleibt in beiden Fällen korrekt | Siehe [shader-compiler.md](subsystems/shader-compiler.md) O-SC-4 | – |
| O-13 | **Freigabe [ADR-0009](adr/0009-shared-guest-ram.md)** (gemeinsamer Gastspeicher: eigenes memfd-basiertes `GuestRam` mit `&self`-Zugriff, `unsafe` nur in `ritsu-mem::host_map`)? | *Empfehlung des Architekten:* Option B annehmen | WP-MEM-01/02, WP-DBT-09, WP-PLAT-10, WP-GPU-11 |
