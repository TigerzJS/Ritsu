# tests/ – übergreifende Tests

Hier liegt alles, was **mehr als ein Crate** betrifft oder Daten braucht, die nicht in ein Crate gehören.
Crate-eigene Tests bleiben im Crate (`#[cfg(test)]`-Module und `crates/<c>/tests/`). Die Gesamtstrategie steht in
[docs/process/testing.md](../docs/process/testing.md).

`tests/` selbst ist kein Cargo-Paket. `tests/dbt-ref/` ist ein **eigenständiges** Paket mit eigenem `[workspace]` (wie
`fuzz/` und `bench/`): es steht nicht in der Wurzel-`Cargo.toml` und nicht im Crate-Graph, `make lint-aux` und
`make test` bauen es über `--manifest-path`. Käme ein Test-Crate in den Haupt-Workspace, müsste es in `Cargo.toml` und
in der `ALLOWED`-Map von `tools/check_crate_graph.py` (und ARCHITECTURE.md §4) eingetragen werden.

## Geplantes Layout

| Verzeichnis | Inhalt | Meilenstein |
|---|---|---|
| [`dbt-ref/`](dbt-ref/README.md) | **Vorhanden (WP-DBT-04):** Generator für Differenztests (Klassentabelle, Seed), `ritsu-ref-runner` (aarch64, ohne libc) für Referenzläufe auf Hardware/qemu, `golden/` (aufgezeichnete Referenzdaten), Vergleich T0 ↔ Golden. `masks.toml` folgt mit FP/NEON | M0/M1 |
| `elf/` | User-Mode-Linux-ELF-Programme (Quelltext, Bauanleitung, erwartete Ausgabe) | M1 |
| `gpu/` | Testszenen (`scenes/`), Referenzbilder von echter Apple-Hardware (`reference/`), Toleranzen, Bildvergleich | M3 |
| `fixtures/` | Eingecheckte Testdaten, je Unterverzeichnis mit `README.md` (Herkunft, Erzeugungsbefehl, Lizenz) | laufend |
| `boot/` | Boot-/Desktop-Smoke-Tests (nur HIL, braucht Nutzer-Image) | M4+ |

## Regeln

* **Clean Room (ADR-0008):** Keine Apple-Binärdateien, keine Apple-Shader, -Beispiele oder -Texturen. Fixtures sind
  eigener oder frei lizenzierter Inhalt. Referenzbilder zeigen **unsere** Szenen.
* Fixtures sind klein, reproduzierbar erzeugt und dokumentiert. Große oder wachsende Daten gehören nicht nach Git.
* Tests sind deterministisch; Zufall hängt an `RITSU_TEST_SEED` (siehe testing.md §4.5).
* Was auf einem Runner nicht läuft, wird als `#[ignore = "<Grund>"]` ausgewiesen, nie stillschweigend übersprungen.

## Ausführen

```bash
make test                 # alle Tests (Rust + C++), inkl. T0 gegen die Golden Files (dbt-ref-check)
RITSU_TEST_SEED=0x1234 cargo test -p ritsu-dbt diff_   # Differenztests mit festem Seed
cargo test --workspace -- --ignored                    # nur auf Runnern mit den nötigen Voraussetzungen
make dbt-ref-smoke                                     # frische Vektoren: Runner (qemu-user oder nativ arm64) gegen T0
RITSU_TEST_SEED=0x1234 make dbt-ref-fresh              # dito, ~118k Vektoren, fester Seed
```
