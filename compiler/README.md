# compiler/

Hier lebt der Shader-Compiler **`air2spirv`**: Er übersetzt Apple AIR (LLVM-Bitcode-Dialekt aus `.metallib`) in
Vulkan-SPIR-V 1.6. Er ist in C++20 geschrieben und hat eine schmale C-ABI für `ritsu-gpu-host`
([ADR-0003](../docs/adr/0003-languages.md), [ADR-0007](../docs/adr/0007-shader-translation-on-host.md)).
Das Detaildesign steht in [docs/subsystems/shader-compiler.md](../docs/subsystems/shader-compiler.md).

**Stand (WP-M0-SC, WP-SC-03, WP-SC-02):** Es gibt die C-API, Argumentprüfung, einen Erkenner für rohes LLVM-Bitcode
und für den LLVM-Bitcode-Wrapper, ein CLI, Tests und Fuzz-Targets. Dazu kommt die SPIR-V-Infrastruktur: ein
deterministischer SPIR-V-Builder, SPIRV-Tools (`spirv-val`, `spirv-dis`, `spirv-opt`) in-process und ein
FileCheck-artiger Matcher für Tests. Mit `AIR2SPIRV_WITH_LLVM=ON` liest der Compiler das Bitcode mit LLVM 18.1
(Pre-Scan, Parse, `verifyModule`) und bildet alle Fehler auf die Statuscodes ab. Eine echte Übersetzung gibt es
noch nicht: Gültiges Bitcode liefert `A2S_ERROR_UNIMPLEMENTED` mit Diagnose („parsed N functions“).
`.metallib`-Container werden mit `A2S_ERROR_UNSUPPORTED` abgelehnt, bis eine Clean-Room-Spezifikation vorliegt
([ADR-0008](../docs/adr/0008-clean-room.md)).

