# bench/ – Benchmarks

Leistungsbehauptungen gelten nur mit einer Messung aus diesem Verzeichnis (CLAUDE.md §3 Regel 8). Die Ziele stehen in
[ARCHITECTURE.md §6](../docs/ARCHITECTURE.md) (Q-PERF-1 bis Q-PERF-4); Methodik und Schwellen in
[docs/process/testing.md §8](../docs/process/testing.md).

## Geplantes Layout

| Pfad | Inhalt |
|---|---|
| `cpu/` | Integer-/NEON-Mikro-Benchmarks für den DBT (Q-PERF-1), Gast-Programme als Quelltext |
| `boot/` | Boot-Zeit bis Login-Fenster (Q-PERF-2), braucht Nutzer-Image, nur HIL |
| `gpu/` | Metal-Szenen gegen Vulkan-Äquivalente (Q-PERF-3), Frame-Latenz (Q-PERF-4) |
| `baselines/<host-id>.json` | Eingecheckte Basislinien je Maschinenklasse (`ci-x86`, `ci-arm64`, `g5mf`) |
| `results/` | Lokale Ergebnisse, **nicht** einchecken (Eintrag in `.gitignore` ist nachzutragen) |

Mikro-Benchmarks einzelner Crates liegen im jeweiligen Crate (`crates/<c>/benches/`), nutzen aber dieselben
Namens- (`bench_<gegenstand>`) und Ausgabekonventionen.

## Regeln

* Immer im **Release-Profil**, mit festem Eingabekorpus und festem Seed.
* Jede Messung nennt ihre Q-PERF-ID. Ergebnis als JSON-Zeile: Name, Wert, Einheit, Streuung, Commit,
  Compiler-Version, CPU-Modell, Datum.
* **Deterministische** Metriken (Befehlszahlen, Codegröße) scheitern ab +2 %; **Wanduhrzeit** warnt auf geteilten
  Runnern ab +10 % und ist nur auf dem G5-MF-Runner verbindlich (siehe testing.md §8, §9).
* Basislinien werden nur per PR mit Vorher/Nachher-Tabelle geändert.
* Ergebnisse der Nacht-CI landen als Artefakt und im Datenbranch `bench-data`, nicht im Hauptbranch.

## Ausführen

Noch keine Benchmarks vorhanden (Stand M0). Geplant:

```bash
cargo bench -p ritsu-dbt                 # Mikro-Benchmarks eines Crates
make bench                               # alle Benchmarks (Ziel kommt mit dem ersten Benchmark)
```
