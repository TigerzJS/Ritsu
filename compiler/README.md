# compiler/

Hier lebt der Shader-Compiler **`air2spirv`**: Er übersetzt Apple AIR (LLVM-Bitcode-Dialekt aus `.metallib`) in
Vulkan-SPIR-V 1.6. Er ist in C++20 geschrieben und hat eine schmale C-ABI für `ritsu-gpu-host`
([ADR-0003](../docs/adr/0003-languages.md), [ADR-0007](../docs/adr/0007-shader-translation-on-host.md)).
Das Detaildesign steht in [docs/subsystems/shader-compiler.md](../docs/subsystems/shader-compiler.md).

**Stand (WP-M0-SC):** Das ist ein Skelett. Es gibt die C-API, Argumentprüfung, einen Erkenner für rohes LLVM-Bitcode
und für den LLVM-Bitcode-Wrapper, ein CLI, Tests und ein Fuzz-Target. Eine echte Übersetzung gibt es noch nicht:
Gültiges Bitcode liefert `A2S_ERROR_UNIMPLEMENTED` mit Diagnose. `.metallib`-Container werden mit
`A2S_ERROR_UNSUPPORTED` abgelehnt, bis eine Clean-Room-Spezifikation vorliegt
([ADR-0008](../docs/adr/0008-clean-room.md)).

## Bauen und testen

Voraussetzungen: CMake ≥ 3.25, Ninja, GCC ≥ 13 oder Clang ≥ 18. LLVM ist optional und im Skelett standardmäßig aus.

```bash
cmake -S compiler/air2spirv -B build/air2spirv -G Ninja
cmake --build build/air2spirv
ctest --test-dir build/air2spirv --output-on-failure
```

| Option | Standard | Wirkung |
|---|---|---|
| `AIR2SPIRV_WERROR` | `ON` | Warnungen (`-Wall -Wextra -Wpedantic -Wconversion …`) sind Fehler |
| `AIR2SPIRV_WITH_LLVM` | `OFF` | `find_package(LLVM CONFIG REQUIRED)` und gegen `bitreader`/`core`/`support` linken. Auf Ubuntu 24.04: `llvm-18-dev` + `libzstd-dev`, ggf. `-DLLVM_DIR=/usr/lib/llvm-18/lib/cmake/llvm` |
| `AIR2SPIRV_SANITIZE` | `OFF` | Alles mit ASan + UBSan bauen (bei Clang braucht das `libclang-rt-18-dev`) |
| `AIR2SPIRV_BUILD_TESTS` | `ON` (top level) | ctest-Tests `air2spirv.<bereich>.<fall>` |
| `AIR2SPIRV_BUILD_TOOLS` | `ON` | `air2spirv-cli` |
| `AIR2SPIRV_BUILD_FUZZERS` | `OFF` | libFuzzer-Targets in `air2spirv/fuzz/` (nur Clang, schaltet `AIR2SPIRV_SANITIZE` ein) |
| `AIR2SPIRV_BUILD_ID` | `dev` | Build-Kennung im Versionsstring; geht in jeden Shader-Cache-Schlüssel ein |

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
  src/                            Implementierung (Namespace air2spirv), C-ABI-Grenze in c_api.cpp
  tools/air2spirv-cli/            Kommandozeilenwerkzeug
  tests/                          eigene Mini-Testumgebung ohne externe Abhängigkeiten, C99-Header-Test
  fuzz/                           libFuzzer-Target fuzz_air_sniff + Startkorpus (selbst erzeugt)
```
