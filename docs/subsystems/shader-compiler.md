# Shader-Compiler `air2spirv`: Detaildesign

> Status: **Entwurf v0.3** (WP-M0-SC, WP-SC-03, WP-SC-02). Autor-Rolle: `shader-compiler-engineer`.
> Verbindlich sind [ADR-0003](../adr/0003-languages.md) (C++20, C-ABI), [ADR-0007](../adr/0007-shader-translation-on-host.md)
> (Übersetzung am Host, Vulkan-SPIR-V 1.6, `spirv-val`, Cache, unbekannte Records melden) und
> [ADR-0008](../adr/0008-clean-room.md) (Clean Room). [ADR-0005](../adr/0005-gpu-guest-path.md) ist noch *Proposed*.
> Dieses Dokument baut deshalb nichts Irreversibles auf eine der beiden Varianten.
> Code: [`compiler/air2spirv/`](../../compiler/air2spirv/), Bauanleitung: [`compiler/README.md`](../../compiler/README.md).

## 0. Kennzeichnungen

| Marke | Bedeutung |
|---|---|
| **[SPEC]** | Apple-intern und nicht öffentlich dokumentiert. Braucht eine Spezifikation vom `reverse-engineer` nach ADR-0008 in `docs/specs/metallib/`. Bis dahin ist **nichts** davon als Tatsache zu behandeln, und kein Code darf davon abhängen. |
| **[ANNAHME]** | plausibel, aber ungeprüft. Muss vor dem Bau darauf bestätigt werden. |
| **[MESSEN]** | Treiber- oder Hardwareverhalten, das auf RTX 4050 / Iris Xe / Lavapipe gemessen werden muss |
| **[OFFEN]** | Entscheidung nötig, siehe §13 |
| ★ | Hochrisiko, zuerst als Spike |

**Öffentliche Quellen**, auf die sich dieses Dokument stützt: die *Metal Shading Language Specification* (MSL-Spec,
Attribute wie `[[buffer(n)]]` und ihre Semantik), die *Metal Feature Set Tables*, die öffentliche Metal-API-Doku,
die LLVM-Doku (Bitcode-Format, Bitcode-Wrapper) sowie die Spezifikationen von Vulkan und SPIR-V. Wie Metal-Konzepte
in AIR **kodiert** sind (Metadaten-Namen, Operandenlayout, Adressraum-Nummern, Intrinsic-Signaturen) und wie der
`.metallib`-Container aufgebaut ist, steht in keiner dieser Quellen. Das ist durchgehend **[SPEC]**.

## 1. Zweck und Grenzen

`air2spirv` übersetzt **eine Einstiegsfunktion** eines AIR-Moduls in ein **Vulkan-SPIR-V-1.6-Modul**, das
`spirv-val` besteht. Dazu liefert es die Reflexionsdaten, die `ritsu-gpu-host` zum Bau der Vulkan-Pipeline braucht.

**Im Scope:** Container lesen, Bitcode parsen, AIR analysieren, absenken, SPIR-V erzeugen, optional optimieren,
validieren, Reflexion. Cache-Schlüssel berechnen.

**Nicht im Scope:**

* **MSL-Quelltext übersetzen.** `newLibraryWithSource:` läuft im Gast mit Apples Compiler. Beim Host kommt nur AIR an.
* **Apple-GPU-Maschinencode** (z. B. aus Binary Archives). Wenn eine App nur Maschinencode und kein AIR mitliefert,
  kann der Host nichts tun. Das wird ehrlich gemeldet, siehe O-SC-12.
* **Pipeline-Objekte, Ressourcen, Synchronisation.** Das macht `ritsu-gpu-host`. Der Compiler beschreibt nur, was
  der Shader erwartet.
* **On-Disk-Cache.** Speicherort, Verdrängung und Integrität gehören dem Host. Der Compiler liefert den Schlüssel
  (§8).

## 2. Pipeline-Überblick

```
 Gast-Bytes (feindlich)
   │
   ▼
 [0] Eingangsprüfung     Größenlimit, Argumente, struct_size           ── vorhanden (Skelett)
   │
 [1] Container           .metallib → Liste von Funktionen               ── [SPEC], WP-SC-06
   │                     roh / Bitcode-Wrapper → genau ein Modul        ── vorhanden (Skelett)
 [2] Bitcode-Extraktion  Wrapper-Header, Bounds, Ausrichtung            ── vorhanden (Skelett)
   │
 [3] LLVM-Parse          Pre-Scan (Identifikation, Größen, Layouts)     ── vorhanden (WP-SC-02)
   │                     parseModule → llvm::Module, verifyModule       ── vorhanden (WP-SC-02)
   │                     BitstreamAudit (unbekannte Records melden)     ── WP-SC-07 ★
 [4] AIR-Analyse         Einstiegspunkte, Stufe, Argumente, Builtins,  ── WP-SC-08 ([SPEC] für Kodierung)
   │                     Function Constants → AirModule (eigenes Modell)
 [5] Vorbereitung (IR)   Specialization, Inlining, SROA/mem2reg,        ── WP-SC-09 ff.
   │                     Legalisierung, FixIrreducible, Strukturierung ★
 [6] SPIR-V-Emission     eigener deterministischer Builder              ── WP-SC-03, WP-SC-09
   │
 [7] spirv-opt (opt.)    feste, versionierte Pass-Liste                 ── WP-SC-27
   │
 [8] spirv-val (Pflicht) Debug/CI immer, Release: Standard an           ── WP-SC-03
   │
   ▼
 a2s_output: SPIR-V-Wörter + Reflexion + Diagnosen
```

Jede Stufe hat genau zwei Ausgänge: Sie gibt ein gültiges Ergebnis weiter, oder sie bricht mit Status und Diagnose ab
(§6.4). **Es gibt keinen Pfad, der still etwas weglässt.** Ein nicht unterstütztes Feature führt zu
`A2S_ERROR_UNSUPPORTED` und nennt das Feature in der Diagnose (CLAUDE.md §3 Regel 5).

### 2.1 Stufe 1: Container (`.metallib`)

Eine `.metallib` enthält in der Regel mehrere Funktionen (Shader-Bibliothek) und dazu Metadaten (Name, Stufe,
eventuell Reflexion, Hashes, Quell-/Debug-Infos, Verweise auf dynamische Bibliotheken). **Layout, Magic,
Versionsfelder, Tabellenformate und Ausrichtung sind [SPEC].** Der Compiler braucht aus der Spezifikation mindestens:

1. eine eindeutige Erkennung (Magic + Version) und eine Liste der unterstützten Container-Versionen,
2. pro Funktion den Namen, die Stufe und die Lage des Bitcodes (Offset, Größe), dazu vorhandene Hashes,
3. ob Funktionen Bitcode gemeinsam nutzen und ob das Bitcode einen Wrapper trägt,
4. welche Zusatzabschnitte es gibt (Reflexion, Debug, Quellen) und welche davon semantisch relevant sind,
5. die Regeln, nach denen ältere und neuere Versionen sich unterscheiden (Kompatibilitätsmatrix, Risiko R-5).

Bis die Spezifikation vorliegt, nimmt das Skelett **nur rohes Bitcode und Bitcode im LLVM-Wrapper** an (öffentliche
Formate). Alles andere bekommt `A2S_ERROR_UNSUPPORTED` mit Hinweis auf ADR-0008. Der spätere Container-Parser
bekommt ein eigenes Fuzz-Target und prüft jede Größen- und Offsetangabe überlaufsicher gegen die Puffergröße. Die
Muster dafür stehen schon in `input_sniffer.cpp`.

### 2.2 Stufe 2: Bitcode-Extraktion

Öffentlich dokumentiert (LLVM, „Bitcode Wrapper Format“): Fünf Little-Endian-`u32` `magic = 0x0B17C0DE`, `version`,
`offset`, `size`, `cputype`, danach der Bitstrom mit der Magic `'B' 'C' 0xC0 0xDE`. Das Skelett prüft:

* Header vollständig (≥ 20 Byte), Magic,
* `offset ≥ 20` (keine Überlappung mit dem Header) und `offset ≤ len`,
* `size > 0` und `size ≤ len − offset` (überlaufsicher, also ohne `offset + size`),
* der Strom beginnt mit der Bitcode-Magic, und `size % 4 == 0`.
* `version ≠ 0` ist eine **Warnung**, kein Fehler. LLVM dokumentiert nur 0. Ob Apples Werkzeuge andere Werte
  schreiben, ist **[SPEC]**. Dass AIR in `.metallib` überhaupt einen Wrapper trägt, ist **[ANNAHME]**.
* Daten hinter dem Strom (Padding) sind erlaubt.

### 2.3 Stufe 3: LLVM-Parse und Versionsrisiko ★

AIR wird von **Apples LLVM-Fork** erzeugt. Dessen Bitcode kann Records, Attribute, Metadaten-Arten oder
Intrinsic-Namen enthalten, die Upstream-LLVM nicht kennt oder anders nummeriert. **Strategie** (Stand WP-SC-02,
Details und Messwerte in §11.4):

1. **Ein festgelegtes Upstream-LLVM:** vorläufig **LLVM 18.1** aus dem Ubuntu-Paket `llvm-18-dev` (S-AIR-1 Teil 1,
   Begründung in §11.4). Gelinkt werden nur die statischen Komponenten `BitReader`, `BitstreamReader`, `Core` und
   `Support`, später `Analysis`/`TransformUtils`/`Scalar` für die IR-Vorbereitung. Ob 18.1 AIR aus dem
   macOS-27-SDK fehlerfrei liest, klärt S-AIR-1 Teil 2 mit echtem AIR (O-2).
2. **Pre-Scan vor dem eigentlichen Parse** (vorhanden, `src/bitcode/prescan.*`). Ein Durchgang mit
   `llvm::BitstreamCursor` über die Blockstruktur liest Producer, Epoch und Modulversion, begrenzt Größenangaben,
   mit denen LLVM ungeprüft Speicher anlegt, und lehnt Records ab, die LLVM 18 ohne Grenzprüfung liest (Liste in
   §11.4). Eine neuere Epoch oder Modulversion ist `A2S_ERROR_UNSUPPORTED`.
3. **Bitstream-Audit** (WP-SC-07 ★) erweitert den Pre-Scan: `BitcodeReader` überspringt unbekannte *Blöcke* still
   und ignoriert in einigen Blöcken unbekannte *Records*. Das widerspricht ADR-0007. Das Audit vergleicht alle
   Blöcke und Records mit einer Liste der Block-IDs und Record-Codes, die die festgelegte LLVM-Version kennt.
   Unbekannte Einträge erzeugen eine Diagnose. Eine Richtlinientabelle entscheidet, was Fehler (`UNSUPPORTED`) und
   was Warnung ist: Ein unbekannter Metadaten-Record in einem Debug-Block ist z. B. eine Warnung, ein unbekannter
   Instruktions-Record immer ein Fehler. Bis dahin meldet LLVM einen unbekannten Instruktions-Record als
   „Invalid value“, und das wird zu `MALFORMED`.
4. **Fehlerabbildung** (vorhanden): Der Reader meldet jeden Fehler als `BitcodeError::CorruptedBitcode`, nur der
   Text unterscheidet „kaputt“ von „zu neu“. Eine Tabelle von Textanfängen der festgelegten Version
   (`Unknown attribute kind`, `Incompatible epoch`, `Unsupported version of DISubrange`, `Invalid summary
   version`) führt zu `UNSUPPORTED`, alles andere zu `MALFORMED`. Die Tests prüfen die Tabelle mit Fixtures, eine
   LLVM-Änderung fällt dort auf. Bei `MALFORMED` nennt eine Notiz den Producer, weil ein neuerer Producer die
   Ursache sein kann.
5. **`verifyModule`** läuft nach dem Parse immer. Der Reader verifiziert nicht vollständig, und Gast-Bitcode ist
   feindlich. Ein Verifier-Fehler wird zu `A2S_ERROR_MALFORMED_INPUT`, die Verifier-Ausgabe (escaped, höchstens
   16 Zeilen) steht in den Diagnosen. Ungültige Debug-Info und Debug-Info einer anderen Version werden wie bei
   LLVMs eigenem Auto-Upgrade verworfen, mit Warnung. Für die Übersetzung sind sie bedeutungslos.
6. **Typisierte und opake Zeiger:** Upstream-LLVM ≥ 17 kennt nur opake Zeiger. Altes Bitcode mit typisierten Zeigern
   wird beim Lesen automatisch umgestellt, und die Pointee-Typen gehen dabei verloren. Ob aktuelles AIR typisierte
   oder opake Zeiger nutzt und ob die Metadaten Pointee-Typen tragen (Argument-Typnamen, Strukturlayouts), ist
   **[SPEC]**. Die Absenkung darf sich deshalb **nur** auf die Typen von `load`/`store`/`getelementptr`/`alloca`
   verlassen (die sind auch bei opaken Zeigern vorhanden) und auf Metadaten, deren Bedeutung spezifiziert ist.
7. **Unbekannte `air.*`-Intrinsics** und unbekannte Metadaten-Schlüssel an Einstiegspunkten sind `UNSUPPORTED` und
   werden mit vollem Namen gemeldet. Es gibt keinen Fallback über „Funktion ignorieren“ (WP-SC-08).
8. **LLVM-Prozesszustand** (vorhanden, `src/bitcode/llvm_runtime.*`): Pro Übersetzung gibt es einen eigenen
   `llvm::LLVMContext` mit eigenem Diagnose-Handler, damit parallele Übersetzungen möglich sind. Einmal pro
   Prozess (`std::call_once`) installiert die Bibliothek einen Fatal-Error- und einen Bad-Alloc-Handler und setzt
   die versteckte Option `-disable-auto-upgrade-debug-info`. Sonst ruft LLVM 18 beim Lesen jedes Moduls mit
   aktueller Debug-Info selbst den Verifier auf und beendet bei einem Fehler den Prozess mit
   `report_fatal_error("Broken module found")` (nachgewiesen mit `llvm-dis-18` und der Fixture
   `invalid_dominance_debug.bc`). Danach gibt es keine `cl::opt`-Änderung mehr. Ein Fatal Error ist **nicht**
   abfangbar, siehe §9 (Sandbox).

### 2.4 Stufe 4: AIR-Analyse

