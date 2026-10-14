# bench/ – Benchmarks

Leistungsbehauptungen gelten nur mit einer Messung aus diesem Verzeichnis (CLAUDE.md §3 Regel 8). Die Ziele stehen in
[ARCHITECTURE.md §6](../docs/ARCHITECTURE.md) (Q-PERF-1 bis Q-PERF-4); Methodik und Schwellen in
[docs/process/testing.md §8](../docs/process/testing.md).

## Aufbau

`bench/` ist ein eigenes Cargo-Paket `ritsu-bench` mit eigenem `[workspace]` (wie `fuzz/`), **kein** Mitglied des
Haupt-Workspace. Gründe: Das Wurzel-`Cargo.toml` und die Crate-Grafik (`tools/check_crate_graph.py`,
ARCHITECTURE.md §4) bleiben unberührt, `cargo test --workspace` baut keine Benchmark-Programme, und parallel
arbeitende Agenten stoßen sich nicht an einer gemeinsamen Datei. Der Preis: `cargo fmt --all` und
`cargo clippy --workspace` sehen das Paket nicht; deshalb prüfen `make lint-aux` (fmt + clippy) und `make test-aux`
(Unit-Tests) es explizit, beide Teil von `make check`.

| Pfad | Inhalt |
|---|---|
| `src/harness.rs`, `stats.rs` | Messschleife (Aufwärmen, Kalibrierung, Stichproben) und Median/MAD |
| `src/t0.rs`, `a64.rs` | Gast-Programme für den T0-Interpreter und ein kleiner A64-Assembler (nur M0-Befehle) |
| `src/ir.rs` | IR-Region aus `CBZ`-Folgen (Verifier, Lowering) |
| `src/gpu.rs` | Strom-Generatoren für `decode_stream` / `decode_reply_stream` |
| `src/virtq.rs` | Virtqueue-Gerätseite (`Queue::pop` + `push_used`) |
| `src/suite.rs` | Liste der Benchmarks, Ergebnisdokument |
| `src/meta.rs` | Commit, Host-CPU (`/proc/cpuinfo`), Rust-Version, Konfiguration |
| `src/main.rs` | Kommandozeile `ritsu-bench` |
| `baselines/<host-id>.json` | Eingecheckte Basislinien je Maschinenklasse |
| `results/` | Lokale Ergebnisse, nicht eingecheckt (`bench/.gitignore`) |

Die Workloads liegen in der Bibliothek, nicht im Programm, damit `cargo test` sie prüft: Jedes Gast-Programm hat
von Hand hergeleitete Endzustände und Befehlszahlen, jeder Strom eine handgerechnete Größe, die Assembler-Kodierungen
wurden gegen `llvm-mc` geprüft. Ein Benchmark, dessen Arbeit nicht stimmt, misst nichts.

### Warum kein `criterion`

Der Harness (`harness.rs`, rund 100 Zeilen) macht dasselbe Verfahren: aufwärmen, die innere Schleife so
dimensionieren, dass eine Stichprobe etwa 100 ms dauert, 15 Stichproben, Median und MAD. `criterion` brächte rund 40
transitive Abhängigkeiten (Supply-Chain, MSRV, Registry-Zugriff), ein eigenes Ausgabeformat und HTML-Berichte, die
hier niemand braucht. Es fehlen dem Harness Ausreißer-Klassifikation und Regressionstests; die Entscheidung trifft
`tools/bench_compare.py`. `iai-callgrind` (deterministische Befehlszahlen) wäre die bessere Antwort auf das
Rauschen, siehe „Grenzen“; Valgrind ist hier installiert, der Harness lässt sich darauf aufsetzen.

## Ausführen

```bash
make bench                                   # Release-Build, schreibt bench/results/dev.json, Tabelle auf stderr
make bench BENCH_HOST_ID=ci-x86              # Maschinenklasse setzen -> bench/results/ci-x86.json
make bench-compare BENCH_HOST_ID=ci-x86      # messen + gegen bench/baselines/ci-x86.json vergleichen
cargo run --release --manifest-path bench/Cargo.toml -- --filter t0_ --samples 30
cargo run --release --manifest-path bench/Cargo.toml -- --list
python3 tools/bench_compare.py --self-test   # Selbsttest des Vergleichsskripts (Teil von `make check`)
```

Optionen von `ritsu-bench`: `--output FILE`, `--host-id ID` (oder `RITSU_BENCH_HOST_ID`), `--filter TEXT`,
`--samples N`, `--quick` (nur Rauchtest, Zahlen bedeutungslos), `--allow-debug` (Debug-Build, sonst Fehler), `--list`.
Der Seed für Nutzdaten kommt aus `RITSU_TEST_SEED` (Standard fest) und steht im Ergebnis; die Arbeitsmengen hängen
nicht vom Seed ab. Auf dem G5 MF vorher den Governor festlegen (`performance`) und `taskset` benutzen.

