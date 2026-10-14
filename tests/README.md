# tests/ – übergreifende Tests

Hier liegt alles, was **mehr als ein Crate** betrifft oder Daten braucht, die nicht in ein Crate gehören.
Crate-eigene Tests bleiben im Crate (`#[cfg(test)]`-Module und `crates/<c>/tests/`). Die Gesamtstrategie steht in
[docs/process/testing.md](../docs/process/testing.md).

`tests/` ist (noch) kein Cargo-Paket. Kommt ein Test-Crate dazu, muss es in `Cargo.toml` und in der `ALLOWED`-Map von
`tools/check_crate_graph.py` (und ARCHITECTURE.md §4) eingetragen werden.

## Geplantes Layout

| Verzeichnis | Inhalt | Meilenstein |
|---|---|---|
| `dbt-ref/` | Generator für Differenztests aus den Kodierungstabellen, aarch64-Harness für die Referenzläufe, `masks.toml`, `golden/` (aufgezeichnete Referenzdaten) | M1 |
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
make test                 # alle Tests (Rust + C++)
RITSU_TEST_SEED=0x1234 cargo test -p ritsu-dbt diff_   # Differenztests mit festem Seed
cargo test --workspace -- --ignored                    # nur auf Runnern mit den nötigen Voraussetzungen
```