Die Analyse baut ein **eigenes, LLVM-unabhängiges Modell** (`AirModule`/`AirEntryPoint`). Die späteren Stufen fragen
nur dieses Modell nach Metal-Semantik und durchsuchen die LLVM-Metadaten nicht noch einmal. Die *Bedeutung* der
Elemente ist öffentlich (MSL-Spec), ihre *Kodierung* in AIR ist durchgehend **[SPEC]**.

| Element | Metal-Semantik (öffentlich) | Vulkan-Ziel |
|---|---|---|
| Stufen | `vertex`, `fragment`, `kernel`, `[[object]]`, `[[mesh]]`, `[[visible]]`, `[[intersection(...)]]`, Tile-Funktionen | `Vertex`, `Fragment`, `GLCompute`, `TaskEXT`, `MeshEXT`, Funktion (inlinen/dispatchen), Ray-Query-Schleife, nicht unterstützt |
| Ressourcen-Argumente | `[[buffer(n)]]`, `[[texture(n)]]`, `[[sampler(n)]]`, `[[threadgroup(n)]]` | Bindungsmodell §5 |
| Stufen-Ein-/Ausgaben | `[[stage_in]]` mit `[[attribute(n)]]`, `[[user(name)]]`-Varyings, `[[color(n)]]`, `[[color(n), index(i)]]`, `[[depth(any/greater/less)]]` | `Location`, `Index`, `FragDepth` + `DepthGreater/Less` |
| Builtins Vertex | `[[vertex_id]]`, `[[instance_id]]`, `[[base_vertex]]`, `[[base_instance]]`, `[[position]]`, `[[point_size]]`, `[[clip_distance]]`, `[[invariant]]`, `[[render_target_array_index]]`, `[[viewport_array_index]]`, `[[amplification_id]]` | `VertexIndex`, `InstanceIndex`, `BaseVertex`/`BaseInstance` (DrawParameters), `Position`, `PointSize`, `ClipDistance`, `Invariant`, `Layer`, `ViewportIndex`, `ViewIndex` (Multiview) |
| Builtins Fragment | `[[position]]`, `[[front_facing]]`, `[[sample_id]]`, `[[sample_mask]]`, `[[point_coord]]`, `[[primitive_id]]`, `[[barycentric_coord]]`, `[[render_target_array_index]]` | `FragCoord`, `FrontFacing`, `SampleId`, `SampleMask`, `PointCoord`, `PrimitiveId`, `BaryCoordKHR`, `Layer` |
| Builtins Compute | `[[thread_position_in_grid]]`, `[[thread_position_in_threadgroup]]`, `[[thread_index_in_threadgroup]]`, `[[threadgroup_position_in_grid]]`, `[[threads_per_threadgroup]]`, `[[threads_per_grid]]`, `[[threadgroups_per_grid]]`, `[[thread_index_in_simdgroup]]`, `[[simdgroup_index_in_threadgroup]]`, `[[threads_per_simdgroup]]`, `[[simdgroups_per_threadgroup]]` | `GlobalInvocationId` (*mit Versatz, §4.14*), `LocalInvocationId`, `LocalInvocationIndex`, `WorkgroupId`, `WorkgroupSize`, Push-Konstante, `NumWorkgroups`, `SubgroupLocalInvocationId`, `SubgroupId`, `SubgroupSize`, `NumSubgroups` |
| Interpolation | `[[flat]]`, `[[center_perspective]]`, `[[center_no_perspective]]`, `[[centroid_*]]`, `[[sample_*]]`, `interpolant<T,…>` | `Flat`, Standard, `NoPerspective`, `Centroid`, `Sample`, `InterpolateAtCentroid/Sample/Offset` (GLSL.std.450) |
| Function Constants | `[[function_constant(i)]]`, `is_function_constant_defined` | Spezialisierung auf AIR-Ebene, §4.6 |
| Attribute | `[[early_fragment_tests]]`, `[[max_total_threads_per_threadgroup(n)]]`, `[[raster_order_group(i)]]` | `EarlyFragmentTests`, Reflexion, §4.12 |

**Zu prüfende Feinheiten** (mit eigenen Test-Shadern auf dem Mac, schwarzer Kasten):

* Enthält `[[instance_id]]` die Basisinstanz? Die MSL-Spec beschreibt das. Vulkans `InstanceIndex` enthält
  `firstInstance`. Bei einem Unterschied wird `InstanceIndex − BaseInstance` erzeugt. **[ANNAHME]** bis zum Test.
* Die Herkunft von Fragment-`[[position]]` und das Verhalten bei Y-Flip: Der Host spiegelt den Viewport mit negativer
  Höhe. `FragCoord` ist davon nicht betroffen, die Front-Face-Wicklung schon. Das wird mit dem `gpu-engineer`
  abgestimmt.

### 2.5 Stufe 5: Vorbereitung auf IR-Ebene

Die Reihenfolge ist fest und versioniert (sie gehört zur Determinismus-Garantie):

1. **Spezialisierung** der Function Constants (Konstanten einsetzen, `is_function_constant_defined` falten,
   abhängige Argumente streichen).
2. **Vollständiges Inlining** aller Nicht-Einstiegsfunktionen. SPIR-V kann zwar Funktionen, aber Zeiger auf
   `Function`-Speicher als Argumente sind eingeschränkt, und Rekursion ist verboten. Rekursion führt zu `UNSUPPORTED`.
3. **SROA, mem2reg, InstCombine (vorsichtig), DCE.** Die Passliste wird so gewählt, dass sie keine neuen
   Konstrukte erzeugt, die wir nicht absenken können (z. B. keine Vektorisierung, kein `select` auf Zeiger ohne
   Absenkung).
4. **Legalisierung:** ungerade Integer-Breiten (`i24`, `i128` aus Optimierungen), Vektoren mit mehr als vier
   Elementen (aufteilen), `freeze`/`undef`/`poison` → `OpUndef` bzw. Nullwert, `switch` behalten
   (`OpSwitch` ist strukturierbar), Aggregat-Werte → Composite-Instruktionen.
5. **CFG-Normalisierung** und **Strukturierung** (§4.1).

### 2.6 Stufen 6 bis 8: Emission, Optimierung, Validierung

* **Eigener SPIR-V-Builder** (WP-SC-03) auf Basis von SPIRV-Headers: Typ- und Konstanten-Deduplikation, IDs in
  Erzeugungsreihenfolge, Dekorationen, Capability- und Extension-Sammlung aus dem tatsächlichen Bedarf.
  **Bewusst kein LLVM-SPIR-V-Backend** als Hauptpfad: Es zielte lange auf OpenCL-SPIR-V, holt bei Vulkan (über die
  HLSL-Arbeit) zwar auf, legt aber Adressraum- und Bindungsabbildungen selbst fest und zieht den kompletten
  CodeGen-Stack in den Host-Prozess. Der Spike **S-SC-1** (Teil von WP-SC-09) bewertet es trotzdem als Alternative
  oder als Steinbruch, besonders seinen Strukturierer.
* **`spirv-opt`** ist optional (`A2S_COMPILE_FLAG_OPTIMIZE`) und nutzt eine feste Passliste pro Compiler-Version.
  Kein `-O` mit wechselndem Inhalt, denn der Cache-Schlüssel hängt daran.
* **`spirv-val`** läuft in Debug-Builds und in CI immer. In Release-Builds ist er Standard und nur per Flag
  abschaltbar. Ungültiges SPIR-V führt bei Treibern zu Abstürzen oder zu stiller Fehlübersetzung, und das ist teurer
  als die Validierungszeit. Ein Validierungsfehler nach der Emission ist ein **Compiler-Bug** und wird als
  `A2S_ERROR_INTERNAL` gemeldet, nie an den Gast als „Shader kaputt“.
  Dafür gibt es `spirv::check_generated_module()` (§11.3), das die Validator-Meldungen zeilenweise escaped als
  Diagnosen anhängt.

## 3. Interne Struktur (Zielbild)

```
src/
  c_api.cpp            C-ABI-Grenze: Argumentprüfung, Ausnahmen abfangen, Besitz        (vorhanden)
  options.*            struct_size-Laden, Optionen normalisieren                         (vorhanden)
  input_sniffer.*      roh/Wrapper erkennen, Bounds                                      (vorhanden)
  diagnostics.*        Diagnosen, Escaping feindlicher Strings                           (vorhanden)
  version.*            Versionsstring (Compiler, LLVM, SPIRV-Tools, Build-ID)            (vorhanden)
  compiler.*           Pipeline-Steuerung                                                (Stub)
  container/           .metallib-Parser                                     [SPEC] WP-SC-06
  bitcode/             Pre-Scan, Parse, Verifier, LLVM-Prozesszustand   (vorhanden, WP-SC-02)
                       BitstreamAudit                                              WP-SC-07
  air/                 AirModule-Modell, Analyse, Intrinsic-Tabelle         [SPEC] WP-SC-08
  ir/                  Spezialisierung, Legalisierung, Strukturierer               WP-SC-11
  lower/               Absenkung je Bereich (memory, image, subgroup, math, …)
  spirv/               Builder, Capability-Sammlung, Validierung/Optimierung  (vorhanden, WP-SC-03)
  cache/               Schlüsselberechnung                                         WP-SC-18
```

Regeln: Namespace `air2spirv`, `std::span`/`std::string_view`, kein rohes `new`/`delete`, intern Ausnahmen erlaubt,
aber **jede** C-Funktion fängt alles ab. LLVM-Typen verlassen `bitcode/`, `air/` und `ir/` nicht. `lower/` und
`spirv/` sehen LLVM nur über eine schmale Fassade, damit Tests ohne LLVM laufen können.

## 4. Die schweren Probleme und der Plan dafür

### 4.1 Strukturierter Kontrollfluss ★

**Problem.** Ein LLVM-CFG ist unstrukturiert. SPIR-V für Vulkan verlangt strukturierten Kontrollfluss: Jede
Verzweigung braucht `OpSelectionMerge`, jede Schleife `OpLoopMerge` mit Merge- und Continue-Block, dazu die
Dominanzregeln der SPIR-V-Spec. Optimiertes LLVM-IR enthält Mehrfach-Exits aus Schleifen, kritische Kanten, Sprünge
in Schleifenmitten und selten **irreduzible** Schleifen.

**Plan.**

1. **Vorbehandlung mit Upstream-LLVM-Pässen:** `LowerSwitch` nur wo nötig, **`FixIrreducible`** (macht irreduzible
   Zyklen über Guard-Blöcke reduzibel, Upstream-Pass aus dem AMDGPU-Umfeld) und **`UnifyLoopExits`** (ein Exit pro
   Schleife). Danach ist der CFG reduzibel, und jede Schleife hat genau einen Header und einen Exit.
2. **Strukturierer** auf dem reduziblen CFG. Er bestimmt Merge- und Continue-Blöcke über Dominator- und
   Postdominator-Bäume und fügt bei Bedarf Leer- und Guard-Blöcke ein. Zum Vorbild kommen in Frage:
   (a) der Strukturierer von **dxil-spirv**, der genau dieses Problem (LLVM-IR aus DXIL → Vulkan-SPIR-V) produktiv
   löst (laut Repository MIT-Lizenz, zu prüfen; Wiederverwendung hängt an O-1/O-SC-11),
   (b) **`SPIRVStructurizer`** aus dem Upstream-LLVM-SPIR-V-Backend,
   (c) eigener Ansatz nach „No More Gotos“ (Yakdan et al.).
   Der Spike in WP-SC-11 entscheidet. **Empfehlung:** an (a) orientieren, als eigener Code mit eigener Testbasis.
3. **Rückfall „Relooper“** mit Label-Variable und `switch`-Dispatch in einer Schleife. Er funktioniert für jeden
   CFG, ist aber langsam und verändert die Rekonvergenz. Er wird nur genutzt, wenn 1 und 2 scheitern, und wird als
   Warnung gemeldet.

**Uniformität und Rekonvergenz** (Querbezug §4.9): Strukturierung verschiebt, wo Threads wieder zusammenlaufen. Für
SIMD-Group-Operationen in divergentem Code ändert sich damit, welche Lanes „aktiv“ sind. Apples
Rekonvergenzverhalten ist nicht öffentlich spezifiziert (O-SC-10). Auf Vulkan-Seite helfen
`VK_KHR_shader_maximal_reconvergence` und `VK_KHR_shader_subgroup_uniform_control_flow` (Verfügbarkeit
**[MESSEN]**). Der Strukturierer **erhält** deshalb die ursprüngliche Postdominator-Rekonvergenz, wo es geht. Wo er
das nicht kann und der Bereich Subgroup-Operationen enthält, meldet er das.

**Test:** Generator für zufällige CFGs (auch irreduzible) → Strukturierung → `spirv-val` → Äquivalenz der
Pfadmengen (eigener kleiner Interpreter auf IR- und SPIR-V-Ebene).

### 4.2 Adressräume

Die Nummern der Adressräume in AIR sind **[SPEC]**. Die Metal-Konzepte und ihre Ziele:

| Metal | Vulkan/SPIR-V | Bemerkung |
|---|---|---|
| `device` | `PhysicalStorageBuffer` (BDA) | Standardpfad, wegen beliebiger Zeigerarithmetik (§4.3) |
| `constant` | `PhysicalStorageBuffer` + `NonWritable`, optional `Uniform`/Push-Konstanten bei kleinen, festen Puffern | Metal-`constant` darf groß sein, UBO-Limits (64 KiB) reichen nicht garantiert |
| `threadgroup` | `Workgroup` | mit `VK_KHR_workgroup_memory_explicit_layout`, wenn Typumdeutung nötig ist, §4.10 |
| `thread` | `Function` / `Private` | nach SROA/mem2reg meist in Registern. Typumdeutung in `Function` → als `uint`-Array emulieren |
| `threadgroup_imageblock` | — | Tile-Shading, nicht unterstützt (§4.13) |
| `object_data` (Mesh-Payload) | `TaskPayloadWorkgroupEXT` | §4.15 |
| `ray_data` / Intersection-Payload | `Function` in der Ray-Query-Schleife | §4.16 |

### 4.3 Beliebige Zeiger und Zeigerarithmetik im Gerätespeicher ★

