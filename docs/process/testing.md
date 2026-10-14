# Teststrategie

> Status: **Entwurf v0.1** (WP-M0-QA). Verbindlich für alle Beiträge, auch von Agenten. Gilt zusammen mit
> [ARCHITECTURE.md](../ARCHITECTURE.md) (Qualitätsziele §6) und den ADRs
> [0006](../adr/0006-dbt-tiered-design.md) (DBT), [0007](../adr/0007-shader-translation-on-host.md) (Shader) und
> [0008](../adr/0008-clean-room.md) (Clean Room).

Ritsu übersetzt eine komplette Plattform (CPU und GPU) und verarbeitet ausschließlich feindliche Eingaben vom Gast.
Ein Fehler ist hier selten ein Absturz, sondern meistens **still falsches Verhalten** (ein Flag falsch, ein Pixel
falsch). Die Teststrategie ist deshalb auf drei Dinge ausgelegt: **Orakel** (gegen wen wird verglichen?),
**Reproduzierbarkeit** (jeder Fehlschlag lässt sich mit einem Befehl wiederholen) und **Ehrlichkeit** (was nicht
lauffähig ist, wird als *übersprungen* ausgewiesen, nie als bestanden).

## 1. Einstieg: Befehle

```bash
make check          # fmt-check + clippy -D warnings + alle Tests (Rust + C++) + cpp-fmt-check + Crate-Graph + Doku-Links
make test           # nur Tests (test-rust + test-aux + cpp)
make test-rust      # cargo test --workspace
make test-aux       # fuzz-replay + bench-test (die eigenständigen Pakete fuzz/ und bench/, beide auf stable)
make fuzz-replay    # fuzz/regressions/ und Seed-Korpus als normale Tests abspielen (kein nightly, kein cargo-fuzz)
make lint-aux       # fmt --check + clippy -D warnings für fuzz/ und bench/ (Teil von `make check`)
make bench          # Benchmark-Suite (Release) -> bench/results/<BENCH_HOST_ID>.json; nicht Teil von `make check`
make bench-compare  # bench + Vergleich mit bench/baselines/<BENCH_HOST_ID>.json (tools/bench_compare.py)
make bench-check    # Rauchtest der Benchmark-Pipeline + Selbsttest des Vergleichsskripts (Teil von `make check`)
make cpp            # air2spirv: CMake/Ninja-Build + ctest (überspringt mit Hinweis, solange das Projekt fehlt)
make cpp-fmt-check  # clang-format --dry-run --Werror (Teil von `make check`; überspringt sich, wenn clang-format fehlt)
make fuzz-smoke     # 60 s je Fuzz-Target (Rust: cargo-fuzz/nightly, C++: libFuzzer/clang); FUZZ_TIME=<s>; überspringt sauber, wenn Werkzeuge fehlen
make graph          # Abhängigkeitsregel (tools/check_crate_graph.py)
make links          # relative Links in *.md (tools/check_md_links.py)
cargo test -p ritsu-dbt -- --nocapture <filter>    # einzelner Test
```

`make check` muss vor jedem Commit grün sein (CLAUDE.md §4); es enthält `lint-aux`, `test-aux` (damit die Fuzz-Regressionen,
§5.3) und `bench-check`. Die CI (`.github/workflows/ci.yml`) ruft dieselben
`make`-Ziele auf. Es gibt keine Prüfung, die nur in der CI existiert.

## 2. Testpyramide

```
                         ▲  wenige, langsam, teuer
                        ╱ ╲
                       ╱HIL╲            Hardware-in-the-Loop: echte GPU, Boot-Tests (G5 MF, später)
                      ╱─────╲
                     ╱System ╲          User-Mode-ELF-Läufe, Bildvergleich, später Boot-Smoke
                    ╱─────────╲
                   ╱ Differenz ╲        T0↔T1↔T2↔Referenz, Shader-Ausgabe↔spirv-val/FileCheck
                  ╱─────────────╲
                 ╱  Fuzzing      ╲      jeder Gast-Parser, Invarianten statt Sollwerte
                ╱─────────────────╲
               ╱ Integration/Property╲  crates/*/tests/, proptest, Crate-übergreifend nur über öffentliche APIs
              ╱───────────────────────╲
             ╱        Unit-Tests         ╲   #[cfg(test)]-Module, ctest-Einheiten; Millisekunden, deterministisch
            ╱─────────────────────────────╲
            ▼  viele, schnell, billig
```

| Ebene | Wo | Läuft | Dauer-Budget | Zweck |
|---|---|---|---|---|
| Unit | `#[cfg(test)]` im Crate, C++: `compiler/air2spirv/tests/` | jeder Push (CI `rust`, `cpp`) | gesamt < 60 s | Logik einer Einheit, Randfälle |
| Integration / Property | `crates/<c>/tests/*.rs`, `proptest` | jeder Push | gesamt < 5 min | Zusammenspiel über die öffentliche API, Invarianten |
| Differenztests | `crates/ritsu-dbt/tests/diff_*.rs`, Referenzdaten in `tests/` | jeder Push (gegen eingecheckte Referenz), Nacht (frische Seeds) | < 10 min je Lauf | Übersetzung ≙ Referenz-CPU |
| Fuzzing | `fuzz/`, `compiler/air2spirv/fuzz/` | PR: Smoke (Minuten), Nacht: lang | siehe §5 | Absturzfreiheit, Speicherfehler, Hänger |
| GPU / Bildvergleich | `tests/gpu/` (Szenen, Referenzbilder) | CI mit Lavapipe, später G5 MF | < 10 min | Rendering-Korrektheit (Q-COR-2) |
| System | `tests/elf/`, später `tests/boot/` | Nacht / HIL | variabel | Ende-zu-Ende-Meilensteine |
| Benchmarks | `bench/` | Nacht / HIL, nie als Pass/Fail auf geteilten Runnern | – | Q-PERF-* |

**Regel:** Ein Fehler wird auf der **untersten** Ebene gefangen, auf der er sich ausdrücken lässt. Wer einen Fehler
im Differenztest oder per Fuzzing findet, schreibt zusätzlich einen Unit- oder Regressionstest (§5.5).

## 3. Rust-Tests: Konventionen

* Unit-Tests stehen im selben Datei-Modul (`#[cfg(test)] mod tests`). Integrationstests in `crates/<c>/tests/`.
* `unwrap`/`expect` sind in Tests erlaubt (`clippy.toml`), `dbg!` nicht.
* Tests sind **deterministisch**: keine Systemzeit, keine ungesetzten Zufallszahlen, keine Netzwerk- oder
  Dateisystemabhängigkeit außerhalb von `tempfile`-Verzeichnissen, keine Reihenfolgeabhängigkeit.
* Gast-Eingaben werden in Tests *auch* absichtlich feindlich gewählt (zu groß, zirkulär, ungültig ausgerichtet).
  Erwartet wird ein `ritsu_core::Error::Guest` oder eine Gast-Exception, nie ein `panic!` (CLAUDE.md §3 Regel 2).
