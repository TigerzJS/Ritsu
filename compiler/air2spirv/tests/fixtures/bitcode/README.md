# LLVM-Bitcode-Fixtures (WP-SC-02)

Testeingaben für Stufe 3 von `air2spirv` (Bitcode lesen und verifizieren, siehe
[shader-compiler.md §2.3 und §11.4](../../../../../docs/subsystems/shader-compiler.md)).

**Herkunft:** Alle Quellen in [`src/`](src/) sind für dieses Projekt selbst geschrieben. Es sind keine Apple-Dateien,
keine Apple-Shader und kein Code Dritter enthalten (ADR-0008). Das ist **kein AIR**: Echtes AIR gibt es erst mit
WP-SC-04 (eigene Metal-Shader, auf einem Mac kompiliert, O-2). Bis dahin prüfen diese Dateien den LLVM-Teil des
Lesens mit Upstream-Clang-Bitcode.

**Erzeugung:** [`generate.sh`](generate.sh) mit `clang-18`, `llvm-as-18` und `llvm-cat-18` (Ubuntu 24.04, LLVM
18.1.3) sowie [`patch_bitcode.py`](patch_bitcode.py). Das Skript kopiert alle Dateien zusätzlich als Startkorpus nach
[`fuzz/corpus/fuzz_air_bitcode/`](../../../fuzz/corpus/fuzz_air_bitcode/). Die Ausgabe ist für denselben LLVM-Build
bytegleich. Kompiliert wird mit relativen Pfaden und `-fdebug-compilation-dir=.`, damit kein absoluter Pfad im
Bitcode landet.

**Lizenz:** wie das Repository.

| Datei | Quelle | Erwartung |
|---|---|---|
| `minimal.bc` | `src/minimal.ll` | gültig, 1 Funktion. Wird zusätzlich als C-Array in die API-Tests eingebettet |
| `vector_add.spir64.bc` | `src/vector_add.cl` (OpenCL C, `spir64`, `-O1`) | gültig, 3 Funktionen |
| `control_flow.x86_64.bc` | `src/control_flow.c` (`-O1`) | gültig, 2 Funktionen |
| `control_flow_O0.x86_64.bc` | `src/control_flow.c` (`-O0`) | gültig, 3 Funktionen |
| `structs_debug.x86_64.bc` | `src/structs.c` (`-O1 -g`) | gültig, 4 Funktionen, Debug-Info |
| `structs.arm64-apple.wrapped.bc` | `src/structs.c` (`arm64-apple-macos14`) | gültig, im öffentlichen LLVM-Bitcode-Wrapper |
| `invalid_dominance.bc` | `src/invalid_dominance.ll` (`llvm-as -disable-verify`) | `MALFORMED` (Verifier) |
| `invalid_dominance_debug.bc` | `src/invalid_dominance_debug.ll` | `MALFORMED`. LLVM 18 selbst würde hier mit `report_fatal_error` abbrechen |
| `broken_debug_info.bc` | `src/broken_debug_info.ll` | gültig, ungültige Debug-Info wird mit Warnung verworfen |
| `old_debug_info_version.bc` | `src/old_debug_info_version.ll` | gültig, Debug-Info-Version 2 wird mit Warnung verworfen |
| `two_modules.bc` | `llvm-cat -b minimal.bc control_flow.x86_64.bc` | `UNSUPPORTED` (mehrere Module) |
| `unsupported_epoch1.bc` | `control_flow.x86_64.bc`, Epoch 0 → 1 | `UNSUPPORTED` |
| `unsupported_module_version3.bc` | `control_flow.x86_64.bc`, Modulversion 2 → 3 | `UNSUPPORTED` |
| `unsupported_attribute_kind100.bc` | `control_flow.x86_64.bc`, Attribut-Art 70 → 100 | `UNSUPPORTED` |
| `huge_attribute_index.bc` | `control_flow.x86_64.bc`, Parameterindex einer Attributgruppe → 2³¹ | `MALFORMED` (Pre-Scan). `llvm-dis-18` bricht hier mit „out of memory“ ab |

**Block-Layout-Varianten** (`layout_*.bc`, aus `control_flow.x86_64.bc` mit
[`make_layout_fixtures.py`](make_layout_fixtures.py) und [`bitstream.py`](bitstream.py)): Sie prüfen, dass der
Pre-Scan den Strom so liest wie LLVM 18, oder Layouts ablehnt, bei denen das nicht sicher ist.
`bitstream.py` liest den Strom in einen Baum mit den Originalbits jedes Records, schreibt ihn bitgleich zurück und
rechnet nach Layout-Änderungen die Wort-Offsets in `VSTOFFSET` und `FNENTRY` neu.

| Datei | Änderung | Erwartung | `llvm-dis-18` |
|---|---|---|---|
| `layout_blockinfo_in_function_block.bc` | BLOCKINFO mit anderer Konstanten-Abkürzung im ersten Funktionsblock | gültig (LLVM überspringt ihn) | liest |
| `layout_blockinfo_at_top_level.bc` | dieselbe BLOCKINFO vor dem Identifikationsblock | gültig (LLVM überspringt ihn) | liest |
| `layout_blockinfo_twice.bc` | zweite BLOCKINFO direkt nach der ersten | `UNSUPPORTED` | Fehler |
| `layout_blockinfo_after_function_block.bc` | BLOCKINFO zusätzlich nach dem ersten Funktionsblock | `UNSUPPORTED` | liest |
| `layout_abbrev_after_function_block.bc` | Abkürzungsdefinition auf Modulebene nach dem ersten Funktionsblock | `UNSUPPORTED` | Fehler |
| `layout_function_block_length_mismatch.bc` | Funktionsblock meldet ein Wort mehr Länge als sein Inhalt | `MALFORMED` | liest |
| `layout_vstoffset_mismatch.bc` | `VSTOFFSET` ein Wort daneben | `MALFORMED` | Fehler |
| `layout_fnentry_offset_mismatch.bc` | Funktions-Offset in der Symboltabelle ein Wort daneben | `MALFORMED` | liest |
| `layout_fnentry_in_function_symtab.bc` | Funktions-Offset in einer Symboltabelle auf Funktionsebene | `MALFORMED` | liest |
| `layout_function_block_id_two_chunks.bc` | Block-ID des ersten Funktionsblocks in zwei VBR8-Chunks | `MALFORMED` | liest |
| `layout_nested_constants_block.bc` | Konstantenblock in einem Konstantenblock | gültig (LLVM überspringt ihn) | liest |

`patch_bitcode.py` ändert genau ein Feld und kodiert den neuen Wert in dieselbe Bitbreite (VBR mit gleicher
Chunk-Zahl). Blocklängen bleiben dadurch gültig. Grundlage ist nur das öffentlich dokumentierte
[LLVM-Bitstream-Format](https://llvm.org/docs/BitCodeFormat.html) und die Record-Nummern aus
`llvm/Bitcode/LLVMBitCodes.h`.