> **Sicherheit:** LLVMs Bitcode-Reader ist gegen feindliche Eingaben nicht robust. `fuzz_air_bitcode` findet darin
> in Sekunden Abstürze, die `air2spirv` nicht verhindern kann
> ([shader-compiler.md §11.4](../docs/subsystems/shader-compiler.md#114-llvm-anbindung-wp-sc-02)). Gast-Shader
> dürfen erst mit der Prozess-Isolation aus WP-ARCH-04 an einen Build mit LLVM.

## Bauen und testen

Voraussetzungen: CMake ≥ 3.25, Ninja, GCC ≥ 13 oder Clang ≥ 18, **SPIRV-Headers und SPIRV-Tools** (Pflicht, weil
ADR-0007 `spirv-val` verlangt). LLVM ist optional und standardmäßig aus. Mit LLVM wird genau LLVM 18.1 verlangt
(`llvm-18-dev` + `libzstd-dev`).

```bash
# Ubuntu 24.04 (spirv-tools enthält auch Header, statische Bibliotheken und CMake-Configs)
sudo apt-get install cmake ninja-build spirv-headers spirv-tools
```

Fehlt eins der Pakete, bricht CMake mit diesem Hinweis ab. Eine eigene Installation wählt man mit
`-DCMAKE_PREFIX_PATH=<präfix>` (oder `SPIRV-Headers_DIR`, `SPIRV-Tools_DIR`, `SPIRV-Tools-opt_DIR`).

```bash
cmake -S compiler/air2spirv -B build/air2spirv -G Ninja
cmake --build build/air2spirv
ctest --test-dir build/air2spirv --output-on-failure
```

| Option | Standard | Wirkung |
|---|---|---|
| `AIR2SPIRV_WERROR` | `ON` | Warnungen (`-Wall -Wextra -Wpedantic -Wconversion …`) sind Fehler |
| `AIR2SPIRV_WITH_LLVM` | `OFF` | Stufe 3 (Bitcode lesen, verifizieren) mit LLVM 18.1 (`find_package(LLVM 18.1 CONFIG)`, statisch gegen `bitreader`/`bitstreamreader`/`core`/`support`). Auf Ubuntu 24.04: `llvm-18-dev` + `libzstd-dev`. Andere Präfixe mit `-DLLVM_DIR=<präfix>/lib/cmake/llvm` |
| `AIR2SPIRV_LLVM_ID` | leer | Kennung des gelinkten LLVM im Versionsstring. Leer: Paket und Version per `dpkg-query` (z. B. `llvm-18-dev 1:18.1.3-1ubuntu1`) |
| `AIR2SPIRV_SANITIZE` | `OFF` | Alles mit ASan + UBSan bauen (bei Clang braucht das `libclang-rt-18-dev`) |
| `AIR2SPIRV_BUILD_TESTS` | `ON` (top level) | ctest-Tests `air2spirv.<bereich>.<fall>` |
| `AIR2SPIRV_BUILD_TOOLS` | `ON` | `air2spirv-cli` |
| `AIR2SPIRV_BUILD_FUZZERS` | `OFF` | libFuzzer-Targets in `air2spirv/fuzz/` (nur Clang, schaltet `AIR2SPIRV_SANITIZE` ein) |
| `AIR2SPIRV_BUILD_ID` | `dev` | Build-Kennung im Versionsstring; geht in jeden Shader-Cache-Schlüssel ein (1–64 Zeichen aus `[A-Za-z0-9._+-]`) |
| `AIR2SPIRV_SPIRV_TOOLS_ID` | leer | Kennung der gelinkten SPIRV-Tools im Versionsstring. Leer: Paket und Version per `dpkg-query`, ohne dpkg die Build-Details der Bibliothek. Für Eigenbauten z. B. `git-<sha>` |

Weitere Varianten:

```bash
# Clang
cmake -S compiler/air2spirv -B build/air2spirv-clang -G Ninja -DCMAKE_CXX_COMPILER=clang++ -DCMAKE_C_COMPILER=clang
# Sanitizer
cmake -S compiler/air2spirv -B build/air2spirv-asan -G Ninja -DAIR2SPIRV_SANITIZE=ON
# Fuzzing (60 s Smoke-Lauf gegen den eingecheckten Startkorpus)
cmake -S compiler/air2spirv -B build/air2spirv-fuzz -G Ninja -DCMAKE_CXX_COMPILER=clang++ \
      -DCMAKE_C_COMPILER=clang -DAIR2SPIRV_BUILD_FUZZERS=ON -DCMAKE_BUILD_TYPE=RelWithDebInfo
cmake --build build/air2spirv-fuzz
mkdir -p /tmp/a2s-corpus && cp compiler/air2spirv/fuzz/corpus/fuzz_air_sniff/* /tmp/a2s-corpus/
build/air2spirv-fuzz/fuzz/fuzz_air_sniff -max_total_time=60 -timeout=5 -rss_limit_mb=2048 /tmp/a2s-corpus
```

Der Fuzzer schreibt neue Korpus-Einträge in das angegebene Verzeichnis. Deshalb arbeitet er auf einer Kopie und nicht
auf dem eingecheckten Korpus ([testing.md §5.3](../docs/process/testing.md)).

Bitcode-Fuzzing mit LLVM (`fuzz_air_bitcode`, Fork-Modus und anschließende Einordnung der Funde durch
[`fuzz/triage.py`](air2spirv/fuzz/triage.py): Funde im `air2spirv`-Code lassen den Lauf scheitern, Funde in LLVM
werden aufgelistet und als Artefakte behalten):

```bash
FUZZ_TIME=60 sh compiler/air2spirv/fuzz/run_fuzz_air_bitcode.sh   # Build: build/air2spirv-fuzz-llvm
```

Die Bitcode-Fixtures in [`air2spirv/tests/fixtures/bitcode/`](air2spirv/tests/fixtures/bitcode/) entstehen aus
eigenen Quellen mit `generate.sh` (clang-18, llvm-as-18). Das Skript füllt auch den Startkorpus.

Formatieren: `clang-format -i` mit [`compiler/.clang-format`](.clang-format) (LLVM-Stil, 4 Leerzeichen, 100 Spalten).

## CLI

```bash
build/air2spirv/tools/air2spirv-cli/air2spirv-cli <eingabe> -o <ausgabe.spv> [--entry <name>]
```

| Exit-Code | Bedeutung |
|---|---|
| 0 | Erfolg, SPIR-V geschrieben |
| 1 | Eingabe kaputt (`A2S_ERROR_MALFORMED_INPUT`) |
| 2 | Aufruffehler oder ungültiges Argument |
| 3 | Ein-/Ausgabefehler |
| 4 | Eingabe nicht unterstützt (`A2S_ERROR_UNSUPPORTED`) |
| 5 | noch nicht implementiert (`A2S_ERROR_UNIMPLEMENTED`) |
| 6 | interner Fehler oder kein Speicher mehr |

## Aufbau

```
air2spirv/
  include/air2spirv/air2spirv.h   öffentliche C-ABI (C99-kompatibel, versioniert, struct_size)
  cmake/                          SPIRV-Headers/-Tools und LLVM 18.1 finden und prüfen
  src/                            Implementierung (Namespace air2spirv), C-ABI-Grenze in c_api.cpp
  src/bitcode/                    Stufe 3 (nur mit LLVM): Pre-Scan, Parse, Verifier, LLVM-Prozesszustand
  src/spirv/                      deterministischer SPIR-V-Builder, SPIRV-Tools-Hülle (val/dis/as/opt)
  tools/air2spirv-cli/            Kommandozeilenwerkzeug
  tests/                          eigene Mini-Testumgebung ohne externe Abhängigkeiten, C99-Header-Test
  tests/support/                  FileCheck-artiger Matcher, handgebaute SPIR-V-Testmodule, check_spirv()
  tests/fixtures/bitcode/         LLVM-Bitcode aus eigenen Quellen + Erzeugungsskript
  fuzz/                           libFuzzer-Targets fuzz_air_sniff, fuzz_air_bitcode, Startkorpora,
                                  Regressionseingaben, triage.py
```