* `proptest` für Invarianten (Roundtrip Encode/Decode, Idempotenz, Monotonie). Seeds: §4.5.
* Tests, die Hardware oder Software brauchen, die in der normalen CI fehlt, tragen `#[ignore = "<Grund>"]`
  (z. B. `#[ignore = "needs real Vulkan device"]`). Sie laufen auf dem passenden Runner mit `--ignored`.
  Ein Test darf **nie** stillschweigend `return`en, weil etwas fehlt.

### 3.1 Namenskonventionen

| Gegenstand | Muster | Beispiel |
|---|---|---|
| Rust-Testfunktion | `<einheit>_<situation>_<erwartung>` in `snake_case` | `ram_write_past_end_returns_guest_error` |
| Property-Test | Präfix `prop_` | `prop_decode_encode_roundtrip` |
| Differenztest | Präfix `diff_` + Befehlsklasse | `diff_add_sub_immediate_t0_t1`, `diff_ref_logical_shifted_reg` |
| Integrationstest-Datei | `crates/<c>/tests/<bereich>_<thema>.rs` | `tests/virtqueue_chain_limits.rs` |
| Regressionstest | Präfix `regress_` + Kennung des Fundes | `regress_fuzz_virtqueue_loop_2026_10_14` |
| Fuzz-Target | Präfix `fuzz_` + Parser | `fuzz_virtqueue_walk`, `fuzz_gpu_proto_decode` |
| Benchmark | Präfix `bench_` + Gegenstand | `bench_t1_block_translate` |
| ctest-Test (C++) | `air2spirv.<bereich>.<fall>` | `air2spirv.container.rejects_truncated_header` |
| Fixture-Verzeichnis | `tests/fixtures/<bereich>/<name>/` mit `README.md` (Herkunft, Erzeugungsbefehl, Lizenz) | `tests/fixtures/air/triangle_vs/` |
| Szenen (GPU) | `tests/gpu/scenes/<id>-<kurzname>/` | `0007-depth-test-less/` |

## 4. DBT: Differenztests

Q-COR-1 verlangt „0 bekannte Abweichungen in den Basis-ISA-Tests“. ADR-0006 legt die Orakel-Kette fest:

```
   echte arm64-Hardware ──(Referenz-Läufe aufgezeichnet)──► eingecheckte Referenzdaten
            │ (Orakel für T0)                                      │
            ▼                                                      ▼
        T0 Interpreter ──(Orakel für T1/T2)──► T1 Basis-JIT ◄──► T2 Cranelift-JIT
            ▲
   QEMU (qemu-aarch64) als zweites, unabhängiges Orakel bei Abweichungen / Zweifeln
```

Zwei Vergleichsarten, beide Pflicht für jede neue Befehlsklasse (CLAUDE.md §5):

1. **Referenzvergleich (T0 ↔ Hardware).** Beweist, dass die Semantik stimmt. Läuft in jeder CI gegen
   *aufgezeichnete* Referenzdaten und braucht dafür keine arm64-Maschine.
2. **Stufenvergleich (T0 ↔ T1 ↔ T2).** Beweist, dass die JIT-Stufen die Semantik des Interpreters nicht verändern.
   Läuft in-process auf dem x86-Host, ohne Referenzdaten, und damit auch mit unendlich vielen frischen Seeds.

### 4.1 Der Testfall

Ein Testfall ist ein Tripel **(Eingangszustand, Befehlsfolge, erwarteter Endzustand)**:

* **Eingangszustand:** X0–X30, SP, NZCV, FPCR/FPSR, V0–V31 (ab NEON), ein kleiner Datenpuffer (Speicher) an
  bekannter Adresse; X-Register, die als Zeiger dienen, zeigen in diesen Puffer.
* **Befehlsfolge:** meist **ein** Befehl (plus fester Vor- und Nachspann, der den Zustand sichert). Mehrbefehls-Folgen
  für Speicherordnung, exklusive Zugriffe und Verzweigungen.
* **Endzustand:** alle Register, Flags, der Datenpuffer, plus ein Signal (kein Fehler / Exception-Klasse /
  Syndrom). Gemessen wird *alles*, nicht nur das Zielregister. Nebenwirkungen auf andere Register sind genau die
  Fehler, die man sucht.

### 4.2 Erzeugung aus den Kodierungstabellen

Die Tests werden **generiert**, nicht von Hand geschrieben. Quelle sind die Kodierungstabellen des Decoders
(ADR-0006: eine gemeinsame Semantik-Quelle). Das garantiert, dass keine Befehlsklasse vergessen wird, und macht
Abdeckung messbar.

1. Ein Generator liest für jede Kodierungsklasse die Feldzerlegung (feste Bits, Registerfelder, Immediates,
   Optionen) aus der Tabelle.
2. Pro Klasse werden Befehlswörter erzeugt: **systematisch** (jedes Feld einzeln über seinen Wertebereich,
   Randwerte: Register 0/30/31 als SP bzw. XZR, Immediate-Minimum/Maximum, Shift-Mengen) und **zufällig**
   (Seed, §4.5).
3. Pro Befehlswort werden Operandenwerte mit **Randwert-Bias** gewürfelt: 0, 1, −1, `MIN`, `MAX`, Zweierpotenzen
   ±1, alternierende Bitmuster, bei Fließkomma NaN (still/signalisierend), ±∞, ±0, Denormale, Rundungsgrenzen.
4. Unzulässige oder UNPREDICTABLE-Kodierungen werden **nicht** auf Gleichheit geprüft. Die Tabellen markieren sie;
   der Generator erzeugt sie nur für den Test „T0 meldet Undefined Instruction bzw. verhält sich wie vom Handbuch
   erlaubt“. Implementierungsdefinierte Ergebnisse stehen in einer **Maskenliste** (`tests/dbt-ref/masks.toml`,
   je Eintrag mit Begründung und Quelle im Arm-Handbuch). Eine Maske ohne Begründung wird im Review abgelehnt.
5. Der Generator gibt zu jedem Lauf einen **Abdeckungsbericht** aus: Anzahl Kodierungsklassen mit/ohne Test.
   Ziel M1: 100 % der implementierten Klassen. Eine implementierte Klasse ohne Differenztest lässt `make check`
   (später: ein CI-Schritt) fehlschlagen.

Die Generatoren und Hilfsprogramme für die Referenzläufe liegen unter `tests/dbt-ref/` (siehe
[tests/README.md](../../tests/README.md)).

### 4.3 Referenz = echte arm64-Hardware

Die verbindliche Referenz ist **tatsächliche AArch64-Hardware**, nicht ein anderer Emulator.

* **Wo:** GitHub-gehostete Runner `ubuntu-24.04-arm` (derzeit Neoverse-basierte Server-Kerne; die genaue CPU wird
  in den Metadaten jeder Referenzdatei festgehalten, `/proc/cpuinfo` bzw. `MIDR_EL1`). Der Job `dbt-reference`
  in `.github/workflows/ci.yml` ist heute ein Platzhalter und wird in M1 zum Aufzeichnungs-Job.
