# Befunde in LLVM 18.1 (fuzz_air_bitcode, WP-SC-02)

Je ein Vertreter jeder Fundklasse, die `fuzz_air_bitcode` im Bitcode-Reader von LLVM 18.1.3 (Ubuntu
1:18.1.3-1ubuntu1) gefunden hat und die `air2spirv` nicht verhindern kann. Einordnung, Folgen und Gegenmaßnahmen
stehen in [shader-compiler.md §11.4](../../../../docs/subsystems/shader-compiler.md#114-llvm-anbindung-wp-sc-02).

**Herkunft:** mutierte Fassungen der eigenen Fixtures aus `tests/fixtures/bitcode/` (eigene Quellen, ADR-0008).

**Wichtig:** Diese Dateien bringen jedes Programm zum Absturz, das sie mit LLVM 18 liest, auch die Werkzeuge der
Distribution (`llvm-dis-18` stürzt bei den drei SEGV-Dateien ab). Sie liegen deshalb **nicht** in
`regressions/`: Der Fuzzer und die Tests spielen sie nicht ab. Sie dienen als Beleg für WP-ARCH-04
(Prozess-Isolation), als Testmaterial für WP-SC-07 und für eine mögliche Meldung an LLVM.

| Datei | Klasse | Erster Frame | Bemerkung |
|---|---|---|---|
| `metadata_unresolved_forward_ref.bc` | SEGV (Lesen) | `MetadataLoaderImpl::lazyLoadOneMetadata` | Unaufgelöste Vorwärtsreferenz am Ende eines Metadaten-Blocks. Ohne Lazy Loading ist der Index leer, und der Reader liest `GlobalMetadataBitPosIndex[…]` aus einem leeren Vektor. Häufigste Klasse (51 von 104 Funden in 30 min). Derselbe Codepfad in LLVM 19.1, 20.1 und `main` |
| `function_body_struct_index.bc` | SEGV (Lesen) | `StructType::getTypeAtIndex` aus `parseFunctionBody` | Strukturindex einer Instruktion ohne passende Prüfung |
| `function_body_wild_read.bc` | SEGV (Lesen) | `BitcodeReader::parseFunctionBody` | wilder Zeiger beim Lesen eines Funktionsrumpfs |
| `leak_metadata_placeholder.bc` | Leck | `MDTuple::getImpl` aus `getMetadataFwdRef` | Platzhalter-Knoten bleiben bei Fehlerabbruch liegen |
| `leak_forward_value.bc` | Leck | `BitcodeReaderValueList::getValueFwdRef` | vorwärts referenzierte Werte bleiben bei Fehlerabbruch liegen |

Abspielen (Build siehe `../run_fuzz_air_bitcode.sh`):

```bash
build/air2spirv-fuzz-llvm/fuzz/fuzz_air_bitcode compiler/air2spirv/fuzz/llvm18_findings/<datei>
python3 compiler/air2spirv/fuzz/triage.py build/air2spirv-fuzz-llvm/fuzz/fuzz_air_bitcode \
    compiler/air2spirv/fuzz/llvm18_findings     # erwartet: llvm=5
```

Klassen, die der Pre-Scan inzwischen abfängt, liegen als Regressionseingaben in
[`../regressions/fuzz_air_bitcode/`](../regressions/fuzz_air_bitcode/).