**Problem.** Metal erlaubt im `device`-Raum alles, was C++ erlaubt: Zeigerarithmetik, Zeiger in Strukturen und
Puffern, Casts zwischen Typen, Vergleiche. Vulkans logische Adressierung (`StorageBuffer` + `OpAccessChain`) erlaubt
nichts davon.

**Plan.**

* **Alle `device`/`constant`-Zeiger werden zu `PhysicalStorageBuffer64`-Zeigern** (`bufferDeviceAddress`, Vulkan
  1.2 Kern, auf beiden Zielgeräten vorhanden). Zeigerarithmetik wird zu
  `OpConvertPtrToU` → `OpIAdd` → `OpConvertUToPtr`. `getelementptr` mit konstanten Indizes wird zu `OpAccessChain`,
  wenn der Typ passt (bessere Codequalität).
* **Typumdeutung:** Mit opaken LLVM-Zeigern bestimmt jeder `load`/`store` seinen Typ. Pro Zugriff wird ein Zeiger
  des passenden SPIR-V-Typs erzeugt (`OpConvertUToPtr` mit Zieltyp). Das erzeugt viele Pointer-Typen, aber sie
  sind gültig. Auf längere Sicht ist **`SPV_KHR_untyped_pointers`** (`VK_KHR_shader_untyped_pointers`) die saubere
  Lösung. Die Treiberunterstützung ist **[MESSEN]**.
* **Ausrichtung:** Jeder Zugriff über `PhysicalStorageBuffer` braucht den Operanden `Aligned`. Der Wert kommt aus
  der `align`-Angabe von LLVM. Fehlt sie oder ist sie kleiner als die natürliche Ausrichtung, wird in kleinere
  Zugriffe zerlegt, statt eine höhere Ausrichtung zu behaupten (sonst still falsch auf Intel).
* **Layout:** Strukturen bekommen explizite `Offset`/`ArrayStride` aus dem LLVM-`DataLayout` des AIR-Moduls (nicht
  aus Metal-Regeln geraten). Ob das DataLayout im AIR-Modul vollständig ist, ist **[SPEC]**.
* **Sicherheit:** Über BDA kann ein Gast-Shader jede Adresse im GPU-Adressraum des `VkDevice` lesen. Das entspricht
  Metal (auch dort gibt es keine Bounds). Auf dem Host bedeutet das aber: Der Gast darf **im selben `VkDevice`
  keine fremden Daten** vorfinden (keine Host-internen Puffer mit Geheimnissen, Trennung pro VM). GPU-Faults enden
  als `VK_ERROR_DEVICE_LOST`, den `ritsu-gpu-host` abfangen muss. Hinweis an `gpu-engineer`/`architect`.

### 4.4 Argument Buffers (Tier 2) ★

**Problem.** Argument Buffers sind gewöhnlicher Gerätespeicher, in dem Ressourcen-*Referenzen* liegen: Pufferadressen
(64 Bit), Textur- und Sampler-Handles (`MTLResourceID`), verschachtelte Argument Buffers, Arrays davon, beliebig
indiziert. Tier 2 erlaubt Schreiben aus Shadern und unbegrenzte Indizierung.

**Plan** (Teil des Bindungsmodells §5):

* Pufferreferenzen sind GPU-Adressen, also BDA wie §4.3.
* Textur- und Sampler-Referenzen sind **Indizes in globale Bindless-Heaps** (Descriptor-Arrays mit
  `runtimeDescriptorArray` + `shaderSampledImageArrayNonUniformIndexing` usw., oder
  `VK_EXT_descriptor_buffer`). Der Host legt fest, welcher 64-Bit-Wert eines `MTLResourceID` welchem Heap-Index
  entspricht. Der Shader liest den Wert aus dem Speicher, extrahiert den Index und greift mit `NonUniform` zu.
  Ob die Kodierung von `MTLResourceID` im Gast-Speicher (Variante P) vom Host gewählt werden kann oder Apple-fest
  ist, ist **[SPEC]** bzw. hängt an ADR-0005.
* Das Layout von Argument Buffers ist bei Tier 2 im Shader selbst beschrieben (Struct-Layout im IR). Ältere
  Tier-1-/Encoder-basierte Layouts (`MTLArgumentEncoder`) setzt der Host um.
* Ressourcen-*Residenz* (`useResource`, `useHeap`) betrifft nur den Host.

### 4.5 Texturen und Sampler (inkl. `constexpr sampler`)

* **Zugriffe:** `sample`, `sample_compare`, `gather`, `gather_compare`, `read`, `write`, `fence`, Abfragen (`get_width`,
  `get_num_mip_levels`, …), LOD-Varianten (`bias`, `level`, `gradient2d`, `min_lod_clamp`), konstante Offsets. Ziele:
  `OpImageSample*`, `OpImageFetch` (für `read` auf Sample-Texturen), `OpImageRead`/`OpImageWrite` (Storage
  Images), `OpImageQuerySize(Lod)`, `OpImageQueryLevels`, `MinLod` (Feature `shaderResourceMinLod`).
* **Texturtypen:** 1D/2D/3D, Arrays, Cube/Cube-Array, `texture2d_ms`, `depth*`, `texture_buffer` → Texel-Buffer.
* **Format unbekannt zur Compile-Zeit:** In Metal steht das Pixelformat einer `access::read_write`/`write`-Textur
  nicht im Shader und wird erst beim Binden bekannt. Deshalb gilt `StorageImageReadWithoutFormat` /
  `WriteWithoutFormat` (`A2S_FEATURE_STORAGE_IMAGE_WITHOUT_FORMAT`). Fehlt es, ist das Feature dem Gast nicht zu
  melden.
* **`constexpr sampler`** im Shader: Der Compiler meldet in der Reflexion pro Konstant-Sampler den vollständigen
  Zustand. Der Host legt den `VkSampler` einmal an und schreibt seinen Heap-Index in einen dafür reservierten
  Bereich der Argument-Tabelle. Metal-Zustände ohne direktes Vulkan-Gegenstück: `coord::pixel` (unnormalisiert;
  Vulkan erlaubt das nur mit Einschränkungen → sonst im Shader mit der Texturgröße normalisieren),
  Rahmenfarben (Metal kennt nur drei feste, Vulkan hat passende), `mirror_clamp_to_edge` (Vulkan-1.2-Feature).
* **Swizzle** von Textur-Views setzt der Host über `VkComponentMapping` um, nicht der Compiler.
* **Textur-Atomics** (Metal 3.1+) → Image-Atomics (`r32ui`/`r32i`; 64 Bit über
  `VK_EXT_shader_image_atomic_int64`).

### 4.6 Function Constants → Spezialisierung

**Entscheidung (Vorschlag):** Function Constants werden **auf AIR-Ebene** vor der Übersetzung eingesetzt, nicht als
SPIR-V-Specialization-Constants. Gründe: Sie können die *Existenz* von Argumenten, Struct-Feldern und Stufen-Ein- und
-Ausgaben steuern (`[[function_constant(i)]]` an Argumenten) und `is_function_constant_defined` beeinflusst
beliebigen Code. Das lässt sich in SPIR-V-Spec-Konstanten nicht vollständig ausdrücken. Folge: Eine Übersetzung pro
Konstanten-Satz. Die Werte gehen in den Cache-Schlüssel ein (`a2s_input::function_constants`, bereits in der ABI).
Die Kodierung von Function Constants in AIR ist **[SPEC]**.

**Ausnahme Workgroup-Größe:** Siehe §4.14. Dafür werden echte Spec-Konstanten (`LocalSizeId`) genutzt.

### 4.7 `half` und `bfloat`

* `half`-Arithmetik → `Float16` (`shaderFloat16`, auf RTX 4050 und Iris Xe vorhanden **[MESSEN]**). `half` im
  Speicher → `StorageBuffer16BitAccess`. `half` in Stufen-Ein- und -Ausgaben braucht `storageInputOutput16`, das auf
  beiden Zielen fehlen kann **[MESSEN]**. Dann wird die Schnittstelle auf `float` verbreitert und konvertiert.
* `bfloat` (MSL 3.1+) → `VK_KHR_shader_bfloat16`, wo vorhanden (**[MESSEN]**, auf Iris Xe unwahrscheinlich).
  Sonst gilt Emulation: Speicherung als 16 Bit, Rechnen in `float`, Konvertierung mit Round-to-Nearest-Even. Ob
  Metal `bfloat`-Arithmetik genauso rundet, wird mit Testshadern auf dem Mac geprüft (O-SC-8).

### 4.8 Fast-Math und Gleitkomma-Semantik

* Metal kompiliert standardmäßig mit **Fast-Math** (öffentliche Compiler-Option). Die Fast-Math-Flags der
  LLVM-Instruktionen (`nnan`, `ninf`, `nsz`, `arcp`, `contract`, `afn`, `reassoc`) werden ausgewertet:
  * ohne Flags oder `precise::`-Funktionen → `NoContraction` + mit `SPV_KHR_float_controls2` `FPFastMathMode None`
    bzw. Ausführungsmodus `SignedZeroInfNanPreserve`, wenn verfügbar,
  * mit Flags → Standardverhalten von Vulkan (ohnehin großzügig) oder passendes `FPFastMathMode`.
* **Genauigkeit:** Vulkans Genauigkeitsvorgaben für `sin`, `exp`, `pow` usw. (GLSL.std.450) sind teils lockerer als
  Metals ULP-Angaben für `precise::`. Für `precise::`-Varianten gibt es bei Bedarf eigene Polynom-Implementierungen.
  Wie weit das gehen muss, ist eine Richtlinienfrage (O-SC-8): exakter = langsamer.
* **Denormals und Rundung:** Metals Verhalten pro Typ ist in der MSL-Spec beschrieben. Abbildung über
  `DenormFlushToZero`/`DenormPreserve` (float_controls), falls das Gerät es kann. Sonst Abweichung dokumentieren und
  testen.
* `fma` explizit → `OpExtInst Fma`. Implizite Kontraktion nur, wenn `contract` gesetzt ist.

### 4.9 SIMD-Groups → Subgroups ★

* Apple-GPUs haben **SIMD-Breite 32**, und Metal-Code verlässt sich darauf (z. B. `simd_shuffle` mit festen Lanes,
  Reduktionen über 32 Lanes, `simd_ballot` als Bitmaske). Die Breite des Ballot-Typs ist **[SPEC]**/MSL-Spec prüfen.
* **NVIDIA:** Subgroup-Größe 32, passt.
* **Intel Xe-LP:** Subgroup-Größen 8/16/32, vom Compiler gewählt. Mit `VK_EXT_subgroup_size_control` (Kern 1.3)
  und `requiredSubgroupSize = 32` + `REQUIRE_FULL_SUBGROUPS` für Compute (Mesh/Task falls vorhanden). Für **Fragment-
  und Vertex-Shader** ist die Größe auf Intel voraussichtlich *nicht* festlegbar (`requiredSubgroupSizeStages`
  **[MESSEN]**, O-SC-9). Optionen dort: (a) SIMD-Operationen mit Breite 32 über Subgroups der Größe 16 emulieren,
  geht nur für eine Teilmenge sinnvoll und kostet Shared-Memory-Umwege; (b) das Feature dem Gast auf Intel nicht
  melden; (c) Shader ablehnen (`UNSUPPORTED`, ehrlich). Startpunkt: (c), später (a) für häufige Muster.
* Die Reflexion meldet `required_subgroup_size = 32`, wenn der Shader SIMD-Operationen enthält. Der Host setzt es in
  `VkPipelineShaderStageRequiredSubgroupSizeCreateInfo` um. Die Optionen `min/max_subgroup_size` und
  `required_subgroup_size_stages` (schon in der ABI) sagen dem Compiler, was das Zielgerät kann.
* **Abbildung:** `simd_sum/product/min/max/and/or/xor` → `OpGroupNonUniform*Reduce`, `simd_prefix_*` →
  Inclusive/ExclusiveScan, `simd_shuffle*` → `OpGroupNonUniformShuffle(Up/Down/Xor)`, `simd_broadcast` →
  `Broadcast`, `simd_ballot`/`simd_all`/`simd_any` → `Ballot`/`All`/`Any`, `simd_is_first` → `Elect`.
* **`simdgroup_index_in_threadgroup` = `SubgroupId`** gilt nur, wenn Subgroups lineare Blöcke von
  `LocalInvocationIndex` bilden. Das ist mit Full Subgroups und einer X-Größe, die ein Vielfaches von 32 ist,
  praktisch erfüllt, aber nicht spezifiziert **[MESSEN]**. Sonst: selbst berechnen (`LocalInvocationIndex / 32`) und
  eigene Lane-ID.
* **Quad-Operationen** → `OpGroupNonUniformQuadBroadcast/Swap`. Für Quad-Shuffle mit dynamischem Lane-Index wird
  `OpGroupNonUniformShuffle` mit berechnetem Index genutzt. Im Compute-Shader bilden vier aufeinanderfolgende
  Lanes ein Quad (beide APIs). In Fragment-Shadern entspricht der 2×2-Block.
* **`simdgroup_matrix`** (8×8-MMA) → `VK_KHR_cooperative_matrix` (RTX 4050 ja, Iris Xe nein **[MESSEN]**), sonst
  Emulation über Shuffles.

### 4.10 Threadgroup-Speicher und Barrieren

* `threadgroup`-Variablen → `Workgroup`. **Dynamisch große** `[[threadgroup(n)]]`-Argumente (Größe per
  `setThreadgroupMemoryLength`) → Arrays, deren Länge eine Spec-Konstante ist. Die Reflexion nennt die
  Spec-Konstanten-ID. Wenn mehrere Bereiche über Zeiger-Casts aufeinander zugreifen, wird
  `VK_KHR_workgroup_memory_explicit_layout` (ein großer, aliasierter Block) genutzt, sonst ein `uint`-Array mit
  Byte-Adressierung.