* **Was läuft dort:** ein **statisch gelinktes aarch64-Linux-Testprogramm** ohne libc-Abhängigkeit
  (Harness in Rust `no_std` oder Assembler). Es bekommt eine Datei mit Testfällen (§4.1), führt sie einzeln aus
  (Befehlswort in eine ausführbare Seite schreiben, Eingangszustand laden, ausführen, Endzustand sichern),
  fängt Signale ab (`SIGILL`, `SIGSEGV`, `SIGBUS`, `SIGFPE`) und schreibt Ergebnisdatensätze.
  Statische Linkung macht die Läufe unabhängig von der Runner-Distribution.
* **Ausgabe:** ein kompaktes Binärformat (Kopf: Format-Version, Generator-Version, Master-Seed, Runner-CPU, Kernel;
  danach je Testfall Eingangs- und Endzustand). Der Kopf ist Teil der Reproduzierbarkeit.
* **Aufbewahrung:** Referenzdaten für die **feste** Regressions-Menge (Seed fest, Version gepinnt) werden
  eingecheckt (`tests/dbt-ref/golden/`, komprimiert). Jeder Push prüft T0 gegen diese Dateien, auf x86, ohne
  arm64-Maschine. Frische Zufalls-Seeds laufen nachts: der arm64-Job erzeugt Testfälle und Referenz, der x86-Job
  lädt beides als Artefakt und vergleicht.
* **Kosten/Verfügbarkeit:** Ob arm64-Runner für dieses Repository kostenlos nutzbar sind, hängt von der
  Repository-Sichtbarkeit ab (siehe Offene Fragen im WP-Bericht). Fallback: lokal reproduzierbar auf jedem
  arm64-Linux-Rechner (`make dbt-ref-record` ist für M1 geplant) und Referenz-Update per PR.

**Was die Hardware-Referenz nicht liefern kann:** Kernel-/EL1-Verhalten (Systemregister, MMU, Exceptions auf EL1),
und PAC-Schlüssel (Linux erlaubt Userland keine festen Schlüssel). Dafür gilt §4.6.

### 4.4 QEMU als zweites Orakel

`qemu-aarch64` (User-Mode) ist **kein** Ersatz für Hardware, sondern ein unabhängiges zweites Urteil:

* Differenz T0 ≠ Hardware: QEMU entscheidet nicht, die Hardware hat recht. Weicht QEMU ebenfalls ab, ist das ein
  Hinweis auf eine Mehrdeutigkeit im Handbuch → Maskenliste (mit Begründung) oder Rückfrage.
* Keine arm64-Hardware verfügbar (lokale Entwicklung): QEMU kann Referenzdaten für **Entwicklung** erzeugen. Sie
  werden mit `oracle = "qemu"` im Kopf markiert und **nie** als Golden-Datei eingecheckt.
* Bekannte QEMU-Abweichungen (Fließkomma-NaN-Propagation, Flags bei einzelnen SIMD-Befehlen) werden in
  `masks.toml` je Orakel geführt.

### 4.4a Decoder-Differenztest gegen `llvm-objdump` (Teil von WP-DBT-04)

Für den *Decoder* (nicht für die Semantik) gibt es ein weiteres unabhängiges Orakel: LLVMs AArch64-Disassembler
(`llvm-objdump --mattr=+all`, llvm-18 ist auf den Entwicklungs- und CI-Rechnern installiert; alternativ
`llvm-mc --disassemble`). Idee, Ausarbeitung in WP-DBT-04:

* Wörter aus den Kodierungstabellen (§4.2) plus Zufallswörter werden als rohe Befehlsfolge disassembliert.
* Jedes Wort, das `ritsu_dbt::decode` als Befehl erkennt, muss von LLVM als **gültiger** Befehl mit passendem
  Mnemonic und passenden Operanden gelesen werden (Abgleich über eine Normalisierung, nicht über Textgleichheit).
* Ein Wort, das LLVM als ungültig meldet (`<unknown>`), darf **nicht** als Befehl dekodiert werden.
* Ein Wort, das LLVM kennt, der Decoder aber als `Inst::Unknown` liefert, ist kein Fehler, sondern eine noch nicht
  implementierte Befehlsklasse: der Test zählt sie und führt sie in einer Liste (die mit dem Ausbau schrumpft),
  damit Lücken sichtbar bleiben (CLAUDE.md §3 Regel 5).
* LLVM ist nur ein Orakel für die **Dekodierung**. Hardware (§4.3) bleibt das Orakel für die Semantik. Weichen
  LLVM und Arm ARM voneinander ab, gilt das Arm ARM.

### 4.5 Seeds und Reproduzierbarkeit

* Alle Zufallsquellen (Generator, `proptest`, Fuzz-Eingabe-Mutation außerhalb von libFuzzer) leiten sich aus einem
  **Master-Seed** (`u64`) ab. Der Seed eines einzelnen Testfalls ist `hash(Master-Seed, Testname, Fallindex)`;
  das Hinzufügen eines Tests verschiebt deshalb nicht die Fälle der anderen.
* Setzbar über `RITSU_TEST_SEED=<u64>` (oder `0x…`). Ohne Angabe gilt in **Push/PR-CI ein fester Seed**
  (kein „grün heute, rot morgen“ durch Zufall), in **Nachtläufen ein frischer Seed**, der im Log und im
  Artefakt-Kopf steht.
* Jeder Fehlschlag druckt als **letzte Zeilen** einen vollständigen Wiederholungsbefehl, z. B.
  `RITSU_TEST_SEED=0x9e3779b97f4a7c15 cargo test -p ritsu-dbt diff_add_sub_immediate -- --exact`
  sowie das fehlschlagende Befehlswort (hex), Eingangszustand und beide Endzustände als Diff
  (nur abweichende Register).
* `proptest` schreibt Regressionsdateien (`proptest-regressions/`); sie werden **eingecheckt**.
* **Verkleinern:** Schlägt eine Mehrbefehls-Folge fehl, reduziert der Harness sie auf den kleinsten
  fehlschlagenden Befehl und die kleinste Menge abweichender Eingangsregister, bevor er ausgibt.
* Jeder gefundene Fehler wird als **fester Regressionstest** (`regress_…`) mit Befehlswort und Zuständen
  festgehalten, unabhängig vom Seed.

### 4.6 Sonderfälle

