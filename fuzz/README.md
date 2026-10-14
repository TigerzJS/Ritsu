# fuzz/ – Fuzzing

Alles, was aus dem Gast kommt, ist feindlich (CLAUDE.md §3 Regel 2, Q-SEC-1). **Jeder Gast-Parser bekommt ein
Fuzz-Target** im selben Änderungssatz. Strategie, Invarianten und CI-Zeiten:
[docs/process/testing.md §5](../docs/process/testing.md).

## Layout

| Pfad | Inhalt |
|---|---|
| `Cargo.toml` | Eigenes Cargo-Paket mit eigenem `[workspace]` (kein Mitglied des Haupt-Workspace), von `cargo fuzz init` erzeugt |
| `fuzz_targets/fuzz_<parser>.rs` | Ein Target pro Parser, z. B. `fuzz_virtqueue_walk`, `fuzz_mmio_bus`, `fuzz_gpu_proto_decode`; nur der libFuzzer-Einstieg, leitet an `ritsu_fuzz::harness::<parser>::run` weiter |
| `src/harness/<parser>.rs` | Die Harness-Logik als gewöhnliche Funktion `run(&[u8])` (Eingabeformat, Invarianten, Modell); `harness::TARGETS` ordnet Zielnamen den Funktionen zu |
| `tests/replay.rs` | Stable-Test: spielt `regressions/` und `corpus/` durch die Harness-Funktionen ab, prüft Registry, Dateien und `[[bin]]`-Liste auf Übereinstimmung |
| `corpus/<target>/` | Kleiner, minimierter, selbst erzeugter Startkorpus (< 5 MB je Target, keine Apple-Dateien) |
| `regressions/<target>/` | Minimierte Absturzeingaben; werden von normalen Tests abgespielt |

Das C++-Gegenstück (libFuzzer für AIR-Container-Erkennung) liegt in `compiler/air2spirv/fuzz/`.

## Regressionen und Korpus ohne nightly abspielen

```bash
make fuzz-replay        # = cargo test --manifest-path fuzz/Cargo.toml (stable, Teil von `make test` und `make check`)
```

Jede Datei unter `regressions/<target>/` und `corpus/<target>/` läuft durch die Harness-Funktion ihres Targets; ein
Fehlschlag nennt die Datei, die Panic-Meldung und den Wiederholungsbefehl für `cargo +nightly fuzz run`. Ein
Verzeichnis ohne zugehöriges Target ist ein Fehler (seine Eingaben würden sonst nie abgespielt). Ein neues Target
braucht: `src/harness/<name>.rs` mit `pub fn run(data: &[u8])`, Eintrag in `harness::TARGETS` und `pub mod`,
`fuzz_targets/fuzz_<name>.rs` (drei Zeilen, siehe die vorhandenen), `[[bin]]` in `Cargo.toml` und die Liste in
`.github/workflows/fuzz.yml`.

## Ausführen (Fuzzing selbst)

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

## Targets (Stand WP-QA-01)

| Target | Parser | Eingabe |
|---|---|---|
| `fuzz_gpu_proto_decode` | `ritsu_gpu_proto::decode_stream` | rohe Bytes; zusätzlich Fixpunkt-Roundtrip des Encoders |
| `fuzz_gpu_proto_hello` | `decode_hello_stream`, `decode_session_stream` | rohe Bytes |
| `fuzz_gpu_proto_reply` | `decode_reply_stream` | rohe Bytes; Re-Encode-Roundtrip |
| `fuzz_gpu_host_session` | `ritsu_gpu_host::frontend_g::GuestSession<NullBackend>` | Optionen + Folge von Roh-Strömen nach gültigem HELLO; Antwortstrom muss immer dekodieren |
| `fuzz_virtqueue_walk` | `ritsu_virtio::queue::Queue` (`pop`, `push_used`, `should_notify`) | Byte-Format laut Kopf der Datei: Queue-Konfiguration + Folge von Gast-Schreibzugriffen und Geräteoperationen |
| `fuzz_mmio_bus` | `ritsu_platform::mmio::MmioBus` | Byte-Format laut Kopf der Datei: Registrierungen und Zugriffe, geprüft gegen ein Regionsmodell |
| `fuzz_aarch64_decode` | `ritsu_dbt::decode` | rohe Bytes als 32-Bit-Wörter |
| `fuzz_t0_step` | `ritsu_dbt::step` (T0) | 8 Byte Zustands-Header + Befehlswörter im RAM, max. 512 Schritte |

C++: `fuzz_air_sniff` in `compiler/air2spirv/fuzz/` (wird von `fuzz/tools/smoke_cpp.sh` gebaut und gestartet).

## Hilfsskripte

| Skript | Zweck |
|---|---|
| `tools/gen_seeds.py` | erzeugt den Startkorpus `corpus/<target>/` deterministisch |
| `tools/smoke.sh` | zeitbegrenzter Lauf der Rust-Targets (`make fuzz-smoke-rust`), Korpus bleibt unverändert |
| `tools/smoke_cpp.sh` | Build und zeitbegrenzter Lauf von `fuzz_air_sniff` (`make fuzz-smoke-cpp`) |

Neue Fuzz-Eingaben werden nie in `corpus/` geschrieben: `smoke.sh` nutzt ein Arbeitsverzeichnis (`FUZZ_WORK`).
Beim direkten Aufruf `cargo fuzz run <target>` ohne Pfad schreibt cargo-fuzz dagegen in `fuzz/corpus/<target>/`;
dann neue Dateien nicht einchecken, sondern erst mit `cmin` auswählen (`docs/process/testing.md §5.3`).

## Offene Funde

Siehe `regressions/<target>/`: jede Datei dort ist ein bekannter, noch nicht (oder gerade behobener) Fund, der von
`smoke.sh` bei jedem Lauf zuerst abgespielt wird. Solange ein Fund offen ist, scheitert der Smoke-Job des Targets
absichtlich (CLAUDE.md: kein Überspringen, um CI grün zu bekommen).