* **Barrieren:** `threadgroup_barrier(mem_flags)` → `OpControlBarrier(Workgroup, Workgroup, AcquireRelease | …)`
  mit `WorkgroupMemory` (`mem_threadgroup`), `UniformMemory` (`mem_device`), `ImageMemory` (`mem_texture`).
  `simdgroup_barrier` → `OpControlBarrier(Subgroup, Subgroup, …)`. `atomic_thread_fence` mit Scope →
  `OpMemoryBarrier`. Grundlage ist das **Vulkan-Speichermodell** (`VulkanMemoryModel`, Kern 1.2,
  `A2S_FEATURE_VULKAN_MEMORY_MODEL`). Damit lassen sich Sichtbarkeit und Verfügbarkeit (`MakeAvailable`/
  `MakeVisible`) genau ausdrücken. Die exakte Metal-Semantik von `mem_device` innerhalb einer Threadgroup steht in
  der MSL-Spec. Die Abbildung wird mit Litmus-Tests auf Lavapipe/Hardware geprüft.
* Limits: Metal 32 KiB Threadgroup-Speicher, Vulkan `maxComputeSharedMemorySize` liegt auf beiden Zielen darüber
  **[MESSEN]**.

### 4.11 Atomics

* 32-Bit-Integer-Atomics auf `device`/`threadgroup` → `OpAtomic*` auf `PhysicalStorageBuffer`/`Workgroup`.
  Metal-Atomics sind überwiegend `memory_order_relaxed` (MSL-Spec, versionsabhängig prüfen) → Semantik `None`/Relaxed.
* 64-Bit → `shaderBufferInt64Atomics`/`shaderSharedInt64Atomics` (`A2S_FEATURE_SHADER_ATOMIC_INT64`).
* **Float-Atomics:** `add` → `VK_EXT_shader_atomic_float`, `min/max` → `VK_EXT_shader_atomic_float2`, jeweils pro
  Gerät **[MESSEN]**. Rückfall: CAS-Schleife über `OpAtomicCompareExchange` auf dem Bitmuster. Das ist korrekt, aber
  langsam, und nur zulässig, wenn die NaN-/Vorzeichen-Null-Semantik dabei stimmt (Test).
* Textur-Atomics §4.5.

### 4.12 Raster Order Groups und programmierbares Blending

* **Raster Order Groups** (`[[raster_order_group(i)]]`) → `VK_EXT_fragment_shader_interlock`
  (`FragmentShaderPixelInterlockEXT`, **[MESSEN]** auf beiden Zielen). Unterschied: Metal ordnet pro Ressource
  bzw. Gruppe, Interlock schützt einen einzigen kritischen Abschnitt, der in uniformem Kontrollfluss der
  Einstiegsfunktion liegen muss. Plan: Begin vor dem ersten und End nach dem letzten ROG-Zugriff, beide auf die
  nächste strukturierte Ebene der Einstiegsfunktion gehoben. Das ist korrekt, aber konservativ.
* **Programmierbares Blending** (`[[color(n)]]` als Fragment-*Eingabe*) → `VK_KHR_dynamic_rendering_local_read`
  (Input Attachments mit Dynamic Rendering) bzw. `VK_EXT_rasterization_order_attachment_access`. Ohne beides:
  `UNSUPPORTED` und Feature nicht melden.

### 4.13 Tile-Shader und Imageblocks

Tile-Funktionen, `imageblock`, `threadgroup_imageblock` und Tile-Dispatches haben **kein Vulkan-Gegenstück**. Plan:
**zunächst nicht unterstützt** (`UNSUPPORTED`, eigener Diagnosetext). Ob das reicht, hängt davon ab, welche
GPU-Familie dem Gast gemeldet wird (O-SC-7): Apple-Familien mit Tile-Shading voraussetzende Software würde sonst
scheitern. Spätere Emulation (Imageblock als Threadgroup-Speicher pro Kachel + Compute-Pass zwischen Render-Passes)
ist möglich, aber teuer und betrifft hauptsächlich den Host (WP-SC-26 nur Analyse).

### 4.14 Workgroup-Größe und ungleichmäßige Threadgroups

* In Metal legt der **Encoder** die Threadgroup-Größe fest (`dispatchThreads(…, threadsPerThreadgroup:)`), nicht der
  Shader. Vulkan braucht sie in der Pipeline. Plan: `LocalSizeId` mit drei Spec-Konstanten (SPIR-V 1.6,
  `maintenance4` ist Kern in 1.3). Die Reflexion nennt deren IDs (`workgroup_size_spec_ids`, bereits in der ABI).
  Der Host baut pro tatsächlich genutzter Größe eine Pipeline-Variante (Pipeline-Cache).
* **`dispatchThreads` mit Grid-Größen, die kein Vielfaches der Threadgroup-Größe sind** (ungleichmäßige
  Threadgroups): Metal erzeugt am Rand kleinere Threadgroups, und Barrieren dort zählen nur existierende Threads.
  Ein einfaches „Threads außerhalb des Grids beenden“ wäre **falsch** (Barrieren hängen bzw. Undefiniertes Verhalten).
  Plan: Der Host **zerlegt** den Dispatch in einen gleichmäßigen Hauptteil und bis zu 3 (2D) bzw. 7 (3D)
  Rand-Dispatches mit passender kleinerer Workgroup-Größe (Spec-Konstanten). Der Compiler liest
  `thread_position_in_grid`, `threadgroup_position_in_grid` und `threads_per_grid` dafür aus Push-Konstanten mit
  Versatz statt direkt aus `GlobalInvocationId`. Das ist exakt und braucht keine Shader-Tricks.

### 4.15 Mesh- und Object-Shader ★

* Object → `TaskEXT`, Mesh → `MeshEXT`, `object_data` → `TaskPayloadWorkgroupEXT`, `mesh_grid_properties` →
  `OpEmitMeshTasksEXT`. Nur mit `VK_EXT_mesh_shader`: **RTX 4050 ja, Iris Xe (Xe-LP) nein.** Auf Intel wird das
  Feature nicht gemeldet. Eine spätere Compute-Emulation ist möglich, aber kein Ziel für M7.
* Metal `mesh<…>` mit `set_vertex`/`set_primitive`/`set_primitive_count`: In Metal darf die Primitivzahl jederzeit
  gesetzt werden. `OpSetMeshOutputsEXT` muss dagegen genau einmal in uniformem Kontrollfluss **vor** allen
  Ausgabeschreibzugriffen stehen. Plan: Ausgaben in Threadgroup-Speicher puffern, Zähler am Ende setzen und dann
  kopieren. Das kostet Speicher und ist korrekt. Die Optimierung (direkt schreiben, wenn die Zahl früh feststeht)
  folgt später.

### 4.16 Ray Tracing ★

* Metals `intersector<…>`-API arbeitet ähnlich wie Ray Query: Der Aufruf liefert direkt ein Ergebnis. Abbildung auf
  `VK_KHR_ray_query` (`OpRayQueryInitializeKHR`/`ProceedKHR`/`Get…`). **RTX 4050 ja, Iris Xe nein** → dort nicht
  melden.
* **Intersection-Funktionen** (eigene Schnitttests, Tabellen mit Intersection-Funktionen): Innerhalb der
  `Proceed`-Schleife werden prozedurale Kandidaten per `switch` auf den Funktionsindex an die (eingebetteten)
  Intersection-Funktionen verteilt. Das ist dieselbe Technik wie §4.17.
* Beschleunigungsstrukturen in Argument Buffers sind 64-Bit-Werte → `OpConvertUToAccelerationStructureKHR`. Das Layout
  der Instanz-Deskriptoren (Metal vs. `VkAccelerationStructureInstanceKHR`) übersetzt der Host.

### 4.17 Visible Function Tables, Funktionszeiger, dynamische Bibliotheken ★

**Problem.** SPIR-V für Vulkan hat **keine Funktionszeiger**. Metal hat Visible Function Tables, Intersection
Function Tables, Funktionszeiger in Shadern und `MTLDynamicLibrary`.

**Plan.** Metal verlangt, dass alle aufrufbaren Funktionen beim Pipeline-Bau bekannt sind (`linkedFunctions`,
`preloadedLibraries`, öffentliche API). Damit ist die Menge geschlossen:

1. Alle Kandidaten-Module werden **auf IR-Ebene zusammengelinkt** (die ABI braucht dafür später mehrere Eingaben, §6.6).
2. Jeder Funktion wird ein **dichter Index** zugeordnet. Ein Tabelleneintrag (`MTLFunctionHandle`) ist für den Host
   dieser Index.
3. Ein indirekter Aufruf wird zu `switch (index) { case k: call f_k(...) }`. Die Kandidaten werden eingebettet oder
   als SPIR-V-Funktionen aufgerufen (SPIR-V-Funktionen sind erlaubt, nur nicht über Zeiger).
4. **Rekursion** (direkt oder über die Tabelle) → `UNSUPPORTED`.
5. Unbekannter Index zur Laufzeit → definiertes Verhalten (kein Aufruf und Nullwerte), weil Metal hier UB hat und
   wir „definiert und harmlos“ wählen.

Kosten: Codegröße steigt linear mit der Kandidatenzahl, und eine neue verlinkte Funktion erzwingt eine neue
Übersetzung (Cache-Schlüssel enthält alle gelinkten Module).

### 4.18 Vertex-Fetch (`stage_in`)

* Mit `[[stage_in]]` beschreibt der `MTLVertexDescriptor` des Pipeline-Zustands Formate, Offsets, Strides und
  Step-Funktionen. **Pfad A:** Abbildung auf Vulkan-Vertex-Input-Attribute (`Location` = `[[attribute(n)]]`). Das
  geht für die meisten Formate. Die Konvertierung von Pufferformat in Shadertyp leistet die Hardware.
* **Pfad B, programmierbares Vertex-Pulling:** für Formate ohne Vulkan-Vertex-Unterstützung (z. B.
  `uchar4normalized_bgra` je nach Gerät, gepackte Formate wie `floatRG11B10`/`floatRGB9E5`), `stepRate`-Werte,
  die `VK_EXT_vertex_attribute_divisor` (Kern 1.4) nicht abdeckt, und konstante Step-Funktionen. Der Compiler
  erzeugt den Fetch-Code (BDA-Loads + Entpacken) aus dem Vertex-Descriptor. Dafür braucht er den Descriptor als
  Eingabe (ABI-Erweiterung §6.6), und der Descriptor gehört zum Cache-Schlüssel.
* Vertex-Shader ohne `stage_in`, die selbst über `[[vertex_id]]` aus Puffern lesen, sind trivial (§4.3).

### 4.19 `precise`, `[[invariant]]`, Sonstiges

* `[[invariant]]` an `[[position]]` → `Invariant`. `precise::`-Funktionen und Ausdrücke ohne `contract`-Flag →
  `NoContraction`.
* `discard_fragment()` → `OpDemoteToHelperInvocation` (Kern 1.3), damit Ableitungen in Nachbar-Pixeln gültig
  bleiben. Ob Metal semantisch „demote“ oder „terminate“ meint, wird geprüft **[ANNAHME]**. Bei `terminate` wäre
  `OpTerminateInvocation` richtig.
* **Integer-Semantik:** Division durch null, zu weite Shifts: in beiden Welten undefiniert bzw. poison, also keine
  Sonderbehandlung nötig. `i8`/`i16`/`i64` → `Int8`/`Int16`/`Int64` + Speicher-Features.
* **Endlosschleifen und GPU-Hänger** kann der Compiler nicht verhindern. Der Host braucht einen Watchdog und
  Device-Lost-Behandlung.

## 5. Bindungsmodell (Vorschlag, braucht ADR) [OFFEN]

Damit Zeigerarithmetik (§4.3), Argument Buffers (§4.4) und ungebundene Indizierung (§4.5) einheitlich funktionieren,
wird **„Argument-Tabelle + Bindless-Heaps“** vorgeschlagen:

```
Push-Konstanten (≤ 128 Byte garantiert):
  u64  argument_table_address      → Puffer mit einem Eintrag pro Metal-Slot dieser Stufe
  u32  grid_offset[3], grid_size[3] (nur Compute, §4.14) …
Argument-Tabelle (vom Host pro Draw/Dispatch geschrieben, BDA):
  [buffer(n)]   u64 GPU-Adresse (inkl. Bind-Offset)
  [texture(n)]  u32 Index im Textur-Heap
  [sampler(n)]  u32 Index im Sampler-Heap
  constexpr-Sampler-Indizes, dynamische Threadgroup-Größen …
Set 0 (global, einmal gebunden): Heaps (sampled images, storage images, samplers, texel buffers)
```

* Vorteile: ein einziges Modell für direkte Bindungen und Argument Buffers. Kein Descriptor-Set-Wechsel pro Draw, nur
  ein Push. Metal-Bind-Offsets (`setBuffer:offset:`) kosten nichts. Das passt zu Metal 4, das selbst mit
  Argument-Tabellen und GPU-Adressen arbeitet.
* Kosten: eine Indirektion pro Ressourcenzugriff (meist gecacht, im Shader hochgezogen). Das braucht
  `bufferDeviceAddress` + `descriptorIndexing` (beide Ziele haben das, Lavapipe ebenfalls).
* Alternativen: klassische Descriptor-Sets pro Slot (einfacher, aber Argument Buffers bräuchten trotzdem Bindless)
  oder `VK_EXT_descriptor_buffer` statt Descriptor-Arrays (Detail des Hosts, ändert den Shader kaum).
* Die Reflexion (`a2s_binding`: `kind`, `metal_index`, `access`, `table_offset`, `descriptor_set`/`binding`) ist
  bewusst so geschnitten, dass beide Modelle hineinpassen. Sie ist in der ABI als **PROVISIONAL** markiert.

Das Modell betrifft `ritsu-gpu-host` gleichermaßen, deshalb entscheidet ein ADR (architect + gpu-engineer +
shader-compiler-engineer), bevor WP-SC-12 beginnt.

## 6. Schnittstelle: C-ABI (`air2spirv.h`)

Der Header [`compiler/air2spirv/include/air2spirv/air2spirv.h`](../../compiler/air2spirv/include/air2spirv/air2spirv.h)
ist die einzige öffentliche Schnittstelle. Er kompiliert als C99 und C++20 (beides wird getestet).

### 6.1 Objekte und Besitz

| Objekt | Erzeugen | Freigeben | Bemerkung |
|---|---|---|---|
| `a2s_context` (opak) | `a2s_context_create(desc, &ctx)` | `a2s_context_destroy(ctx)` | unveränderlich nach Erzeugung, hält Limits (`max_input_size`) |
| `a2s_output` (opak) | `a2s_compile(ctx, &input, &out)` | `a2s_output_free(out)` | trägt Status, Diagnosen, SPIR-V, Reflexion. Darf den Kontext überleben |
| `a2s_input`, `a2s_compile_options`, `a2s_context_desc`, `a2s_function_constant` | Aufrufer (`*_init`) | Aufrufer | werden nur während des Aufrufs gelesen |