| Bereich | Vorgehen |
|---|---|
| Exklusive Zugriffe (`LDXR`/`STXR`) | Einzelthread-Semantik differenziell; ABA-Szenario und Erfolg/Misserfolg des Monitors als gezielte Tests (ARCHITECTURE §5.1: „das ABA-Problem wird getestet“). |
| Speicherordnung | Litmus-Tests (MP, SB, LB, 2+2W, IRIW, RCsc-Fall `STLR`→`LDAR`) als Mehrthread-Programme; erlaubte Ergebnismengen aus dem Arm-Speichermodell. Auf x86-Host mit vielen Wiederholungen, verbotene Ergebnisse müssen *nie* auftreten. |
| LSE-Atomics | Differenziell wie normale Befehle, zusätzlich Mehrthread-Stresstest (Zähler, Spinlock). |
| NEON / Fließkomma | Differenziell mit Randwert-Bias; FPCR-Rundungsmodi und FPSR-Flags gehören zum Zustand; Maskenliste für NaN-Payloads nur mit Begründung. |
| PAC (arm64e) | Hardware-Referenz scheidet für feste Schlüssel aus. Stattdessen: Eigenschaftstests (`aut(pac(p)) == p`, falscher Schlüssel/Modifier → Fehlerverhalten laut Handbuch), öffentliche Testvektoren des Algorithmus soweit verfügbar, und Stufenvergleich T0↔T1↔T2 mit fest gesetzten Schlüsseln. Ob die arm64-Runner `FEAT_PAuth` bieten, wird beim Aufbau von `dbt-reference` geprüft. |
| Systemregister, MMU, Exceptions | Kein Hardware-Orakel im Userland. Testen gegen das Handbuch als Spezifikation in Unit-Tests (`ritsu-mem` Stage-1-Walk mit handgebauten Tabellen), Fuzzing des Walkers (§5), später Boot-Tests. |
| Selbstmodifizierender Code | Gezielte Tests: Schreiben auf übersetzte Seite → Invalidierung → neu übersetzter Code gewinnt (alle drei Stufen). |

### 4.7 Meilenstein: User-Mode-Linux-ELF

Vor dem Kernel-Boot muss der DBT **statisch gelinkte aarch64-Linux-ELF-Programme im User-Mode** korrekt ausführen
(mit kleiner Syscall-Schicht im Testprogramm der Laufzeit, nur für diesen Test). Das ist der erste Ende-zu-Ende-Beweis
für Decoder + T0 + Speicher + (später) T1/T2.

* Stufen: `hello` (nur `write`/`exit`) → Rechen-/Sortier-Programme → kleine C-Programme mit musl →
  Mikro-Benchmarks (Q-PERF-1-Basis).
* Pass-Kriterium: **identische** Standardausgabe und Exit-Code wie bei nativer Ausführung auf dem arm64-Runner
  (gleiches Binary, dort aufgezeichnet).
* Die ELF-Programme sind **eigener oder frei lizenzierter Code**, im Repo als Quelltext plus Bauanleitung
  (`tests/elf/`); ggf. werden Binaries als Fixture eingecheckt, nie Apple-Artefakte (ADR-0008).
* Jede Stufe läuft mit T0 allein, dann T0=T1, dann T0=T1=T2 (Ausgabegleichheit über die Tiers).

## 5. Fuzzing

Alles, was aus dem Gast kommt, ist feindlich (CLAUDE.md §3 Regel 2, Q-SEC-1). **Jeder Gast-Parser bekommt ein
Fuzz-Target**, sonst ist er nicht „fertig“ im Sinne der Definition of Done.

### 5.1 Pflicht-Targets

| Target | Crate / Pfad | Eingabe | Invarianten |
|---|---|---|---|
| `fuzz_virtqueue_walk` | `ritsu-virtio` | beliebiger Gastspeicher + Ring-/Deskriptorzustand | kein Panic, terminiert (Kettenlängen-/Zirkelgrenze), keine Zugriffe außerhalb des Gastspeichers, kein unbegrenzter Speicherbedarf |
| `fuzz_mmio_bus` | `ritsu-platform` | Folge von (Adresse, Breite, Lesen/Schreiben, Wert) auf allen Geräten | kein Panic, keine Verletzung von Geräte-Zustandsinvarianten, Zugriffe außerhalb gültiger Register → definierte Antwort |
| `fuzz_gpu_proto_decode` | `ritsu-gpu-proto` | beliebige Bytes als Befehlsstrom | Decode liefert `Err`, nie Panic; Roundtrip `encode(decode(x))` konsistent für gültige Ströme; Längenfelder werden vor Allokation begrenzt |
| `fuzz_gpu_proto_hello` | `ritsu-gpu-proto` | beliebige Bytes als Bootstrap-/Sitzungsstrom | `decode_hello_stream` und `decode_session_stream` panikfrei; akzeptiertes HELLO überlebt Encode/Decode; Sitzungsstrom ist auch ein gültiger `decode_stream` |
| `fuzz_gpu_proto_reply` | `ritsu-gpu-proto` | beliebige Bytes als Antwortstrom (Host → Gast) | `decode_reply_stream` panikfrei; akzeptierte Antworten re-enkodieren zu identischen Antworten |
| `fuzz_gpu_host_session` | `ritsu-gpu-host` | gültiges HELLO, dann Fuzz-Bytes als weitere Ströme an `GuestSession<NullBackend>` | kein Panic; `StreamOutcome::reply_stream()` dekodiert immer fehlerfrei mit `decode_reply_stream` und liefert dieselben Antworten; Zähler bleiben innerhalb der Grenzen |
| `fuzz_gpu_host_commands` | `ritsu-gpu-host` | strukturierte Befehlsfolgen (arbitrary) gegen ein Null-Backend | Ressourcen-Lebensdauer, Handle-Validierung, keine Panics, keine Lecks |
| `fuzz_aarch64_decode` | `ritsu-dbt` | beliebige 32-Bit-Wörter | Decoder ist total; dekodierte Befehle re-enkodieren (wo definiert) identisch |
| `fuzz_t0_step` | `ritsu-dbt` | Registerzustand (Seed) + Befehlswörter im RAM, bis 512 Schritte | T0-`step` panikfrei für jedes Wort; `Err` lässt den Zustand unverändert; sequentieller Schritt erhöht `PC` um 4; Befehlsabruf verändert das RAM nicht |
| `fuzz_t0_vs_t1` | `ritsu-dbt` | Befehlswörter + Zustand | T0 und T1 (später T2) liefern gleichen Endzustand (Differenzfuzzing, keine Hardware nötig) |
| `fuzz_softmmu_walk` | `ritsu-mem` | beliebige Seitentabellen + Adresse | Walk terminiert, keine Zugriffe außerhalb des Gastspeichers, Fehler als Gast-Fault |
| `fuzz_air_sniff` (libFuzzer, C++) | `compiler/air2spirv/fuzz/` | beliebige Bytes als Bitcode-/`.metallib`-Container | `sniff()`/Parser gibt Fehlerstatus statt Absturz; unbekannte Records werden **gemeldet**, nie verworfen (ADR-0007); keine UB (ASan/UBSan) |
| `fuzz_air2spirv_translate` (später) | `compiler/air2spirv/fuzz/` | mutierte gültige AIR-Module | jede Ausgabe besteht `spirv-val`, sonst sauberer Fehler |

Neue Gast-Parser fügen ihr Target im **selben** Änderungssatz hinzu.

