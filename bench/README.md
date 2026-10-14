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
| `src/main.rs` | Kommandozeile `ritsu-bench` (inkl. Befehlszahl-Modus `--icount-*`) |
| `../tools/bench_icount.py` | Treiber der Befehlszahl-Messung (valgrind/callgrind), schreibt dasselbe Ergebnisformat |
| `baselines/<host-id>.json` | Eingecheckte Basislinien je Maschinenklasse (`ci-x86` Wanduhr, `icount-x86_64` Befehlszahlen) |
| `results/` | Lokale Ergebnisse, nicht eingecheckt (`bench/.gitignore`) |

Die Workloads liegen in der Bibliothek, nicht im Programm, damit `cargo test` sie prüft: Jedes Gast-Programm hat
von Hand hergeleitete Endzustände und Befehlszahlen, jeder Strom eine handgerechnete Größe, die Assembler-Kodierungen
wurden gegen `llvm-mc` geprüft. Ein Benchmark, dessen Arbeit nicht stimmt, misst nichts.

### Warum kein `criterion`

Der Harness (`harness.rs`, rund 100 Zeilen) macht dasselbe Verfahren: aufwärmen, die innere Schleife so
dimensionieren, dass eine Stichprobe etwa 100 ms dauert, 15 Stichproben, Median und MAD. `criterion` brächte rund 40
transitive Abhängigkeiten (Supply-Chain, MSRV, Registry-Zugriff), ein eigenes Ausgabeformat und HTML-Berichte, die
hier niemand braucht. Es fehlen dem Harness Ausreißer-Klassifikation und Regressionstests; die Entscheidung trifft
`tools/bench_compare.py`. Die bessere Antwort auf das Rauschen sind deterministische Befehlszahlen; sie
kommen nicht aus `iai-callgrind` (weitere Abhängigkeit), sondern aus `tools/bench_icount.py` auf demselben Harness, siehe
„Befehlszahlen“.

## Ausführen

```bash
make bench                                   # Release-Build, schreibt bench/results/dev.json, Tabelle auf stderr
make bench BENCH_HOST_ID=ci-x86              # Maschinenklasse setzen -> bench/results/ci-x86.json
make bench-compare BENCH_HOST_ID=ci-x86      # messen + gegen bench/baselines/ci-x86.json vergleichen
cargo run --release --manifest-path bench/Cargo.toml -- --filter t0_ --samples 30
cargo run --release --manifest-path bench/Cargo.toml -- --list
python3 tools/bench_compare.py --self-test   # Selbsttest des Vergleichsskripts (Teil von `make check`)
make bench-icount                            # Befehlszahlen (valgrind), schreibt bench/results/icount-x86_64.json, ~20 s
make bench-icount-compare                    # messen + gegen bench/baselines/icount-x86_64.json prüfen (Exit 1 bei > 2 %)
python3 tools/bench_icount.py --self-test    # Selbsttest des Befehlszahl-Treibers (Teil von `make check`)
```

Optionen von `ritsu-bench`: `--output FILE`, `--host-id ID` (oder `RITSU_BENCH_HOST_ID`), `--filter TEXT`,
`--samples N`, `--quick` (nur Rauchtest, Zahlen bedeutungslos), `--allow-debug` (Debug-Build, sonst Fehler), `--list`.
Der Seed für Nutzdaten kommt aus `RITSU_TEST_SEED` (Standard fest) und steht im Ergebnis; die Arbeitsmengen hängen
nicht vom Seed ab. Auf dem G5 MF vorher den Governor festlegen (`performance`) und `taskset` benutzen.

`make check` führt nur `make bench-check` aus (Selbsttests beider Skripte, Rauchlauf der Pipeline im Debug-Build mit
`--quick`, ein `--icount-run` ohne valgrind, Schema-Prüfung aller Ergebnisdateien und Basislinien), keine Messung.

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
| `icount_<name ohne bench_>` (je Wanduhr-Benchmark) | wie dort | Host-Befehle je Arbeitseinheit | **deterministisch**: nur mit `make bench-icount`, eigenes Dokument, siehe „Befehlszahlen“ |

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