`make check` führt nur `make bench-check` aus (Selbsttest + Rauchlauf der Pipeline im Debug-Build mit `--quick`,
Schema-Prüfung aller Ergebnisdateien und Basislinien), keine Messung.

## Benchmarks (Stand WP-QA-04)

| Name | Q-PERF | Einheit | Misst |
|---|---|---|---|
| `bench_t0_countdown` | 1 | Minst/s | `ritsu_dbt::interp::step`: `SUBS` + `B.NE`, 500 000 Iterationen (1 000 002 Befehle) |
| `bench_t0_mix` | 1 | Minst/s | `step` auf einer Mischung aller M0-Klassen (MOVZ/MOVK, ADD/SUB imm, CMP, B.cond, CBZ/CBNZ, B, BL/RET, BR), 100 000 Iterationen |
| `bench_ir_verify_cbz_{256,1024,4096}` | 1 | µs | `Region::verify` auf Regionen aus 256/1024/4096 `CBZ` (4096 = eine 16-KiB-Seite = 8193 Blöcke); Messwert für WP-DBT-17 |
| `bench_ir_lower_cbz_4096` | 1 | µs | `Builder::lower_sequence` für dieselbe Seite |
| `bench_gpu_proto_decode_mixed` | 3 | MB/s | `decode_stream` auf 1 MiB kleiner Records (Render-/Compute-Frames) |
| `bench_gpu_proto_decode_bulk` | 3 | MB/s | `decode_stream` auf 1 MiB `WriteBuffer`-Records à 4 KiB |
| `bench_gpu_proto_decode_reply` | 3 | MB/s | `decode_reply_stream` (Host → Gast) |
| `bench_virtq_pop_push_{1,3}desc` | 2 | Mop/s | `Queue::pop` + `push_used`, Kette aus 1 bzw. 3 Deskriptoren, Queue der Größe 256 |
| `ir_cbz_{256,1024,4096}_{blocks,values,ops}` | 1 | Anzahl | **deterministisch**: Größe der erzeugten IR |

Hinweise zu den Zahlen: MB/s sind 10^6 Byte/s. T0 wird über `step` gemessen (Abruf, Dekodierung und Ausführung), die
Schleife darum zählt nur Befehle. `decode_stream` kopiert die Nutzdaten nicht (die Befehle borgen sie aus dem Strom),
deshalb ist der `bulk`-Wert eher eine Records-pro-Sekunde-Messung. Q-PERF-Zuordnung ist die nächstliegende, keine
der Messungen ist bereits ein Q-PERF-Messwert im Sinne der Tabelle in ARCHITECTURE.md §6 (dafür fehlen Gast-Programme
und Referenzwerte).

## Ergebnisformat

Ein JSON-Dokument je Lauf (`schema` 1), Objekte in fester Reihenfolge:

```json
{
  "schema": 1,
  "meta": {
    "created_utc": "2026-10-14T10:58:50Z", "host_id": "ci-x86",
    "commit": "<git rev-parse HEAD>", "dirty": false,
    "rustc": "rustc 1.97.0 (…)",
    "profile": { "name": "release", "opt_level": "3", "debug_assertions": false, "panic": "abort", "rustflags": "" },
    "cpu": { "model": "Intel(R) Xeon(R) …", "logical_cpus": 4, "avx2": true, "avx512f": false, "governor": null },
    "os": { "kernel": "6.18.44", "arch": "x86_64" },
    "config": { "quick": false, "warmup_ms": 300, "sample_ms": 100, "samples": 15, "filter": null, "seed": 1 }
  },
  "results": [
    { "name": "bench_t0_mix", "kind": "wall", "q_perf": "Q-PERF-1", "unit": "Minst/s", "better": "higher",
      "value": 108.3, "mad": 3.1, "spread_pct": 4.2, "min": 70.5, "max": 156.2,
      "samples": 15, "iters_per_sample": 9, "work_per_call": 1699993, "work_unit": "instructions" },
    { "name": "ir_cbz_4096_blocks", "kind": "deterministic", "q_perf": "Q-PERF-1", "unit": "blocks",
      "better": "lower", "value": 8193 }
  ]
}
```

`value` ist der Median der Stichproben, `mad` die rohe Median-Abweichung, `spread_pct = 100 · 1,4826 · MAD / Median`
(entspricht einer Standardabweichung bei Normalverteilung). `work_per_call` ist ein deterministischer Fingerabdruck der
Arbeit: ändert sie sich, ist die Basislinie ungültig, kein Befund über Geschwindigkeit. `dirty` zeigt an, dass
getrackte Dateien vom Commit abweichen (die Zahl ist dann nicht aus dem Commit reproduzierbar).