**Stand (WP-QA-01 / WP-PLAT-16):** Vorhanden und in der CI sind `fuzz_gpu_proto_decode`, `fuzz_gpu_proto_hello`, `fuzz_gpu_proto_reply`,
`fuzz_gpu_host_session`, `fuzz_virtqueue_walk`,
`fuzz_mmio_bus`, `fuzz_aarch64_decode` und `fuzz_t0_step` (Rust, `fuzz/`) sowie `fuzz_air_sniff` (C++).
Noch offen, weil der Parser bzw. das Crate fehlt: `fuzz_gpu_host_commands` (strukturierte Variante mit `arbitrary`;
`fuzz_gpu_host_session` deckt den Bytestrom-Pfad ab), `fuzz_t0_vs_t1` (braucht T1),
`fuzz_softmmu_walk` (Walker ist noch nicht fertig), `fuzz_air2spirv_translate`. Die Harnesses prüfen, wo möglich,
gegen ein unabhängiges Modell (virtio-1.2-Layout, MMIO-Regionsmodell) statt nur auf Panics.
Eingaben von `fuzz_virtqueue_walk`, `fuzz_mmio_bus`, `fuzz_t0_step` und `fuzz_gpu_host_session` folgen einem kleinen, im Kopf der jeweiligen
Datei dokumentierten Byte-Format statt `arbitrary`, damit Seeds von Hand erzeugt werden können.

### 5.2 Technik

* **Rust:** `cargo-fuzz` (libFuzzer, ASan). Das Verzeichnis `fuzz/` ist ein eigenes Cargo-Paket mit eigenem
  `[workspace]`-Abschnitt und **kein** Mitglied des Haupt-Workspace (cargo-fuzz verlangt nightly; die normale CI
  bleibt auf `stable`). Strukturbewusste Eingaben über `arbitrary`.
* **C++:** libFuzzer (`-fsanitize=fuzzer,address,undefined`) als optionales CMake-Target
  (`AIR2SPIRV_BUILD_FUZZERS=ON`), nur mit Clang.
* Die Harness-Logik jedes Targets steht als gewöhnliche Funktion `ritsu_fuzz::harness::<ziel>::run(&[u8])` in
  `fuzz/src/harness/`; `fuzz/fuzz_targets/fuzz_<ziel>.rs` leitet nur dorthin weiter. Dieselbe Funktion läuft unter
  libFuzzer (nightly) und in `cargo test` (stable, §5.3). Neues Target: Modul, Eintrag in `harness::TARGETS`,
  Datei in `fuzz_targets/`, `[[bin]]` in `fuzz/Cargo.toml` und CI-Matrix; `fuzz/tests/replay.rs` schlägt fehl, wenn diese
  Listen auseinanderlaufen oder ein Korpus-/Regressionsverzeichnis keinem Target gehört.
* Die Fuzz-Funktionen rufen dieselbe öffentliche Einstiegsfunktion wie der Produktionscode. Es gibt keine
  „Fuzz-only“-Pfade im Parser.
* Harte Grenzen gegen Hänger und Speicherexplosion: libFuzzer-`-timeout` und `-rss_limit_mb` sind gesetzt; ein
  Timeout ist ein Fund wie ein Absturz.

### 5.3 Korpus

* Startkorpus pro Target in `fuzz/corpus/<target>/` (Rust) bzw. `compiler/air2spirv/fuzz/corpus/<target>/` (C++):
  **klein, minimiert (`cmin`) und selbst erzeugt** (Seeds aus Unit-Tests, handgebaute gültige Strukturen,
  eigene Fixtures). Keine Apple-Dateien (ADR-0008). Gesamtgröße je Target in Git < 5 MB.
* Die Seeds erzeugt `python3 fuzz/tools/gen_seeds.py` deterministisch aus den Formatbeschreibungen (nicht aus
  Fremddaten). Ändert sich ein Eingabeformat oder die Wire-Spezifikation, wird das Skript angepasst und der
  Korpus neu erzeugt.
* Der große, wachsende Korpus der Nachtläufe lebt **nicht** in Git, sondern im GitHub-Actions-Cache bzw. als
  Artefakt, und wird nach jedem Lauf minimiert; nützliche neue Einträge werden gelegentlich per PR in den
  Git-Korpus übernommen.
* Abstürze (`artifact-*`) werden minimiert (`cargo fuzz tmin`), als Regressionseingabe unter
  `fuzz/regressions/<target>/` eingecheckt und von einem normalen `cargo test` abgespielt (Regressionstest, §3.1).
  Erst danach wird der Fehler behoben.
* **Abspielen (WP-QA-08):** `make fuzz-replay` (= `cargo test --manifest-path fuzz/Cargo.toml`, Teil von `make test`
  und `make check`, läuft in der `rust`-CI auf stable) schickt jede Datei unter `fuzz/regressions/<target>/` und
  `fuzz/corpus/<target>/` durch die Harness-Funktion ihres Targets. Ein Fehlschlag nennt Datei, Panic-Meldung und den
  Befehl `cargo +nightly fuzz run <target> <datei>`. Der Test beweist seine Wirkung: Mit dem Fix von `MmioBus`
  (Basisausrichtung) wieder entfernt, schlägt die Regression `fuzz_mmio_bus/unaligned_base_offset_misaligned` rot fehl.

### 5.4 Fuzzing in der CI

| Wann | Was | Dauer |
|---|---|---|
| Jeder PR | **Smoke-Fuzz** je Target gegen den Git-Korpus plus Regressionseingaben | 60 s je Target (parallel in einer Matrix) |
| Nachts | Langlauf je Target mit wachsendem Korpus | 30 min je Target (anfangs; Ziel M6: dauerhaft, Q-SEC-1) |
| Vor Release | Langlauf mit Coverage-Auswertung | mehrere Stunden |

Die Jobs stehen in [`.github/workflows/fuzz.yml`](../../.github/workflows/fuzz.yml) (Trigger: `pull_request`,
`schedule` täglich 02:17 UTC, `workflow_dispatch` mit Eingabe `minutes`):

| Job | Inhalt |
|---|---|
| `targets` | Prüft, dass Matrix, `[[bin]]`-Liste in `fuzz/Cargo.toml` und `fuzz/fuzz_targets/` übereinstimmen (ein neues Target kann nicht vergessen werden). |
| `rust` (Matrix je Target) | `dtolnay/rust-toolchain@nightly`, `cargo install cargo-fuzz`, dann `fuzz/tools/smoke.sh <target>`: liest Regressionen und Seed-Korpus, schreibt neue Eingaben in ein Arbeitsverzeichnis (der Git-Korpus bleibt unverändert), `-timeout=5 -rss_limit_mb=2048`. Nachts zusätzlich: Korpus aus dem Actions-Cache, danach `cargo fuzz cmin` und Speichern. |
| `cpp` | `make fuzz-smoke-cpp`: baut `fuzz_air_sniff` mit clang (`-DAIR2SPIRV_BUILD_FUZZERS=ON`, ASan/UBSan) und lässt es laufen; nachts mit Cache und `-merge=1`. |