## Befehlszahlen (deterministisches Gate, WP-QA-09)

Wanduhr taugt auf geteilten Runnern nicht als Gate (Spannweite der Lauf-Mediane 8 bis 66 %). Die Zahl der **ausgeführten
Host-Befehle** hängt nur vom Code, vom Compiler und von der libc ab, nicht von der Last der Maschine.
`tools/bench_icount.py` misst sie mit `valgrind --tool=callgrind` je Workload der Wanduhr-Suite (dieselben Closures, also
dieselbe Arbeit und derselbe `work_per_call`-Fingerabdruck) und schreibt ein Dokument im selben Format
(`kind: deterministic`, `better: lower`), das `tools/bench_compare.py` mit der 2-%-Grenze prüft.

**Methode.** `ritsu-bench --icount-run NAME --reps N` baut den Workload einmal auf und ruft die Closure `N`-mal; der Treiber
liest `Ir` (Summe der Befehle) für `N` und `2N` Aufrufe:

```
Ir(N) = S + N·c      Ir(2N) = S + 2N·c      c = (Ir(2N) − Ir(N)) / N      Wert = c / work_per_call
```

`S` (Prozessstart, Aufbau des Workloads, dynamischer Lader) kürzt sich heraus; nur der Kern zählt, im eingeschwungenen
Zustand (der erste Aufruf kann abweichen, siehe `fixed_ir`, nur Diagnose). `N` steht je Workload in `suite::icount_reps`
(Größenordnung 10^8 Host-Befehle je Schritt, damit der Lauf unter Callgrind kurz bleibt). Release-Profil mit
Zeilen-Debug-Info (`profile.release` in `bench/Cargo.toml`), Callgrind ohne Cache-/Branch-Simulation.

**Einheit.** Host-Befehle je Arbeitseinheit: je **Gastbefehl** für T0 (`host_insn/guest_insn`, ein früher Indikator für
Q-PERF-1), je Byte für die Protokoll-Decoder, je Block für die IR-Pässe, je pop+push-Paar für die Virtqueue. Zusätzlich
stehen `instr_per_call`, `ir_reps`, `ir_2reps`, `fixed_ir` im Ergebnis.

**Messwerte auf HEAD** (`c8eb895`, Code der Crates identisch mit `0d3c742`; Xeon 2,1 GHz, valgrind 3.22, glibc 2.39,
rustc 1.97.0), Basislinie `baselines/icount-x86_64.json`:

| Workload | Host-Befehle je Einheit | Einheit |
|---|---|---|
| `icount_t0_countdown` | 160,0002 | je Gastbefehl (`SUBS` + `B.NE`) |
| `icount_t0_mix` | 145,94 | je Gastbefehl (alle M0-Klassen) |
| `icount_ir_verify_cbz_{256,1024,4096}` | 1716 / 1727 / 1722 | je Block |
| `icount_ir_lower_cbz_4096` | 481,9 | je Block |
| `icount_gpu_proto_decode_{mixed,bulk,reply}` | 15,69 / 0,102 / 7,13 | je Byte |
| `icount_virtq_pop_push_{1,3}desc` | 430,3 / 566,8 | je pop+push-Paar |

**Wiederholbarkeit (gemessen).** Drei Aufrufe des Treibers mit je drei Wiederholungen (9 Messungen je Workload) liefern
bitgleiche Werte: Abweichung 0,00000 % bei allen elf Workloads. Unempfindlich sind sie auch gegen ein anderes Binärabbild
(kopierte Datei, andere Pfadlänge), 8 KiB zusätzliche Umgebungsvariablen und abgeschaltetes ASLR (`setarch -R`, bitgleich zur Basislinie). Nach einem Neubau aus sauberem Checkout zeigte sich als größte Abweichung 0,0006 % (`ir_verify_cbz_1024`, 20 von 3,5 Mio. Befehlen), alle anderen bitgleich. Der Treiber bricht ab, wenn
Wiederholungen eines Workloads um mehr als 0,5 % abweichen (`--max-spread`).