`a2s_compile` setzt `*out` immer: auf `NULL` oder auf ein Objekt, das freigegeben werden muss, **auch bei Fehlern**.
So kommen Diagnosen beim Aufrufer an (Muster wie bei shaderc). Alle `NULL`-Argumente werden geprüft.
`*_free/destroy(NULL)` ist erlaubt.

### 6.2 ABI-Evolution

* `A2S_API_VERSION` (Major.Minor). Bei Major 0 verlangt die Bibliothek **exakte** Gleichheit (instabil). Ab 1.0 gilt
  Major gleich und Minor ≤ Bibliothek.
* Jeder Struct beginnt mit `struct_size`. Laden nach den Regeln von Linux' `copy_struct_from_user`: kleiner als die
  erste veröffentlichte Version → Fehler; kleiner als die aktuelle → Rest mit Null aufgefüllt; größer → unbekannter
  Rest muss null sein, sonst `INVALID_ARGUMENT` (ein neueres Feature, das wir nicht kennen, wird nie still
  ignoriert). Ausgabe-Structs: Der Aufrufer setzt `struct_size`, die Bibliothek füllt, was beide kennen, und meldet
  ihre Größe zurück.
* Structs haben keine impliziten Füllbytes (explizite `reserved`-Felder, Größen 24/32/48 Byte). Damit sind sie
  stabil für `bindgen` und für Hashing.
* Enums haben feste Werte und `*_MAX_ENUM = 0x7FFFFFFF` (32 Bit). Bitmasken mit 64 Bit sind `#define`s.
* **Fähigkeiten:** Unbekannte `A2S_FEATURE_*`-Bits werden ignoriert (ein Feature nicht zu nutzen ist immer sicher).
  Unbekannte `A2S_COMPILE_FLAG_*`-Bits werden abgelehnt (sie verlangen Verhalten).

### 6.3 Optionen pro Übersetzung

`a2s_compile_options` hängt am einzelnen Aufruf, nicht am Kontext, weil ein Host-Prozess beide GPUs bedienen kann:

* `target_vulkan_version` (Kodierung wie `VK_MAKE_API_VERSION`, mindestens 1.3 wegen SPIR-V 1.6),
* `features` (`A2S_FEATURE_*`: die tatsächlich *aktivierten* Device-Features),
* `min/max_subgroup_size`, `required_subgroup_size_stages` (aus `VkPhysicalDeviceSubgroupSizeControlProperties`),
* `flags` (`OPTIMIZE`, `DEBUG_INFO`, `SKIP_VALIDATION`).

### 6.4 Fehler und Diagnosen

| Status | Bedeutung | Beispiel |
|---|---|---|
| `A2S_OK` | Modul erzeugt und validiert | |
| `A2S_ERROR_INVALID_ARGUMENT` | Vertrag der API verletzt | `NULL`, `struct_size`, reservierte Bits |
| `A2S_ERROR_MALFORMED_INPUT` | Eingabe kaputt | abgeschnittener Wrapper, Verifier-Fehler |
| `A2S_ERROR_UNSUPPORTED` | gültig, aber außerhalb dessen, was dieser Build kann oder je können wird | `.metallib` ohne Spec, Tile-Shader, Limit überschritten, Vulkan < 1.3 |
| `A2S_ERROR_UNIMPLEMENTED` | gültig und im Scope, aber noch nicht geschrieben | aktuell: jede Übersetzung |
| `A2S_ERROR_INTERNAL` | Compiler-Bug | Ausnahme, `spirv-val` lehnt unsere Ausgabe ab |
| `A2S_ERROR_OUT_OF_MEMORY` | Allokation fehlgeschlagen | |

`A2S_ERROR_OUT_OF_MEMORY` ist eine Ergänzung zur Liste im Brief: OOM ist kein Compiler-Bug, und der Host soll es
unterscheiden können. Diagnosen sind UTF-8-Text, eine pro Zeile, mit Präfix `error:`/`warning:`/`note:`.
**Gast-Strings** (Funktionsnamen, Bytes) werden vor dem Einbetten escaped (`escape_untrusted`), damit kein
Log-Injection möglich ist. Später kommen strukturierte Diagnosen dazu (Code, Funktion, Instruktion).

**Keine Ausnahme verlässt die ABI.** Jede Funktion fängt `std::bad_alloc` → `OUT_OF_MEMORY` und alles andere →
`INTERNAL`. LLVM-Fatal-Errors sind davon *nicht* abgedeckt (§9).

### 6.5 Threads

`a2s_context` ist nach der Erzeugung unveränderlich. `a2s_compile` darf auf demselben Kontext parallel aufgerufen
werden (getestet mit 8 Threads). Jede Übersetzung hat ihren eigenen `LLVMContext`. Ausgaben sind unabhängig. Der
Host verteilt Übersetzungen auf einen Thread-Pool auf den E-Kernen (ARCHITECTURE.md §5.1).

### 6.6 Geplante Erweiterungen (ABI 0.x)

* `a2s_inspect(ctx, bytes) → a2s_library_info`: Funktionsliste, Stufen, Function-Constant-Deklarationen,
  Vertex-Attribute. Der Host braucht das für `MTLLibrary.functionNames`/`MTLFunction`-Reflexion, bevor eine
  Pipeline entsteht (vor allem bei Variante G).
* `a2s_compute_cache_key(ctx, input, out_key[32])` (§8).
* Pipeline-Zustand als Eingabe: Vertex-Descriptor (§4.18), Farbformate (nur wo der Shader davon abhängt), gelinkte
  Funktionen und Bibliotheken (§4.17, mehrere Eingabemodule).
* Abbruch: Zeiger auf ein atomares Flag, das der Compiler zwischen Stufen prüft.
* Limits im Kontext: maximale Funktions-, Block- und Instruktionszahl, Schachtelungstiefe.

### 6.7 Anbindung an Rust

`ritsu-gpu-host` bindet den Header in einem Modul `ffi` ein (bindgen beim Build oder handgeschriebene,
mit `static_assert`s gegen die Header-Größen geprüfte Deklarationen; Entscheidung beim `gpu-engineer`). Die
Bibliothek wird statisch gelinkt (`POSITION_INDEPENDENT_CODE`, versteckte Symbol-Sichtbarkeit außer `a2s_*`). Der
Rust-Build muss die C++-Laufzeit (`stdc++`), seit WP-SC-03 die statischen SPIRV-Tools-Bibliotheken
(`SPIRV-Tools-opt`, dann `SPIRV-Tools`) und, falls aktiviert, die LLVM-Bibliotheken linken. Es gibt keine
Callbacks von C++ nach Rust, also auch kein Unwinding über die Grenze.

## 7. Determinismus

Gleiche Eingabe + gleiche Optionen + gleiche Compiler-Version (`a2s_get_version_string`) ergeben **bytegleiches**
SPIR-V. Regeln:

* keine Iteration über Container, die nach Zeigern sortiert sind (`std::unordered_map<Value*,…>` nur für Lookup,
  nie für die Ausgabereihenfolge), keine Zeitstempel, keine Abhängigkeit von Thread-Anzahl oder ASLR,
* SPIR-V-IDs in fester Erzeugungsreihenfolge, Typen und Konstanten dedupliziert in Erstauftrittsreihenfolge,
  Capabilities nach Enum-Wert und Extensions nach Namen sortiert (unabhängig von der Anforderungsreihenfolge der
  Absenkung), gleiche Dekorationen nur einmal,
* feste, versionierte Passlisten (LLVM und `spirv-opt`), LLVM- und SPIRV-Tools-Version stehen im Versionsstring.

Test: zwei Prozesse, verschiedene Thread-Anzahlen, ASLR an → Ausgaben byteweise vergleichen (WP-SC-18). Im Skelett
gibt es dafür bereits einen Unit-Test für Diagnosen.

## 8. Caching

ADR-0007 legt zwei Stufen fest: SPIR-V nach AIR-Hash und `VkPipelineCache` pro GPU. Für die SPIR-V-Stufe gilt:

* **Schlüssel** = Hash (SHA-256 oder BLAKE3, O-SC-4) über:
  1. `a2s_get_version_string()` (enthält Compiler-, LLVM- und SPIRV-Tools-Version sowie `AIR2SPIRV_BUILD_ID`,
     z. B. `air2spirv 0.1.0+dev (without llvm, spirv-tools v2025.1 [spirv-tools 2025.1~rc1-1~ubuntu0.24.04.2])`),
  2. das **extrahierte Bitcode** der Funktion (nicht die ganze `.metallib`, damit eine Bibliothek mit vielen
     Funktionen nicht bei jeder Änderung alles ungültig macht) sowie aller gelinkten Module,
  3. den Namen des Einstiegspunkts,
  4. die **normalisierten** Optionen (nach `load_compile_options`, also ohne unbekannte Feature-Bits),
  5. die Werte der Function Constants (sortiert nach Index, bereits so normalisiert),
  6. den Pipeline-Zustand, der den Shader beeinflusst (Vertex-Descriptor bei Pulling, …).
* Der Compiler **berechnet** den Schlüssel (`a2s_compute_cache_key`), weil nur er weiß, welche Eingaben wirken. Der
  Host **speichert** (Pfad, Größenlimit, Verdrängung, Integritätsprüfung beim Laden). In Debug-Builds wird
  geladenes SPIR-V erneut mit `spirv-val` geprüft.
* Workgroup-Größen und dynamische Threadgroup-Speichergrößen sind Spec-Konstanten. Sie erzeugen **keine** neue
  Übersetzung, nur neue Pipelines.

## 9. Sicherheit (feindliche Eingaben) ★ [OFFEN]

Shader kommen vom Gast und sind feindlich (CLAUDE.md §3 Regel 2, Q-SEC-1). Die eigenen Parser (Container, Wrapper)
prüfen alles und werden gefuzzt. **LLVM ist nicht gegen feindliches Bitcode gehärtet:** Der Bitcode-Reader hat
Fuzzing-Funde in seiner Geschichte, `report_fatal_error` beendet den Prozess, und `llvm_unreachable`/abgeschaltete
Asserts sind in Release-Builds undefiniertes Verhalten. Läuft der Compiler im Host-Prozess, kann ein bösartiger
Shader im schlimmsten Fall den Prozess übernehmen, der Gastspeicher und das `VkDevice` hält.

**Befund aus WP-SC-02** (§11.4): `fuzz_air_bitcode` findet im Bitcode-Reader von LLVM 18.1 nach wenigen Sekunden
Speicherfehler (Lesezugriffe über Vektorgrenzen, wilde Zeiger, SEGV), Speicherlecks auf Fehlerpfaden und
Allokationen in Gigabyte-Größe aus wenigen Bytes Eingabe. Einige Klassen fängt der Pre-Scan vorher ab. Die
häufigste Klasse (unaufgelöste Vorwärtsreferenzen im `MetadataLoader`, derselbe Codepfad bis LLVM `main`) lässt sich
ohne einen eigenen, vollständigen Metadaten-Validator nicht ausschließen. **Der Prozess, der LLVM mit Gast-Bitcode
füttert, muss deshalb als kompromittierbar gelten.**

**Vorschlag:** Übersetzung in **separaten Worker-Prozessen** (`ritsu-shaderd`): seccomp-Filter, kein Dateisystem
(Cache über übergebene Deskriptoren oder über den Hauptprozess), `RLIMIT_AS`/CPU-Zeitlimit, Kommunikation über
Shared Memory + Pipe. Ein Absturz führt zu einem ehrlichen Shader-Fehler an den Gast, und der Worker startet neu. Die
C-ABI bleibt gleich, der Worker ist nur ein weiterer Aufrufer. Das braucht einen ADR (architect, **WP-ARCH-04**), und
er ist nach dem Befund Voraussetzung dafür, dass Gast-Shader einen Build mit LLVM erreichen. Unabhängig davon
gelten interne Limits (Größe, Funktions-, Block- und Instruktionszahl, Schachtelung, Zeitbudget) →
`UNSUPPORTED` „Limit überschritten“. Die ersten davon setzt der Pre-Scan um (§11.4).

## 10. Teststrategie

| Ebene | Was | Wo | Status |
|---|---|---|---|
| Unit | API-Verträge, struct_size, Sniffer, Wrapper-Bounds, CLI-Exit-Codes, C99-Header | `compiler/air2spirv/tests/` (`air2spirv.<bereich>.<fall>`) | **vorhanden** (55 Tests) |
| Unit | Stufe 3 über die C-API mit Bitcode-Fixtures: gültig → „parsed N functions“, abgeschnitten/zerstört → `MALFORMED`, zu neu → `UNSUPPORTED`, Verifier, Debug-Info, Fuzz-Regressionen, Parallelität. Läuft in beiden LLVM-Modi | `tests/test_bitcode.cpp`, `tests/fixtures/bitcode/` | **vorhanden** (WP-SC-02) |
| Unit | SPIR-V-Builder (Determinismus, Deduplikation, Kodierung, Fehlbedienung), SPIRV-Tools-Hülle (`spirv-val`/`-dis`/`-as`/`-opt` in-process), absichtlich kaputte Module | `tests/test_spirv_builder.cpp`, `tests/test_spirv_tools.cpp` | **vorhanden** (WP-SC-03) |
| Unit | FileCheck-artiger Matcher (`CHECK`, `-NEXT`, `-SAME`, `-EMPTY`, `-NOT`, `-DAG`, `{{regex}}`, `[[VAR:regex]]`) | `tests/support/filecheck.*`, `tests/test_filecheck.cpp` | **vorhanden** (WP-SC-03) |
| Fuzzing | `fuzz_air_sniff` (Bytes → `a2s_compile` + Sniffer-Invarianten) | `compiler/air2spirv/fuzz/` | **vorhanden** |
| Fuzzing | `fuzz_air_bitcode` (mutierte Fixtures → Pre-Scan/Parse/Verifier), Fork-Modus + `triage.py` (§11.4) | dito | **vorhanden** (WP-SC-02), Audit WP-SC-07 |
| Fuzzing | `fuzz_air2spirv_translate`: strukturbewusste IR-Mutation (LLVM `FuzzMutate`), Invariante „jede Ausgabe besteht `spirv-val`, sonst sauberer Fehler“ | dito | ab WP-SC-11 |
| Property | Zufalls-CFGs → Strukturierer → `spirv-val` + Pfadäquivalenz | Unit-Tests | WP-SC-11 |
| Golden/Check | Fixture → SPIR-V → `spirv-dis` → FileCheck-artige Muster (`CHECK:`, `CHECK-NOT:`), **`spirv-val` Pflicht** (Hilfsfunktion `a2s_test::check_spirv`, vorhanden) | `tests/fixtures/air/<name>/` + Check-Dateien | ab WP-SC-09 |
| Ausführung | SPIR-V über `ritsu-gpu-host` auf **Lavapipe** ausführen, Ergebnisse gegen Referenzwerte vom Mac | `tests/gpu/` (G1) | ab WP-SC-09/12 |
| Hardware | RTX 4050 / Iris Xe (G2) | self-hosted Runner (O-3) | später |