Ein Absturz, Timeout oder OOM lässt den Job scheitern; `fuzz/artifacts/<target>/` wird 30 Tage als Artefakt
(`fuzz-artifacts-<target>`) hochgeladen. In der CI ist `FUZZ_REQUIRED=1` gesetzt: fehlende Werkzeuge sind ein Fehler,
kein stilles Überspringen. Lokal: `make fuzz-smoke` (überspringt sauber, wenn nightly/cargo-fuzz bzw. clang fehlen),
`make fuzz-smoke FUZZ_TIME=10`, `make fuzz-smoke-rust FUZZ_TARGETS=fuzz_mmio_bus`. Der ASan/UBSan-Build der
normalen C++-Tests läuft in `ci.yml` als eigener Matrixeintrag (`clang (ASan+UBSan)`).

### 5.5 Fund-Prozess

1. Absturz minimieren, Eingabe als `fuzz/regressions/…` einchecken, Regressionstest hinzufügen (rot).
2. Ursache beheben (nicht nur das Symptom: fehlende Validierung an der Stelle, an der Gast-Daten eintreten).
3. Test grün, Target läuft 5 min ohne neuen Fund. Commit mit `fix(<crate>): …` und Verweis auf den Fund.

## 6. GPU-Tests

Q-COR-2 verlangt Bildvergleich gegen **echte Apple-Hardware** mit Toleranz pro Test. Das ist in der Cloud-CI nicht
vollständig möglich (keine Apple-Hardware, kein echtes Vulkan-Gerät). Deshalb gibt es drei Stufen mit
unterschiedlicher Aussagekraft.

### 6.1 Stufen

| Stufe | Läuft wo | Gerät | Aussage |
|---|---|---|---|
| G0: Logik | jede CI | keins (Null-Backend) | Ressourcenverwaltung, Zustandsverfolgung, Synchronisationsgraph, Protokoll, Fähigkeitsmatrix |
| G1: funktional | CI (`ubuntu-24.04`) | **Mesa Lavapipe** (CPU-Vulkan, Paket `mesa-vulkan-drivers`) | Vulkan-Aufrufe sind gültig, Shader laufen, Bilder entsprechen der Referenz innerhalb der Toleranz |
| G2: Hardware | Self-hosted G5 MF (später) | RTX 4050 (NVIDIA), Iris Xe (ANV) | Treiber-/Backend-Verhalten, Leistung, Präsentation, PRIME |

Lavapipe wird über `VK_DRIVER_FILES` (bzw. `VK_ICD_FILENAMES`) explizit ausgewählt, damit ein zufällig installiertes
anderes Gerät keinen Test verfälscht. Die Gerätewahl steht im Testlog. Lavapipe ist ein vollständig
konformer Software-Rasterizer: er taugt für **Korrektheit**, nie für Leistungsaussagen.

### 6.2 Validierungsschichten sind Fehler

Alle GPU-Tests laden `VK_LAYER_KHRONOS_validation` (inklusive Synchronisations-Validierung, wo verfügbar). Der
Debug-Messenger zählt Meldungen; **jede Meldung der Stufe `ERROR` oder `WARNING` lässt den Test fehlschlagen**. Eine
Ausnahme ist nur mit begründetem Eintrag in einer Testliste erlaubt (Meldungs-ID, Begründung, Verweis auf Issue),
nie mit einem pauschalen Filter. Fehlt die Schicht in der CI, schlägt der Job fehl (kein stilles Weglassen).

### 6.3 Bildvergleich

* **Eigene Testszenen.** Szenen (Geometrie, Shader, Zustände) schreiben wir selbst (`tests/gpu/scenes/`); die
  Metal-Quellen sind unser Code. Damit sind auch die gerenderten Referenzbilder unsere eigenen Daten. Es werden
  keine Apple-Beispielprogramme, -Shader oder -Texturen verwendet (ADR-0008, Punkt 4).
* **Referenzbilder von echter Apple-Hardware.** Jede Szene wird einmalig auf einem echten Mac mit Metal gerendert
  (ein kleines, eigenes Harness-Programm, nicht Teil der CI) und als verlustfreies PNG (bzw. Rohdaten bei
  HDR/Integerformaten) abgelegt: `tests/gpu/reference/<szene>/<format>.png`, daneben `meta.toml` mit
  Szene-Kennung, Gerät (Apple-GPU-Familie), macOS-Build, Datum, Harness-Version.
* **Toleranz pro Test**, nicht global. `tests/gpu/scenes/<id>/scene.toml` legt Metrik und Schwellen fest:
  * Compute/Integer/exakte Operationen: **bitgleich**.
  * Rasterisierung ohne Antialiasing: Anteil abweichender Pixel ≤ x % und maximale Kanaldifferenz ≤ ε.
  * Antialiasing, Filterung, Fließkomma-Formate: maximale Kanaldifferenz und ein Wahrnehmungsmaß (z. B. SSIM/FLIP).
  Jede Lockerung wird begründet (Rundungs- und Rasterregeln weichen zwischen GPUs legitim ab).
* **Ausgabe bei Fehlschlag:** Ist-Bild, Soll-Bild und Differenzbild werden als Testartefakt abgelegt (CI lädt sie
  hoch), plus die Metrikwerte im Log.
* **Zweite Ebene: Regressions-Golden.** Zusätzlich zu den Apple-Referenzen hält die CI pro Szene ein Lavapipe-Golden,
  um Änderungen *im eigenen Code* zu erkennen (enge Toleranz). Es wird bei bewusster Änderung per PR aktualisiert und
  ersetzt nie die Apple-Referenz.
* **Reihenfolge der Wahrheit:** Apple-Referenz > Handbuch/Metal-Spezifikation > Lavapipe-Golden.

### 6.4 Was nicht in der CI läuft

| Nicht in CI möglich | Behandlung |
|---|---|
| Apple-Referenzbilder erzeugen (kein Mac) | Referenzen sind eingecheckte Fixtures; Erneuerung durch einen Menschen mit Mac, Verfahren in `tests/gpu/README.md` (WP für M3). Bis eine Referenz existiert, ist die Szene `#[ignore = "no Apple reference yet"]` und taucht in der Testzusammenfassung als **übersprungen** auf. |
| Echte GPU-Treiber (NVIDIA/ANV), PRIME-Ausgabe, Präsentationslatenz | Self-hosted G5 MF (§9). Bis dahin im Bericht „nicht ausführbar hier“ (CLAUDE.md §4). |
| Boot eines echten macOS-Images (keine Apple-Binärdateien im Repo) | Nur manuell/HIL durch den Nutzer; die CI testet Plattformgeräte einzeln (MMIO-Fuzzing, Geräte-Unit-Tests, Linux-Gast-Smoke, sobald vorhanden). |
| `guest/` bauen (macOS-SDK) | Nicht in Linux-CI; nur Quell-Lint, falls Variante G gewählt wird. |

Grundsatz: **Was nicht läuft, wird sichtbar übersprungen**, nie grün gemeldet. Zusammenfassungen der CI weisen
`ignored`-Tests mit Grund aus.

## 7. Shader-Compiler-Tests (`compiler/air2spirv`)