**Wovon die Zahl doch abhängt.**

* *CPU-Merkmale über die libc.* glibc wählt `memcpy`/`memset` beim Start nach CPUID und Cache-Größen (ifunc). Gemessen: Maskieren
  von AVX2/ERMS/FSRM verschiebt memcpy-lastige Workloads um 1 bis 28 % (T0 und Virtqueue nicht), SSSE3/SSE4 um weitere bis
  zu 23 %. Eine Basislinie von einer CPU würde auf einer anderen scheitern. Deshalb setzt der Treiber für die Kindprozesse
  feste `GLIBC_TUNABLES` (`LIBC_TUNABLES` im Skript: nur Masken, die jede x86-64-CPU nachvollziehen kann, und feste Schwellen).
  Mit diesen Tunables änderten weitere Masken und andere Merkmale die Zahlen um weniger als 0,002 %. Die CPU bleibt im
  Ergebnis vermerkt, wird aber bei reinen Befehlszahlen nicht bemängelt.
* *Compiler und libc-Version.* Andere `rustc`-Version (`meta.rustc`) oder glibc (`meta.config.libc`) ändern die Zahlen
  legitim. `bench_compare.py` nennt den Unterschied als Hinweis; mit `--on-toolchain-diff warn` werden Überschreitungen dann nur
  Warnungen (CI, das auf `stable` schwimmt). Mit gepinnter Toolchain (Vorschlag in „Grenzen“) entfällt das.
* *Der Code selbst.* Das ist der Zweck. Unabhängige Änderungen verschieben kleine Workloads durch Inlining um ein bis zwei
  Prozent (Beispiel: die Änderungen an `ritsu-dbt` im Arbeitsbaum verschoben `ir_lower` um +1,7 %, `ir_verify` um +0,03 %).

**Warum 2 % (und nicht enger).** Das Rauschen der Messung ist 0; die Untergrenze setzt der Code: Ein Gate bei 0,5 % würde bei
jeder Inlining-Entscheidung in einem Nachbarmodul feuern (siehe +1,7 % oben) und Basislinien-PRs ohne Erkenntnis erzwingen. 2 % ist
die Grenze für deterministische Größen aus testing.md §8, entspricht bei T0 rund drei Host-Befehlen je Gastbefehl und liegt
weit unter den Effekten, um die es bei Q-PERF-1 geht (die T0-Änderung im Arbeitsbaum kostet +49 %). Wenn sich nach einigen
Wochen zeigt, dass Zufallsdrift selten über 0,5 % liegt, kann die Grenze für T0 gesenkt werden (`--det-fail`).

**Beispiel (WIP von WP-DBT-02 gegen die Basislinie, `make bench-icount-compare`):**
`icount_t0_countdown 160 → 239 (+49,4 % FAIL)`, `icount_t0_mix 145,94 → 221 (+51,4 % FAIL)`, alle anderen ≤ +1,7 %.

**Grenzen.** Befehlszahlen sind kein Zeitmaß: Cache-Fehlzugriffe, Sprungvorhersage, Befehlsmischung (ein `div` zählt wie
ein `add`) und Speicherbandbreite fehlen; ein Gewinn an Befehlen kann ein Verlust an Zeit sein. Sie ersetzen die
Wanduhr-Messung auf dem G5 MF nicht, sondern fangen die Regressionen ab, die deterministisch sichtbar sind. `rep movs` zählt
unter Callgrind je Iteration; die Tunables vermeiden diese Varianten.

## Basislinien und CI

* `baselines/<host-id>.json` ist selbst ein Ergebnisdokument; erzeugt wird es mit
  `python3 tools/bench_compare.py --merge bench/baselines/<host-id>.json bench/results/run*.json` aus mindestens drei
  Läufen derselben Maschine. Änderung nur per PR mit Begründung und Vorher/Nachher-Tabelle.