**Fixtures** (ADR-0007/0008): nur **selbst geschriebene** Metal-Shader, einmalig auf einem Mac zu AIR/`.metallib`
kompiliert (`xcrun -sdk macosx metal …`), mit `README.md` pro Fixture (Herkunft, exakter Befehl, Xcode- und
macOS-Version, Lizenz) in `tests/fixtures/air/<name>/`. Keine Apple-Shader, keine Shader aus Apps. Braucht einen Mac
(O-2). Ergänzend gibt es **synthetische AIR-Module** als `.ll`-Text → `llvm-as`. Sie entstehen ohne Mac, aber
**erst nach der Spezifikation** der AIR-Konventionen, und ersetzen echte Fixtures nicht.

Erster Fixture-Satz (WP-SC-04): `compute_double` (`out[i] = in[i] * 2`), `compute_threadgroup_reduce` (Barrieren,
Threadgroup-Speicher), `compute_simd_sum`, `compute_atomics`, `compute_pointer_chase` (Zeigerarithmetik),
`triangle_vs`/`triangle_fs` (stage_in, Varyings, Farbausgabe), `textured_fs` (Textur + `constexpr sampler`),
`function_constants`, `control_flow` (verschachtelte Schleifen, `break`/`continue`, `switch`).

## 11. Skelett (Stand dieses WP)

### 11.1 Inhalt

* C-ABI vollständig wie §6 (Version 0.1), Argumentprüfung, `struct_size`-Evolution, Diagnosen mit Escaping,
  Grenzwert `max_input_size`.
* Erkennung von rohem Bitcode und des Bitcode-Wrappers mit vollständigen Bounds-Checks. Alles andere →
  `UNSUPPORTED` (`.metallib` mit Hinweis auf ADR-0008).
* Gültiges Bitcode → `A2S_ERROR_UNIMPLEMENTED` mit Diagnose (ehrlich, kein Platzhalter-SPIR-V). Mit LLVM ist es
  vorher geparst und verifiziert („parsed N functions“), ohne LLVM steht „bitcode not parsed“ in den Diagnosen.
* `air2spirv-cli` mit stabilen Exit-Codes, Tests, libFuzzer-Target, Startkorpus (selbst erzeugt).
* CMake-Optionen `AIR2SPIRV_WERROR` (an), `AIR2SPIRV_WITH_LLVM` (aus; an: Stufe 3 mit LLVM 18.1, §11.4),
  `AIR2SPIRV_SANITIZE`, `AIR2SPIRV_BUILD_FUZZERS`, `AIR2SPIRV_BUILD_ID`, `AIR2SPIRV_LLVM_ID`.

### 11.2 Umgebungshinweise

* `AIR2SPIRV_WITH_LLVM=ON` braucht unter Ubuntu 24.04 `llvm-18-dev` **und** `libzstd-dev` (sonst scheitert
  `LLVMExports.cmake` an `zstd::libzstd_shared`). Für lesbare Stacks in Sanitizer-Berichten `llvm-symbolizer`
  (kommt mit `llvm-18`).
* ASan/UBSan und libFuzzer mit Clang brauchen `libclang-rt-18-dev`. Mit GCC reicht die Standardinstallation.
* SPIRV-Headers und SPIRV-Tools sind seit WP-SC-03 **Pflicht** (Ubuntu 24.04: `spirv-headers` + `spirv-tools`, siehe
  §11.3). Fehlen sie, bricht CMake mit Installationshinweis ab.

### 11.3 SPIR-V-Infrastruktur (WP-SC-03)

**Abhängigkeiten.** SPIRV-Headers und SPIRV-Tools kommen aus den Ubuntu-Paketen `spirv-headers` und `spirv-tools`
(noble-updates: 1.6.1+1.4.309 bzw. 2025.1~rc1). `spirv-tools` enthält unter Debian/Ubuntu neben den Programmen
auch Header, statische Bibliotheken und CMake-Configs, ein separates `-dev`-Paket gibt es nicht. Eingebunden werden
sie über `find_package(... CONFIG)` in [`cmake/Air2SpirvSpirvDeps.cmake`](../../compiler/air2spirv/cmake/Air2SpirvSpirvDeps.cmake).
Die Datei prüft beim Konfigurieren auch, ob die API reicht (`SPV_ENV_VULKAN_1_4`, Validator-/Optimizer-API,
`spv::HasResultAndType`). Andere Installationen gehen über `CMAKE_PREFIX_PATH`.

Entscheidungen:

* **Systempakete statt FetchContent/Submodul** (entschieden vom Projektinhaber, O-SC-4 = O-12 in
  [OPEN-QUESTIONS.md, „Entschieden“](../OPEN-QUESTIONS.md#entschieden)): Sie reichen auf dem CI-Ziel ubuntu-24.04,
  brauchen kein Netzwerk im Build und kein Vendoring, und sie betreffen nur den Host-Compiler, nicht den
  macOS-Gast. Der Preis: Die SPIRV-Tools-Version folgt dem Distro-Stand und ist nicht auf einen Commit gepinnt.
  Für den Cache ist das unkritisch, weil die **Paketidentität** im Versionsstring und damit im Cache-Schlüssel
  steht. `spvSoftwareVersionString()` allein liefert nur `v2025.1` und unterscheidet z. B. `2025.1~rc1` nicht
  von `2025.1`. Deshalb ermittelt CMake beim Konfigurieren per `dpkg-query` das Paket und seine Version, die die
  Bibliothek liefern (`spirv-tools 2025.1~rc1-1~ubuntu0.24.04.2`). Ohne dpkg stehen stattdessen die Build-Details
  der Bibliothek (`spvSoftwareVersionDetailsString()`: Hash bzw. Build-Datum) im String, oder ein mit
  `-DAIR2SPIRV_SPIRV_TOOLS_ID=…` gesetzter Wert. Der String wird nie gekürzt. `AIR2SPIRV_BUILD_ID` ist auf
  1–64 Zeichen aus `[A-Za-z0-9._+-]` begrenzt (CMake-Fehler sonst). Bitgleiche Ausgabe über Maschinen hinweg
  setzt dieselbe Paketversion voraus. Das zeigt der Versionsstring an.
* **Pflicht statt Option:** ADR-0007 macht `spirv-val` verbindlich. Ein Build ohne Validator würde diese Garantie
  still verlieren. Deshalb gibt es keinen Schalter zum Abschalten, nur eine klare CMake-Fehlermeldung.

**Builder** (`src/spirv/module_builder.*`): `ModuleBuilder` erzeugt SPIR-V 1.6 in logischer Layout-Reihenfolge.
Regeln:

* IDs werden ab 1 in Aufrufreihenfolge vergeben. `begin_function`/`begin_block` mit expliziter ID nehmen nur IDs
  aus `reserve_id()`, und jede nur einmal.
* Typen (außer Structs) und Konstanten werden dedupliziert, Konstanten nach Bitmuster (`+0.0` ≠ `-0.0`). Die
  `ArrayStride` gehört zur Typidentität von Arrays und (optional, für `OpPtrAccessChain`) von Zeigern.
* `OpTypeForwardPointer` steht für rekursive BDA-Typen zur Verfügung. Existiert der Zeigertyp schon, wirft das
  Vervollständigen, statt zwei Zeigertypen zu deklarieren.
* Typen, Konstanten und globale Variablen dürfen auch mitten in einem Funktionsrumpf angefordert werden.
* `emit()` prüft Ergebnis-Typ und Blockstruktur. Instruktionen der Modulebene lehnt es ab: `OpType*`,
  Konstanten und Spec-Konstanten, `OpSource*`/`OpString`, Dekorationen, `OpLabel`/`OpFunction*` und `OpVariable`
  außerhalb der Storage Class `Function`.
* Fehlbedienung und Grenzüberschreitungen werfen `BuilderError` und werden an der C-ABI zu `A2S_ERROR_INTERNAL`.
  Dazu zählen die ID-Grenze (0x3FFFFF oder kleiner per `BuilderOptions::max_id_bound`), Opcode > 0xFFFF, mehr als
  65535 Wörter pro Instruktion und NUL in Literal-Strings.
* Header-Wort `Generator` ist 0 (erlaubt, „unbekannt“). Eine Khronos-Tool-ID ist noch nicht registriert.

**SPIRV-Tools in-process** (`src/spirv/tools.*`):

| Funktion | Zweck |
|---|---|
| `validate` | `spirv-val` für Vulkan 1.3/1.4, optional mit scalar block layout |
| `disassemble` | `spirv-dis`: Friendly Names, optional mit Header und Einrückung |
| `assemble` | `spirv-as`, für Tests |
| `optimize` | `spirv-opt` mit expliziter Passliste. Validiert die Eingabe und zusätzlich die Ausgabe. |
| `check_generated_module` | Pipeline-Nachbedingung nach §2.6 |

Jeder Aufruf hat seinen eigenen SPIRV-Tools-Kontext und ist damit reentrant (getestet mit 4 Threads).

**Tests.**

* Akzeptanzmodul: GLCompute, `LocalSize 64 1 1`, Push-Constant-Block mit einem `PhysicalStorageBuffer`-Zeiger (BDA)
  auf `{ uint data[]; }` und `data[gid.x] = gid.x`. Es besteht `spirv-val` für Vulkan 1.3 und 1.4 und ist
  zweimal erzeugt bytegleich.
* Vier absichtlich kaputte Varianten werden abgelehnt, die Tests prüfen jeweils die Regel:
  * ohne `LocalSize`: `VUID-StandaloneSpirv-LocalSize-06426`,
  * ohne `Aligned`: `VUID-StandaloneSpirv-PhysicalStorageBuffer64-04708`,
  * ohne `Block` am Push-Constant: `VUID-StandaloneSpirv-PushConstant-06675`,
  * ohne PSB-Capability: eine Kernregel von SPIR-V ohne Vulkan-VUID. Geprüft wird die Meldung „requires one of
    these capabilities: PhysicalStorageBufferAddresses“.
* Matcher-Tests decken alle Direktiven und alle Fehler im Check-Text ab. Dazu gehören: falsch geschriebene oder
  unbekannte Direktiven, falsche Groß-/Kleinschreibung, fehlender Doppelpunkt und Prefix mit Leerraum. Nicht
  unterstützte LLVM-Direktiven (`CHECK-COUNT-<n>`, `CHECK-LABEL`) melden einen Fehler. `CHECK-NEXT` folgt der
  LLVM-Semantik (der erste Treffer nach dem vorigen muss auf der Folgezeile liegen). Bindungen eines `CHECK-DAG`-Laufs
  sieht ein vorangehendes `CHECK-NOT` nicht.

Stand: 137 ctest-Tests, grün mit GCC 13, Clang 18, ASan/UBSan (GCC und Clang) und mit `AIR2SPIRV_WITH_LLVM=ON`.

### 11.4 LLVM-Anbindung (WP-SC-02)

**Festlegung (S-AIR-1 Teil 1, vorläufig): LLVM 18.1 aus Ubuntu 24.04** (`llvm-18-dev`, getestet mit
1:18.1.3-1ubuntu1). CMake verlangt `find_package(LLVM 18.1 CONFIG)` und bricht mit jeder anderen Major/Minor-Version
ab, weil Fehlertexte, Record-Nummern und die Härtung unten an genau diese Version gebunden sind. Paket und Version
stehen im Versionsstring (`llvm 18.1.3 [llvm-18-dev 1:18.1.3-1ubuntu1]`) und damit im Cache-Schlüssel, wie bei
SPIRV-Tools (§11.3). Begründung:

* **O-12** (Abhängigkeiten aus Ubuntu-Paketen) und das CI-Ziel ubuntu-24.04: LLVM 18 ist dort das Standard-LLVM,
  ohne eigenen LLVM-Build (CI-Zeit, Binärgröße, O-SC-3).
* **Keine Messgrundlage für eine andere Version:** Ohne Mac und ohne echtes AIR (O-2) lässt sich nicht prüfen, ob
  eine neuere Version AIR besser liest. Der Reader von LLVM liest Bitcode älterer Versionen (Auto-Upgrade), aber
  kein Bitcode mit Records, Attributen oder Metadaten, die erst später eingeführt wurden. Auf welchem Upstream-Stand
  Apples Metal-Compiler im macOS-27-SDK beruht, ist unbekannt.
* Der Fuzz-Befund unten ist kein Argument für eine neuere Version: Der Codepfad der häufigsten Absturzklasse
  (unaufgelöste Vorwärtsreferenz am Blockende → `lazyLoadOneMetadata` ohne Index) ist in LLVM 19.1, 20.1 und
  `main` derselbe (Quelltextvergleich von `MetadataLoader.cpp`, nicht ausgeführt).

**Was S-AIR-1 Teil 2 klären muss** (echtes AIR aus eigenen Metal-Shadern, macOS-27-SDK, WP-SC-04, O-2):

1. Producer-String, Epoch und Modulversion des AIR-Bitcodes (der Pre-Scan liest alle drei).
2. Liest LLVM 18.1 alle Fixtures ohne Fehler und ohne Verifier-Befund? Falls nicht: welche Records, Attribute
   oder Metadaten fehlen. Daraus folgt die Version: ein neueres Distro-Paket, ein eigener, festgelegter Build
   (O-SC-3) oder eine ehrliche Ablehnung bestimmter SDK-Versionen.
3. Welche Auto-Upgrades der Reader dabei anwendet (Intrinsics, Attribute, typisierte → opake Zeiger) und ob sie
   Semantik verändern, die WP-SC-08 braucht.
4. Welche Blöcke oder Records der Reader still überspringt. Das ist die Eingabe für das Audit (WP-SC-07).
5. Ob Debug-Info vorhanden ist und in welcher Version (sie wird sonst mit Warnung verworfen).
6. Triple und DataLayout des AIR-Moduls (**[SPEC]**). LLVM 18 kennt die AIR-Architektur nicht und liest das
   Triple als unbekannte Architektur. Das muss für den Parse folgenlos bleiben.

**Ablauf von Stufe 3** (`src/bitcode/`, nur mit `AIR2SPIRV_WITH_LLVM=ON`; ohne LLVM wird nichts geparst und das
gemeldet):

1. `prepare_llvm_runtime()` (einmal pro Prozess): Fatal-Error- und Bad-Alloc-Handler, `-disable-auto-upgrade-debug-info`
   (§2.3 Punkt 8). Scheitert das, ist das `A2S_ERROR_INTERNAL`, und es wird nichts geparst.
2. Pre-Scan (`prescan.cpp`) über die Blockstruktur mit `llvm::BitstreamCursor`:
   * Identifikation: Producer, Epoch (≠ 0 → `UNSUPPORTED`), Modulversion (> 2 → `UNSUPPORTED`), unbekannter Record
     im `IDENTIFICATION_BLOCK` → `UNSUPPORTED`.
   * Größenangaben, mit denen LLVM 18 ungeprüft Speicher anlegt: Typtabelle (`NUMENTRY`), Blockzahl
     (`DECLAREBLOCKS`), Blocknummer in `blockaddress`, Parameterindex in Attributgruppen. Mehr, als der Strom
     kodieren kann → `MALFORMED`. Mehr als ein festes Limit → `UNSUPPORTED`. Dazu Limits für Vektorlänge
     (2¹⁶ Elemente) und Integer-Breite (4096 Bit), weil LLVM proportional dazu Speicher anlegt (Shuffle-Masken,
     Splats, `APInt`).
   * Records, die LLVM 18 ohne Prüfung liest: unvollständige Attributgruppen-Einträge, `DATA`-Konstanten mit einem
     Typ, der weder Array noch Vektor ist, Metadaten-Anhänge an eine Instruktion, die es nicht gibt → `MALFORMED`.
     Inline-Assembly → `UNSUPPORTED` (Metal kennt keins).
   * `USELIST_BLOCK`s (nur die Reihenfolge von Use-Listen, für die Übersetzung bedeutungslos, im Reader mit
     ungeprüften IDs) werden in einer privaten Kopie der Eingabe auf eine unbekannte Block-ID umgeschrieben, die
     LLVM überspringt. Eine Notiz nennt ihre Zahl.
   * **Dekodieren wie LLVM:** Die Prüfungen schützen nur, wenn der Pre-Scan dieselben Records mit denselben
     Abkürzungen liest wie der Reader. Deshalb folgt er dessen Struktur: Die Rolle eines Blocks hängt vom
     Elternblock ab (ein Konstantenblock wird als Kind von Modul oder Funktion geprüft, sonst nur durchlaufen, weil
     der Reader ihn überspringt). BLOCKINFO zählt nur als einziges direktes Kind des Modulblocks vor dem ersten
     Funktionsblock, an anderen Stellen überspringt ihn der Reader, eine zweite oder spätere ist `UNSUPPORTED`.
     Abkürzungsdefinitionen auf Modulebene nach dem ersten Funktionsblock sind `UNSUPPORTED` (der Reader springt
     über diesen Bereich). Jeder Block muss genau an seiner angegebenen Länge enden (der Reader überspringt manche
     Blöcke per Länge und liest andere bis `END_BLOCK`). Die Wort-Offsets, zu denen der Reader springt
     (`VSTOFFSET`, `FNENTRY`), müssen auf eine Symboltabelle bzw. einen Funktionsblock dieses Moduls zeigen, die der
     Pre-Scan gelesen hat, und `FNENTRY` muss eine Funktion nennen. Jedes Modul beginnt mit frischem Zustand, wie
     im Reader. Die Fixtures `layout_*.bc` prüfen das (Code-Review zu WP-SC-02).
3. `getBitcodeModuleList` + `parseModule` mit eigenem `LLVMContext` und einem Diagnose-Handler, der alles einsammelt
   (ohne ihn ruft `LLVMContext::diagnose` bei Fehlern `exit(1)`). Mehr als ein Modul → `UNSUPPORTED`.
4. `verifyModule` immer, danach Debug-Info-Behandlung wie im Auto-Upgrade von LLVM, ohne dessen Fatal Error.
5. Ergebnis: Die Zahlen gehen als Notiz in die Diagnosen („parsed 3 functions (2 defined, 1 declared), 0 global
   variables, 19 instructions (LLVM 18.1.3)“). Danach folgt weiterhin `A2S_ERROR_UNIMPLEMENTED`.

**Fehlerabbildung**

| Ursache | Status | Diagnose (Beispiel) |
|---|---|---|
| Bitstrom kaputt, abgeschnitten, Record ungültig | `MALFORMED` | `bitcode: LLVM 18.1.3 cannot read the module: …` + Producer-Notiz |
| Verifier-Fehler | `MALFORMED` | `bitcode: the module does not pass the LLVM verifier`, dann bis zu 16 Zeilen `verifier: …` |
| Fehler über den Diagnose-Handler des Kontexts | `MALFORMED` | `llvm: …` |
| Epoch, Modulversion, Identifikations-Record neuer als LLVM 18 | `UNSUPPORTED` | `bitcode: epoch 1 is not supported …` |
| Unbekannte Attribut-Art, DISubrange-Version, Summary-Version (Fehlertext des Readers) | `UNSUPPORTED` | `bitcode: attribute kind unknown to the linked LLVM (LLVM 18.1.3): Unknown attribute kind (100) …` |
| Mehrere Module, Inline-Assembly, Limit überschritten | `UNSUPPORTED` | `bitcode: vector element count … exceeds the limit of 65536` |
| Ungültige oder fremde Debug-Info | Warnung, weiter | `warning: bitcode: invalid debug info dropped` |
| LLVM-Laufzeit nicht konfigurierbar (falsche LLVM-Version) | `INTERNAL` | `bitcode: LLVM runtime not usable: …` |

Alle LLVM-Texte gelten als Gast-Daten (sie zitieren Namen und IR) und werden escaped und gekürzt.

**Was sich im Prozess nicht abfangen lässt** (Eingabe für WP-ARCH-04):

1. **`report_fatal_error`** in LLVM: Der installierte Handler schreibt den Grund (bereinigt) nach stderr und ruft
   `abort()`. Zurückkehren darf er nicht, und eine Ausnahme aus ihm würde durch LLVM-Code laufen, der ohne
   Exceptions gebaut ist: Destruktoren und Aufräumarbeiten in diesen Frames würden übersprungen, der `LLVMContext`
   bliebe in unbekanntem Zustand. `abort()` statt LLVMs eigenem `exit(1)`, weil `exit` in einem Prozess mit
   weiteren Threads statische Destruktoren nebenläufig ausführt und für einen Supervisor nicht von einem normalen
   Ende zu unterscheiden ist. Bekannte Auslöser sind entschärft: `Broken module found` (Debug-Info-Upgrade
   abgeschaltet), Lazy-Metadata (nicht genutzt), Riesen-Allokationen (Pre-Scan). Weitere sind nicht ausgeschlossen.
2. **Allokationsfehler in LLVM:** `safe_malloc` → `report_bad_alloc_error` → Handler → `abort()`. Ein
   `std::bad_alloc` aus `operator new` läuft dagegen durch LLVM-Frames ohne Aufräumen bis zur C-ABI und wird dort
   zu `A2S_ERROR_OUT_OF_MEMORY`. Der Prozess lebt, hat aber LLVM-Speicher verloren.
3. **Speicherfehler im Reader** (Fuzz-Befund unten): SEGV, Lesen außerhalb von Puffern, wilde Zeiger. Weder abfangbar
   noch sicher harmlos. `llvm_unreachable` und Assertions sind im Ubuntu-Build abgeschaltet und damit undefiniertes
   Verhalten.
4. **Lecks auf Fehlerpfaden** (`MDTuple`-Platzhalter, vorwärts referenzierte Werte): Jede abgelehnte Eingabe kann
   Speicher kosten. In einem langlebigen Prozess wächst er.
5. **Prozessweiter Zustand:** Fatal-Error-/Bad-Alloc-Handler und `cl::opt` gelten für den ganzen Prozess. LLVM ist
   statisch gelinkt und hat damit eigene globale Variablen, getrennt von einem Mesa-`libLLVM.so` im selben Prozess.
6. **Verworfen:** `llvm::CrashRecoveryContext` (fängt SEGV/SIGABRT per Signal-Handler und `longjmp`). Es installiert
   prozessweite Signal-Handler, die mit dem DBT, dem VMM und der Rust-Laufzeit kollidieren, und macht
   Speicherfehler nicht sicher, nur leiser.

**Fuzzing: `fuzz_air_bitcode`**

`compiler/air2spirv/fuzz/fuzz_air_bitcode.cpp` geht wie Produktionscode über `a2s_compile()` und prüft: Status nur
`MALFORMED`/`UNSUPPORTED`/`UNIMPLEMENTED`, konsistentes Ausgabeobjekt, `UNIMPLEMENTED` nur mit geparstem Modul,
deterministisches Ergebnis bei Wiederholung. Startkorpus sind die Fixtures (`generate.sh`). Weil LLVM-Funde den
Fuzzer sonst sofort anhalten, läuft er im Fork-Modus weiter. Danach spielt `triage.py` jedes Artefakt erneut ab
und ordnet es nach dem ersten aussagekräftigen Stack-Frame ein: `air2spirv` (Lauf scheitert), `llvm` (bekannt,
wird aufgelistet), `no-repro` (nichtdeterministisch), `unknown` (Lauf scheitert). Die Abdeckung, die libFuzzer
sieht, stammt nur aus `air2spirv` selbst, weil die LLVM-Bibliotheken der Distribution nicht instrumentiert sind.

Gefundene Klassen (alle im Reader von LLVM 18.1.3, keine im `air2spirv`-Code):

| Klasse | Wirkung | Status |
|---|---|---|
| Parameterindex einer Attributgruppe → `AttributeList` mit 2³² Einträgen | Allokationsfehler → Fatal (abort) | Pre-Scan, Fixture `huge_attribute_index.bc` |
| Vektortyp mit 67 Mio. Elementen → Shuffle-Maske | 1 GB aus 3,5 KB Eingabe, Timeout | Pre-Scan-Limit, Regression `vector_type_67m_elements.bc` |
| Unterminierter String in Attributgruppe | Lesen hinter dem Record (Stack) | Pre-Scan, Regression `attr_group_unterminated_string.bc` |
| `DATA`-Konstante mit skalarem Typ (ungeprüfter `cast<VectorType>`) | SEGV | Pre-Scan, Regression `data_constant_scalar_type.bc` |
| Use-Liste mit ungültiger Wert-ID | SEGV | Block neutralisiert, Regression `uselist_bad_value_id.bc` |
| Metadaten-Anhang an nicht vorhandene Instruktion | SEGV | Pre-Scan, Regression `attachment_bad_instruction.bc` |
| Unaufgelöste Metadaten-Vorwärtsreferenz (`lazyLoadOneMetadata`) | SEGV | **offen**, Beleg in `fuzz/llvm18_findings/` |
| Strukturindex in Funktionsrumpf (`StructType::getTypeAtIndex`) | SEGV | **offen**, dito |
| Wilder Lesezugriff in `parseFunctionBody` | SEGV | **offen**, dito |
| Lecks auf Fehlerpfaden (`MDTuple`-Platzhalter, Vorwärtswerte) | Speicher wächst | **offen**, dito |

Die drei offenen SEGV-Dateien bringen auch `llvm-dis-18` der Distribution zum Absturz. Die offenen Klassen
schließt nur ein Validator aus, der die Reader-Logik je Record nachbildet (Metadaten-Referenzen, Typen der
Operanden). Das ist der Umfang eines eigenen Readers und gehört, wenn überhaupt, zu WP-SC-07. Die eigentliche
Antwort ist WP-ARCH-04.

**30-min-Lauf** (Akzeptanz WP-SC-02, `FUZZ_TIME=1800 FUZZ_JOBS=3 run_fuzz_air_bitcode.sh`, 4 Kerne, nach allen
Pre-Scan-Maßnahmen): 59,7 Mio. Ausführungen in 1816 s, 2566 Abdeckungspunkte, Korpus 419 Einträge.
104 Artefakte, `triage.py`: **0 in `air2spirv`**, 0 nicht zuordenbar, 104 in LLVM (51 × Metadaten-Vorwärtsreferenz,
34 + 17 Lecks, je 1 × Strukturindex und wilder Lesezugriff). Vor den Pre-Scan-Maßnahmen fand ein 3-min-Lauf
14 Abstürze, 1 Timeout und 1 Leck, davon 7 aus der Attributgruppen-Klasse.

**CI:** `ci.yml` baut und testet mit LLVM 18 unter ASan/UBSan (Eintrag `clang-asan-llvm`). `fuzz.yml` (Job
`cpp-llvm`) fuzzt 60 s pro Pull Request und 30 min pro Nacht mit anschließender Einordnung und lädt die Artefakte
immer hoch. Stand: 154 ctest-Tests in beiden LLVM-Modi grün mit GCC 13, Clang 18 und ASan/UBSan (GCC, Clang).

## 12. Work Packages

Reihenfolge nach Abhängigkeiten. ★ = Hochrisiko, zuerst als Spike. **M4** = für den Meilenstein „Erstes Dreieck“
nötig.

| WP | Titel | Abhängig von | Akzeptanz |
|---|---|---|---|
| WP-SC-01 | Skelett, C-ABI, Sniffer, CLI, Tests, Fuzz-Target (= **WP-M0-SC**) | – | **erledigt:** Build mit GCC/Clang/ASan grün, 55 ctest-Tests, Fuzz-Smoke ohne Fund |
| WP-SC-02 | LLVM festlegen (S-AIR-1), Parse + Verifier, Fehlerabbildung, `fuzz_air_bitcode` (M4) | 01, (04 für S-AIR-1) | **erledigt mit Befund** (§11.4): LLVM 18.1 vorläufig festgelegt (S-AIR-1 Teil 1), Upstream-Clang-Bitcode parst, kaputt → `MALFORMED`, zu neu → `UNSUPPORTED`, CI-Eintrag mit LLVM. Fuzzing: keine Funde im `air2spirv`-Code, aber Speicherfehler in LLVMs Reader → WP-ARCH-04, WP-SC-07. S-AIR-1 Teil 2 offen (O-2) |
| WP-SC-03 | SPIR-V-Infrastruktur: SPIRV-Headers/-Tools einbinden, deterministischer Builder, `spirv-val`/`spirv-opt` in-process, `spirv-dis` + FileCheck-artiger Matcher für Tests (M4) | 01 | **erledigt** (§11.3): handgebautes Compute-Modul (BDA) besteht `spirv-val`, zweimal bytegleich erzeugt; kaputte Module abgelehnt; Matcher läuft; 137 ctest-Tests |
| WP-SC-04 | Fixture-Korpus v1 (Liste §10), Erzeugungsskript, READMEs (M4) | O-2 (Mac) | Fixtures + READMEs eingecheckt, Clean-Room-Review bestanden |
| WP-SC-05 ★ | **Spezifikation** `.metallib` + AIR-Konventionen (Metadaten, Adressräume, Intrinsics, Function Constants) durch `reverse-engineer`, schwarzer Kasten über eigene Fixtures (M4) | 04 | Spec in `docs/specs/metallib/` mit Konfidenz je Abschnitt |
| WP-SC-06 | `.metallib`-Container-Parser nach Spec + Fuzz-Target (M4) | 05 | alle Fixtures zerlegt; Bounds-Tests; Fuzz 30 min ohne Fund |
| WP-SC-07 ★ | Bitstream-Audit: unbekannte Blöcke/Records/Attribute/Intrinsics erkennen und melden, erweitert den Pre-Scan (§11.4). Prüfen, ob eine Validierung der Metadaten-Referenzen (Schema je Record-Code) die häufigste LLVM-Absturzklasse im Prozess ausschließen kann | 02, 04 | künstlich eingefügte unbekannte Records → Diagnose; Fixtures ohne Fund |
| WP-SC-08 | AIR-Analyse → `AirModule` (Einstiegspunkte, Stufen, Argumente, Builtins), Reflexionsgrundlage (M4) | 02, 05 | alle Fixtures korrekt beschrieben (Unit-Tests gegen erwartete Modelle) |
| WP-SC-09 | **Erstes End-to-End:** Kernel `out[i] = in[i] * 2` → SPIR-V (inkl. Spike S-SC-1 LLVM-SPIR-V-Backend) (M4) | 03, 08 | `compute_double` → `A2S_OK`, `spirv-val` grün, Check-Muster (`OpEntryPoint GLCompute`, BDA-Zugriff, `OpFMul`), bytegleich bei Wiederholung; Ausführung auf Lavapipe liefert `2·x` (sobald `ritsu-gpu-host` Compute kann) |
| WP-SC-10 | ADR Bindungsmodell (§5) mit architect + gpu-engineer (M4) | 09 | ADR *Accepted* |
| WP-SC-11 ★ | CFG-Strukturierung inkl. irreduzibler CFGs, Rekonvergenz-Erhalt (M4) | 09 | Zufalls-CFG-Property-Test (≥ 10⁵ Fälle) grün; `control_flow`-Fixture validiert |
| WP-SC-12 | Grafik-Grundlagen: Vertex/Fragment, `stage_in` (Pfad A), Interpolation, Farbausgaben, Builtins (M4) | 10, 11 | `triangle_vs/fs` validieren; Dreieck auf Lavapipe = Referenz vom Mac |
| WP-SC-13 | Texturen und Sampler inkl. `constexpr sampler` | 10, 12 | `textured_fs` validiert, Bildvergleich auf Lavapipe |
| WP-SC-14 | Mathe und Gleitkomma: `air.*`-Mathe-Intrinsics, Fast-Math-Flags, `precise`, `half`, `bfloat`, Konvertierungen | 09 | Genauigkeitstests gegen Mac-Referenzwerte innerhalb der vereinbarten Toleranz (O-SC-8) |
| WP-SC-15 | Function Constants (AIR-Spezialisierung) + `a2s_inspect` | 08 | `function_constants`-Fixture in allen Varianten korrekt; Inspect liefert Funktionsliste |
| WP-SC-16 ★ | Speicher: Zeigerarithmetik/BDA, Ausrichtung, Typumdeutung, Threadgroup-Layout, Barrieren, Atomics (inkl. Float), Speichermodell | 09, 11 | `pointer_chase`, `threadgroup_reduce`, `atomics` laufen auf Lavapipe korrekt; Litmus-Tests |
| WP-SC-17 ★ | SIMD-Group/Quad, Subgroup-Größensteuerung (Intel), ungleichmäßige Threadgroups (§4.14) | 16 | `simd_sum` korrekt auf Lavapipe; Intel-Verhalten gemessen (O-3) |
| WP-SC-18 | Cache-Schlüssel-API + Determinismus-Tests | 09 | Schlüssel ändert sich genau bei relevanten Eingaben (Property-Test); bytegleiche Ausgabe über Prozesse/Threads |
| WP-SC-19 ★ | Worker-Prozess/Sandbox (falls ADR) | ADR zu §9 | Absturz im Worker → sauberer Fehler, Host lebt weiter; seccomp-Profil getestet |
| WP-SC-20 ★ | Argument Buffers Tier 2, Ressourcen-IDs, Bindless | 10, 13, 16 | eigene Fixture mit verschachtelten Argument Buffers korrekt auf Lavapipe |
| WP-SC-21 | Programmierbares Vertex-Pulling (Pfad B) | 12 | alle Metal-Vertex-Formate getestet |
| WP-SC-22 | Raster Order Groups + programmierbares Blending | 12, 16 | Fixture-Tests auf Hardware (G2) |
| WP-SC-23 ★ | Mesh-/Object-Shader → Mesh/Task (nur RTX) | 16, 17 | Fixture korrekt auf RTX 4050 |
| WP-SC-24 ★ | Ray Query + Intersection-Funktionen (nur RTX) | 20, 25 | Fixture korrekt auf RTX 4050 |
| WP-SC-25 ★ | Visible Function Tables, Funktionszeiger, dynamische Bibliotheken (Switch-Dispatch) | 20 | Fixture mit Tabelle aus 3 Funktionen korrekt; Rekursion → `UNSUPPORTED` |
| WP-SC-26 | Tile-Shader/Imageblocks: Analyse und Entscheidung (voraussichtlich „nicht melden“) | 12, O-SC-7 | Entscheidung dokumentiert; saubere `UNSUPPORTED`-Diagnose getestet |
| WP-SC-27 | Optimierungspipeline (`spirv-opt`-Profil), Compile-Zeit-Budget, Benchmarks in `bench/` | 12 | Vorher/Nachher-Zahlen (CLAUDE.md §3 Regel 8) |
| WP-SC-28 | Metal-4-Funktionen (Tensor-/Kooperativ-Operationen, `simdgroup_matrix` → `VK_KHR_cooperative_matrix`) | 17, 20 | Fixtures je Funktion; Emulation auf Iris Xe oder ehrliches Nicht-Melden |

Kritischer Pfad für M4: 02 → 07, 03, (04 → 05 → 06/08) → 09 → 10/11 → 12 → 13. Der langsamste und am wenigsten
steuerbare Teil ist **04/05**: Mac-Zugang und Clean-Room-Spezifikation. Deshalb sollten sie früh angestoßen werden.
02 und 03 laufen bis dahin mit Upstream-Clang-Bitcode.

## 13. Offene Fragen

| ID | Frage | Empfehlung | Wer |
|---|---|---|---|
| O-SC-1 | Bindungsmodell (§5): Argument-Tabelle + Bindless oder klassische Descriptor-Sets? | Argument-Tabelle + Bindless | architect + gpu-engineer (ADR) |
| O-SC-2 | Compiler im Host-Prozess oder im sandboxed Worker (§9)? | Worker, spätestens vor M6 (Q-SEC-1) | architect (ADR) |
| O-SC-3 | Welche LLVM-Version und wie bezogen: Distro-Paket (lokal 18.1.3) oder selbst gebautes, festgelegtes LLVM (CI-Zeit, Binärgröße)? | **Vorläufig** (S-AIR-1 Teil 1, WP-SC-02): Ubuntu-Paket `llvm-18-dev`, LLVM 18.1 (§11.4). Endgültig nach S-AIR-1 Teil 2 mit echtem AIR; ein eigener Build lohnt sich nur, wenn 18.1 AIR nicht liest oder ein instrumentierter Reader fürs Fuzzing gebraucht wird | shader-compiler-engineer + Mensch (CI-Ressourcen) |
| O-SC-4 | SPIRV-Tools/-Headers (und Hash-Bibliothek): System-Paket, Git-Submodule oder vendored? FetchContent braucht Netzwerk im Build | **Entschieden** (SPIRV-Tools/-Headers, = O-12 in [OPEN-QUESTIONS.md, „Entschieden“](../OPEN-QUESTIONS.md#entschieden)): Ubuntu-Pakete `spirv-headers`/`spirv-tools` aus noble-updates, kein gepinnter Commit. Bedingung erfüllt: läuft nur im Host-Compiler, keine Auswirkung auf den macOS-Gast. Paketidentität im Cache-Schlüssel (§11.3). Noch offen: nur die Hash-Bibliothek (WP-SC-18). | Mensch (entschieden 2026-10-14); Hash-Bibliothek: shader-compiler-engineer |
| O-SC-5 | Wer kompiliert die Fixtures auf welchem Mac mit welchem Xcode/SDK (macOS-27-SDK)? | siehe O-2 | Mensch |
| O-SC-6 | Variante P: Kommt über das Paravirt-Protokoll wirklich AIR an (Annahme in ADR-0005) oder ein anderes Format? | durch S-GPU-1 klären; bestimmt den Scope des Compilers | reverse-engineer |
| O-SC-7 | Welche Metal-GPU-Familie wird gemeldet, und was folgt daraus für Tile-Shader, Imageblocks und ROGs? | mit gpu-engineer, Abhängigkeit von ADR-0005 | gpu-engineer + architect |
| O-SC-8 | Genauigkeitsrichtlinie: Wie nah an Metals ULP-Garantien muss Mathe sein (Emulation kostet Leistung)? Was gilt als „nicht still falsch“? | Toleranzen pro Funktion festschreiben, `precise::` exakt, Fast-Math nach Vulkan | Mensch + architect |
| O-SC-9 | Intel Xe-LP: Ist eine Subgroup-Größe für Fragment-/Vertex-Shader festlegbar? | auf dem G5 MF messen (O-3) | gpu-engineer |
| O-SC-10 | Rekonvergenzverhalten von Apple-GPUs bei SIMD-Operationen in divergentem Code | Black-Box-Tests mit eigenen Shadern auf dem Mac | reverse-engineer (Spec) |
| O-SC-11 | Lizenzfolgen: statisches Linken von LLVM (Apache-2.0 WITH LLVM-exception), SPIRV-Tools (Apache-2.0), mögliche Übernahme aus dxil-spirv (MIT) | hängt an O-1 | Mensch |
| O-SC-12 | Wie häufig liefern Apps nur Binary Archives ohne AIR? Gibt es dann einen Rückfall? | messen, sobald Apps laufen; bis dahin ehrlicher Fehler | gpu-engineer |

## 14. Risiken und bewusste Schulden

* **R-SC-1 Apple-LLVM-Drift** (Teil von R-5): Jede Xcode-Version kann neues Bitcode bringen. Gegenmittel: Audit
  (WP-SC-07), Kompatibilitätsmatrix in `docs/compat/`, Fixtures pro unterstützter SDK-Version.
* **R-SC-2 Strukturierung/Rekonvergenz** kann subtil falsche SIMD-Ergebnisse liefern. Gegenmittel: Property-Tests,
  Ausführungstests, Warnung, wenn die Rekonvergenz nicht erhalten bleibt.
* **R-SC-3 Feature-Lücken auf Iris Xe** (Mesh, Ray Query, Subgroup 32 im Fragment, `bfloat`): ehrlich reduzierte
  Feature-Meldung statt stiller Fehler.
* **R-SC-4 LLVM im Host-Prozess** (§9): durch den Fuzz-Befund aus WP-SC-02 bestätigt (§11.4). Bis WP-ARCH-04 steht,
  darf kein Gast-Shader einen Build mit LLVM erreichen.
* **Schulden aus WP-SC-02:** Der Pre-Scan modelliert Interna von LLVM 18 (Record-Layouts, Typ-IDs, welche Records
  Instruktionen erzeugen). Ein LLVM-Wechsel muss ihn mitprüfen (die Tests mit Fuzz-Regressionen helfen dabei).
  Die Fehlerabbildung „zu neu“ hängt an Fehlertexten des Readers. Das Fuzzing sieht keine Abdeckung innerhalb von
  LLVM (nicht instrumentiert). Bekannte LLVM-Absturzklassen werden nicht als Regression geführt, nur von
  `triage.py` als `llvm` eingeordnet.
* **Schulden im Skelett:** `.metallib` wird nicht erkannt (Spec fehlt). Reflexion und `a2s_binding` sind
  PROVISIONAL. Keine Hash-/Cache-Schlüssel-Funktion. Keine feste `spirv-opt`-Passliste (WP-SC-27), `optimize()`
  nimmt bis dahin eine explizite Liste. Kein registrierter SPIR-V-Generator-Magic (Header-Wort 2 = 0). Fuzz-CI-Job
  fehlt (gehört dem `test-engineer`). Das Makefile-Ziel `make cpp` liegt außerhalb von `compiler/` und muss bei
  Bedarf die Optionen §11 übernehmen.