* **`spirv-val` ist Pflicht.** Jedes vom Compiler erzeugte SPIR-V-Modul wird in jedem Test mit `spirv-val`
  (Ziel-Umgebung Vulkan 1.3 / SPIR-V 1.6) validiert, in Debug-Builds auch zur Laufzeit (ADR-0007). Ein Test,
  der SPIR-V erzeugt, ohne es zu validieren, ist ein Review-Befund. Fehlt `spirv-val` in der CI, schlägt der Job fehl.
* **FileCheck-artige Tests.** Eingabe: AIR-Fixture (siehe unten). Der Test ruft den Compiler, disassembliert mit
  `spirv-dis` und prüft den Text gegen `CHECK:`-Zeilen im Fixture (LLVM `FileCheck`, eingebunden über ctest).
  Geprüft werden Struktur und Semantik-Merkmale (Capabilities, Decorations, Bindings, Blockstruktur), nicht
  zufällige Ids.
* **Fixtures:** Eigene Metal-Shader (`*.metal`), einmalig auf einem Mac zu AIR kompiliert und samt Quelltext,
  Compiler-Version und Erzeugungsbefehl eingecheckt (`tests/fixtures/air/…`, ADR-0007). Keine Apple-Shader.
* **Negativ-Tests:** abgeschnittene Container, unbekannte Records/Intrinsics, zu neue Bitcode-Version. Erwartet:
  Fehlerstatus mit Beschreibung (ADR-0007: Unbekanntes wird *gemeldet*, nie still verworfen).
* **Ende-zu-Ende:** SPIR-V → `ritsu-gpu-host` → Lavapipe → Bildvergleich (§6.3), sobald beide Seiten stehen.
* **Sanitizer:** ASan/UBSan-Konfiguration des C++-Tests (Clang, `-DAIR2SPIRV_SANITIZE=ON`) ist ein Eintrag der
  C++-Matrix in `ci.yml`; das 60-s-Smoke-Fuzzing von `fuzz_air_sniff` läuft in `fuzz.yml` (§5.4).
* **Format:** `make cpp-fmt-check` (clang-format) ist Teil der CI; `clang-tidy` (von [ADR-0003](../adr/0003-languages.md) verlangt) fehlt noch.
  Folgearbeit **WP-QA-07 clang-tidy**: `.clang-tidy` in `compiler/` anlegen, `make cpp-tidy` und CI-Schritt
  ergänzen; bewusst nicht Teil von WP-QA-01, weil die Shader-Spur dort parallel arbeitet.

## 8. Benchmarks (`bench/`)

Leistungsbehauptungen gelten nur mit Messung (CLAUDE.md §3 Regel 8). Benchmarks sind **keine** Pass/Fail-Tests auf
geteilten Runnern (zu verrauscht), aber sie haben **Regressionsschwellen**.

* **Struktur:** `bench/` ist ein eigenes Cargo-Paket `ritsu-bench` mit eigenem `[workspace]` (wie `fuzz/`, kein
  Mitglied des Haupt-Workspace; `make lint-aux` und `make test-aux` prüfen es). Der Harness ist eigener Code (std only:
  Aufwärmen, Kalibrierung, Stichproben, Median/MAD) statt `criterion`: keine neue Abhängigkeit, eigenes Ergebnisformat.
  Spätere System-Benchmarks (`bench/boot`, `bench/cpu`, `bench/gpu`) sind eigene Programme. Details, Liste der
  Benchmarks und Begründungen: [bench/README.md](../../bench/README.md).
* **Zwei Messarten:**
  * *Deterministisch* (Größen erzeugter IR, **Host-Befehlszahlen unter Valgrind** (`make bench-icount`, s. u.), später
    Codegröße, Zahl erzeugter Host-Befehle je Gastblock, Cache-Trefferquote): **scheitert** ab **+2 %** gegenüber der
    Basislinie.
  * *Wanduhrzeit* (Durchsatz, Latenz): auf geteilten Runnern nur **Warnung** ab **+10 %**; verbindlich
    (Fehler ab +5 %) nur auf dem self-hosted G5-MF-Runner mit festgelegter Frequenz/Governor.
  Jede Wanduhr-Messung trägt `work_per_call`, einen deterministischen Fingerabdruck der Arbeit; weicht er von der
  Basislinie ab, ist der Vergleich ungültig (Fehler), nie eine Beschleunigung.
* **Rauschen (gemessen):** Auf einem geteilten Cloud-Host (4 vCPU) streuen die Mediane zwischen fünf bzw. sechs vollständigen
  Läufen um 8 bis 66 % (Spannweite relativ zum Median). Die +10 %-Warnung ist dort ein Hinweis, kein Urteil; deshalb vergleicht der Nachtjob den Median
  mehrerer Läufe, und nur deterministische Metriken können auf Cloud-Runnern scheitern.