* `ci-x86.json` ist **vorläufig**: gemessen auf dem Cloud-Entwicklungshost (Xeon @ 2,1 GHz), nicht auf einem
  GitHub-Runner. Der erste Nachtlauf zeigt „CPU differs“ und unterdrückt Wanduhr-Deltas; danach ist sie durch das
  Artefakt des Nachtlaufs zu ersetzen. `g5mf.json` gibt es erst, wenn der Runner steht.
* `icount-x86_64.json` (Befehlszahlen) gilt für die Maschinenklasse „x86-64 mit glibc 2.39 / rustc 1.97.0“, nicht für einen
  Rechner. Erzeugt aus drei Aufrufen mit `make bench-icount BENCH_ICOUNT_ARGS="--repeat 3" BENCH_ICOUNT_OUT=bench/results/runN.json`
  (N = 1…3) und `python3 tools/bench_compare.py --merge bench/baselines/icount-x86_64.json bench/results/run*.json`
  (der Merge verlangt bitgleiche Werte). Erzeugt auf HEAD in einem Scratch-Worktree, weil der Arbeitsbaum fremde, noch nicht
  committete Änderungen an `ritsu-dbt` enthielt (`dirty: true` in der Basislinie bezeichnet nur den damals ungecommitteten
  Harness). Anheben nur per PR mit Vorher/Nachher-Tabelle (`make bench-icount-compare` zeigt sie).
* [`.github/workflows/ci.yml`](../.github/workflows/ci.yml), Job `rust`: installiert valgrind und führt
  `make bench-icount-compare` aus (bei Push und PR; rund 10 s Bau + 15 s Messung, kein Zeitproblem). **Das ist das einzige Wanduhr-
  freie Gate und es scheitert verbindlich** ab +2 %; bei anderer Toolchain als in der Basislinie nur Warnung. Ergebnis als Artefakt und
  in der Job-Zusammenfassung.
* [`.github/workflows/bench.yml`](../.github/workflows/bench.yml): nachts (03:43 UTC) und manuell, **nicht** bei
  Push/PR. Misst `runs`-mal (Standard 3), bildet den Median, vergleicht, schreibt die Tabelle in die Job-Zusammenfassung
  und lädt `bench/results/` als Artefakt hoch (90 Tage). Der Datenbranch `bench-data` (testing.md §8) ist noch nicht
  angelegt.

## Grenzen und Schulden

* Wanduhr-Messungen in der Cloud sind Größenordnungen, keine Urteile. Die Zahlen hier sind nicht die des G5 MF.
* Befehlszahlen (WP-QA-09) sind umgesetzt, aber nur für x86-64 und eine glibc: Eine zweite Plattform (z. B. `ci-arm64`)
  braucht eine eigene Basislinie. Die Toolchain schwimmt auf `stable` (`rust-toolchain.toml`); mit einer festen Version würde der
  Toolchain-Hinweis zum Fehler und jede Compiler-Anhebung zu einem bewussten Basislinien-PR. Vorschlag an den Orchestrator
  (Wurzeldatei, nicht im Scope dieses Pakets).
* Die Befehlszahl-Workloads sind die der Wanduhr-Suite. T1/T2-Benchmarks bekommen ihren `icount`-Eintrag automatisch mit
  (`suite::icount_reps` ist für jeden Benchmark Pflicht, ein Test prüft das). Übersetzt ein JIT-Workload bei jedem Aufruf neu
  (kein Code-Cache), zählt die Übersetzung in `c` mit; bei einem Cache fällt sie in den ersten Aufruf und kürzt sich heraus.
* Fehlend: `bench/cpu` (Gast-Programme für Q-PERF-1 gegen native arm64-Werte), `bench/boot`, `bench/gpu`; T1/T2-Benchmarks
  folgen mit den JIT-Stufen.
