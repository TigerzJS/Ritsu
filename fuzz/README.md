# fuzz/ – Fuzzing

Alles, was aus dem Gast kommt, ist feindlich (CLAUDE.md §3 Regel 2, Q-SEC-1). **Jeder Gast-Parser bekommt ein
Fuzz-Target** im selben Änderungssatz. Strategie, Invarianten und CI-Zeiten:
[docs/process/testing.md §5](../docs/process/testing.md).

## Layout

| Pfad | Inhalt |
|---|---|
| `Cargo.toml` | Eigenes Cargo-Paket mit eigenem `[workspace]` (kein Mitglied des Haupt-Workspace), von `cargo fuzz init` erzeugt |
| `fuzz_targets/fuzz_<parser>.rs` | Ein Target pro Parser, z. B. `fuzz_virtqueue_walk`, `fuzz_mmio_bus`, `fuzz_gpu_proto_decode` |
| `corpus/<target>/` | Kleiner, minimierter, selbst erzeugter Startkorpus (< 5 MB je Target, keine Apple-Dateien) |
| `regressions/<target>/` | Minimierte Absturzeingaben; werden von normalen Tests abgespielt |

Das C++-Gegenstück (libFuzzer für AIR-Container-Erkennung) liegt in `compiler/air2spirv/fuzz/`.

## Ausführen

Voraussetzung: `cargo install cargo-fuzz` und eine nightly-Toolchain (nur für Fuzzing; der Rest des Projekts nutzt `stable`).

```bash
cargo +nightly fuzz list
cargo +nightly fuzz run fuzz_virtqueue_walk -- -max_total_time=60   # Smoke-Lauf wie im PR
cargo +nightly fuzz run fuzz_virtqueue_walk fuzz/regressions/fuzz_virtqueue_walk/<datei>   # Einzeleingabe
cargo +nightly fuzz tmin fuzz_virtqueue_walk <artifact>             # Fund minimieren
```

## Fund-Prozess

1. Absturz minimieren und nach `regressions/<target>/` legen, Regressionstest (rot) schreiben.
2. Ursache an der Stelle beheben, an der Gast-Daten eintreten (fehlende Validierung), nicht das Symptom.
3. Test grün, Target läuft 5 Minuten ohne neuen Fund, Commit `fix(<crate>): …`.

Stand M0: noch keine Targets vorhanden; das Verzeichnis enthält nur dieses README.