* **Befehlszahlen (WP-QA-09, das Gate für geteilte Runner):** `tools/bench_icount.py` misst je Workload der Wanduhr-Suite die
  ausgeführten Host-Befehle mit `valgrind --tool=callgrind`: Differenz der Zählungen für N und 2N Aufrufe der
  Workload-Closure (Start- und Aufbaukosten kürzen sich heraus), geteilt durch `work_per_call`, also Host-Befehle je
  Gastbefehl (T0, früher Indikator für Q-PERF-1), je Byte, je Block, je Queue-Operation. Gemessene Wiederholbarkeit 0,00000 %
  (neun Messungen je Workload bitgleich); damit ist 2 % ein Gate, das auf Cloud-Runnern nie aus Rauschen feuert. Bedingungen
  für diese Eigenschaft: Release-Profil, feste `GLIBC_TUNABLES` (sonst verschiebt die CPU-abhängige memcpy-Auswahl der libc
  die Zahlen um bis zu 28 %), Basislinie je Maschinenklasse (`icount-x86_64`) mit eingetragener rustc- und glibc-Version.
  Weicht die Toolchain ab, nennt `bench_compare.py` das; die CI läuft dann mit `--on-toolchain-diff warn`. Das Gate läuft im
  Job `rust` von `ci.yml` bei jedem Push/PR (rund 25 s). Details, Messwerte und Begründung der 2 %:
  [bench/README.md](../../bench/README.md#befehlszahlen-deterministisches-gate-wp-qa-09).
* **Basislinien:** `bench/baselines/<host-id>.json`, eingecheckt, Aktualisierung nur per PR mit Begründung und
  Vorher/Nachher-Tabelle im PR-Text. Die `host-id` benennt Maschinenklasse (`ci-x86`, `g5mf`, `ci-arm64`). Erzeugt aus
  mehreren Läufen mit `tools/bench_compare.py --merge`. Eine Basislinie einer anderen CPU erzeugt auf Cloud-Runnern
  einen Hinweis (Wanduhr-Deltas unterdrückt), auf `g5mf` einen Fehler.
* **Ergebnisse speichern:** Jeder Lauf schreibt **ein JSON-Dokument** (`schema` 1) mit `meta` (Commit, `dirty`,
  Rust-/Compiler-Version, Profil, CPU-Modell aus `/proc/cpuinfo`, Governor, Kernel, Konfiguration, Seed, Datum) und
  `results` (je Messung: Name, Art, Q-PERF-ID, Einheit, Richtung, Median, MAD, Streuung, Min/Max, Stichproben). Die
  früher geplante „JSON-Zeile je Messung“ entfällt zugunsten eines Dokuments, das Meta einmal trägt;
  `jq -c '.results[]'` liefert die Zeilen. Der Nachtjob [`bench.yml`](../../.github/workflows/bench.yml) lädt das
  Dokument als Artefakt hoch; der Datenbranch `bench-data` (kein Wachstum im Hauptbranch) ist noch nicht angelegt.
  Daraus entsteht der Verlauf pro Metrik (Q-PERF-1…4).
* **CI:** Wanduhr nur nachts und manuell (`bench.yml`), nie bei Push/PR. Befehlszahlen bei Push/PR (`ci.yml`, Job `rust`,
  `make bench-icount-compare`). `make check` führt nur den Rauchtest der Pipeline aus (`make bench-check`: Selbsttests der
  beiden Skripte, Debug-Lauf `--quick`), keine Messung; `make bench-icount-compare` braucht valgrind.
* **Meilenstein-Zielwerte** stehen in [ARCHITECTURE.md §6](../ARCHITECTURE.md); jede Messung bezieht sich auf
  eine Q-PERF-ID.
* Benchmarks laufen im Release-Profil (der Läufer verweigert Debug-Builds), mit festem Eingabekorpus und festem Seed
  (`RITSU_TEST_SEED` überschreibt ihn; die Arbeitsmengen hängen nicht vom Seed ab).

## 9. Hardware-in-the-Loop (später)

Ein **self-hosted Runner** auf dem Gigabyte G5 MF (Labels `self-hosted`, `linux`, `x64`, `g5mf`, `vulkan`) führt
aus, was Cloud-Runner nicht können:

* GPU-Stufe G2: echte Vulkan-Geräte (NVIDIA, Intel), Bildvergleich, Validierungsschichten, Präsentation.
* Verbindliche Wanduhr-Benchmarks (§8) bei festem Energieprofil.
* Boot- und Desktop-Tests (Q-PERF-2/4), sobald ein Nutzer-Image vorliegt.

Sicherheitsregeln: Der Runner führt **nur Workflows von vertrauenswürdigen Branches** aus (nie von Fork-PRs, nie
`pull_request_target` mit fremdem Code). Er hält keine Apple-Binärdateien in Workspace-Verzeichnissen, die als
Artefakt hochgeladen werden können; Logs und Artefakte werden vor dem Hochladen auf Pfade außerhalb von
`/vm/` und `/cache/` beschränkt. Der Workflow dafür kommt mit M3/M4, nicht vorher.

## 10. Flaky Tests

* **Keine Wiederholungen, die Fehler verstecken.** Kein `retry`, kein „zweiter Versuch grün = grün“, weder in der
  CI noch im Testcode.
* **Keine Quarantäne.** Kein Mechanismus, der einen Test dauerhaft aus dem Pflichtlauf nimmt, ohne ihn zu beheben.
  `#[ignore]` ist nur für *fehlende Voraussetzungen* erlaubt (§3), nicht für Instabilität.
* **Ursache beheben.** Ein Test, der einmal ohne Codeänderung rot und dann grün wird, ist ein Fehler (im Test oder
  im Produktionscode, bei einem JIT/Mehrthread-System oft im Produktionscode). Er wird beim ersten Auftreten
  untersucht: Seed (§4.5) und Log sichern, lokal in der Schleife reproduzieren
  (`cargo test … -- --test-threads=1` und viele Wiederholungen, ggf. `--release`), Ursache beheben.
* **Wenn die Ursache nicht sofort klar ist:** Der Test bleibt rot und im Pflichtlauf. Ein Issue mit Seed und Log
  blockiert das Zusammenführen betroffener Änderungen. Zulässig ist, den *Fund* zu minimieren (kleinerer
  Reproduzierer), nicht, ihn zu verstecken.
* **Typische Ursachen und Gegenmittel:** Zeit (feste Uhr/injizierte Uhr statt `Instant::now`), Threads
  (Barrieren und Kanäle statt `sleep`), Reihenfolge von Hash-Iteration (sortieren), geteilte Verzeichnisse
  (`tempfile`), Port-Kollisionen (Port 0), nicht gesetzte Seeds (§4.5), Gleitkomma-Toleranzen (Metrik statt `==`).

## 11. Abdeckungsziele

Zeilenabdeckung ist ein **Indikator, kein Ziel an sich**. Gemessen wird mit `cargo llvm-cov` (Bericht in der
Nacht-CI, anfangs ohne Gate; Gate wird eingeführt, sobald die Basis steht, und nur nach oben verschoben).

| Bereich | Ziel M1 | Ziel M6 | Anmerkung |
|---|---|---|---|
| `ritsu-dbt` Decoder + T0 | ≥ 95 % Zeilen, **100 % der implementierten Kodierungsklassen** differenziell getestet | gleich | Kodierungsabdeckung (§4.2) ist die eigentliche Metrik |
| `ritsu-dbt` T1/T2 | ≥ 80 % | ≥ 90 % | zusätzlich Stufenvergleich auf allen Klassen |
| `ritsu-mem` | ≥ 90 % | ≥ 90 % | Walk-/TLB-Randfälle, SMC |
| `ritsu-virtio`, `ritsu-gpu-proto` | ≥ 85 % + Fuzz-Target | ≥ 90 % | jeder Fehlerpfad (`Err`) hat einen Test |
| `ritsu-platform` | ≥ 70 % | ≥ 85 % | Gerätezustandsautomaten |
| `ritsu-gpu-host` | ≥ 70 % (G0/G1) | ≥ 85 % | Pfade, die nur echte Geräte erreichen, zählen über G2 |
| `air2spirv` | ≥ 80 % | ≥ 90 % | plus Fuzzing des Parsers |
| `ritsu-core`, `ritsu-vmm` | ≥ 60 % | ≥ 75 % | wenig Logik |

Zusätzliche Abdeckungsziele, die wichtiger sind als Zeilen: jeder Fehlerpfad auf Gast-Daten ist getestet; jedes
Fuzz-Target erreicht im Langlauf stabil neue Kanten nur noch selten (Plateau), bevor der Parser als „fertig“
gilt.

## 12. Checkliste für einen Pull Request

* [ ] `make check` lokal grün.
* [ ] Neue Befehlsklasse → generierte Differenztests (T0↔Referenz, T0↔T1↔T2) und Maskenliste ggf. begründet.
* [ ] Neuer Gast-Parser → Fuzz-Target im selben Änderungssatz.
* [ ] Neue GPU-Funktion → Szene + Toleranz (+ Apple-Referenz, oder `#[ignore]` mit Grund und Issue).
* [ ] Neuer Shader-Pfad → `spirv-val` und FileCheck-Test.
* [ ] Leistungsbehauptung → Benchmark mit Vorher/Nachher.
* [ ] Gefundener Fehler → Regressionstest vor dem Fix.
* [ ] Nicht ausführbare Prüfungen im Bericht unter „Offene Fragen“ benannt.