## Regressionsvergleich und Schwellen

`tools/bench_compare.py BASIS ERGEBNIS` (nur Standardbibliothek) setzt testing.md §8 um:

| Art | Schwelle | Wirkung |
|---|---|---|
| deterministisch | schlechter als +2 % | **Fehler** (Exit 1) |
| Wanduhr, geteilter Runner (`ci-x86`, `ci-arm64`, `dev`) | schlechter als +10 % | **Warnung** (Exit 0, GitHub-Annotation) |
| Wanduhr, `g5mf` oder `--strict-wall` | schlechter als +5 % | **Fehler** |
| `work_per_call` ungleich, Benchmark fehlt, Debug-/`--quick`-Ergebnis, andere `host_id` | – | **Fehler** (Vergleich ungültig) |
| andere CPU als in der Basislinie | – | Hinweis; auf `g5mf` Fehler; Wanduhr-Deltas werden unterdrückt |

„Schlechter“ folgt der Richtung der Metrik (`better`). Verbesserungen werden gemeldet, nie bemängelt; die Basislinie
wird dann per PR mit Vorher/Nachher-Tabelle angehoben.

**Warum diese Schwellen (Messung statt Raten).** Die Zahlen aus testing.md (2 / 10 / 5 %) gelten unverändert. Die
Messung auf dem Cloud-Host dieses Pakets (Xeon @ 2,1 GHz, 4 vCPU, geteilt, `steal` > 0) zeigt, warum Wanduhr auf
geteilten Runnern nie ein Gate sein darf: Zwischen fünf bzw. sechs vollständigen Läufen (zwei Serien) schwanken die Mediane einzelner
Benchmarks um 8 bis 66 % (Spannweite relativ zum Median), innerhalb eines Laufs streuen die Stichproben bis zu
27 % (1,4826·MAD). Eine 10-%-Warnung liegt damit im Rauschen und feuert gelegentlich ohne Ursache; sie ist als
Hinweis brauchbar, nicht als Urteil. Gegenmaßnahmen: Der Nachtjob misst dreimal und vergleicht den Median
(`--merge`), Basislinien entstehen ebenfalls aus mehreren Läufen, und der Vergleich nennt die Streuung neben jedem
Delta. Die deterministischen Größen (IR-Blöcke/-Werte/-Operationen) haben kein Rauschen; für sie sind 2 % großzügig.
Auf dem G5 MF (feste Frequenz, ein Kern, `taskset`) sind 5 % erst nach einer Messung der dortigen Streuung
verbindlich zu machen; das ist beim Aufbau des Runners (testing.md §9) nachzuholen.

## Basislinien und CI

* `baselines/<host-id>.json` ist selbst ein Ergebnisdokument; erzeugt wird es mit
  `python3 tools/bench_compare.py --merge bench/baselines/<host-id>.json bench/results/run*.json` aus mindestens drei
  Läufen derselben Maschine. Änderung nur per PR mit Begründung und Vorher/Nachher-Tabelle.
* `ci-x86.json` ist **vorläufig**: gemessen auf dem Cloud-Entwicklungshost (Xeon @ 2,1 GHz), nicht auf einem
  GitHub-Runner. Der erste Nachtlauf zeigt „CPU differs“ und unterdrückt Wanduhr-Deltas; danach ist sie durch das
  Artefakt des Nachtlaufs zu ersetzen. `g5mf.json` gibt es erst, wenn der Runner steht.
* [`.github/workflows/bench.yml`](../.github/workflows/bench.yml): nachts (03:43 UTC) und manuell, **nicht** bei
  Push/PR. Misst `runs`-mal (Standard 3), bildet den Median, vergleicht, schreibt die Tabelle in die Job-Zusammenfassung
  und lädt `bench/results/` als Artefakt hoch (90 Tage). Der Datenbranch `bench-data` (testing.md §8) ist noch nicht
  angelegt.

## Grenzen und Schulden

* Wanduhr-Messungen in der Cloud sind Größenordnungen, keine Urteile. Die Zahlen hier sind nicht die des G5 MF.
* Deterministische Befehlszahlen (`valgrind --tool=callgrind` je Benchmark, Differenz zweier Aufrufzahlen) würden ein
  verbindliches Gate auch auf geteilten Runnern erlauben. Noch nicht umgesetzt.
* Fehlend: `bench/cpu` (Gast-Programme für Q-PERF-1 gegen native arm64-Werte), `bench/boot`, `bench/gpu`; T1/T2-Benchmarks
  folgen mit den JIT-Stufen.
