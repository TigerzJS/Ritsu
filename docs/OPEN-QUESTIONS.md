# Offene Fragen an den Projektinhaber

Entscheidungen, die kein Agent treffen darf. Neue Fragen aus Agentenberichten werden hier gesammelt.

| ID | Frage | Optionen / Empfehlung | Blockiert |
|---|---|---|---|
| O-1 | **Lizenz** des Projekts? | *Empfehlung:* GPL-2.0-or-later, wenn Code aus QEMU/Linux übernommen werden soll. Apache-2.0 OR MIT, wenn maximale Wiederverwendbarkeit wichtiger ist (dann kein GPL-Code). Bis zur Entscheidung: `UNLICENSED`, keine externen Beiträge | externe Beiträge, Veröffentlichung von Releases |
| O-2 | Darf ein **Mac** für Einmal-Aufgaben genutzt werden (AIR-Fixtures kompilieren, Referenzbilder rendern, Spike S-GPU-2, Fallback-Installation)? | Ohne Mac fehlen Referenzdaten für Shader- und Bildtests | M4 |
| O-3 | Soll ein **self-hosted CI-Runner auf dem G5 MF** laufen (echte RTX 4050 / Iris Xe)? | Empfohlen ab M4 | GPU-Hardware-Tests |
| O-4 | **Zusammenarbeit mit NullMoth?** Deren Treiber übersetzt bereits Metal → SPIR-V → NVK (nativ, macOS 15). Lizenz PolyForm Noncommercial verbietet Übernahme in ein Projekt mit freier Lizenz | Erkenntnisse nur über öffentliche Aussagen nutzen, ggf. Kontakt aufnehmen | – |
| O-5 | Zielgruppe v1: nur G5 MF, oder von Anfang an „beliebiger x86-PC mit Vulkan-1.3-GPU“? | *Empfehlung:* G5 MF als Referenz, Architektur generisch (ist sie bereits) | Host-OS-Image |
