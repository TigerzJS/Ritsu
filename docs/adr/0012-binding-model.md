# ADR-0012: Metal-Ressourcenbindungen laufen über eine Argument-Tabelle im Gerätespeicher und globale Bindless-Heaps; air2spirv erzeugt dafür genau eine SPIR-V-Form, und der Host verwaltet die Heaps austauschbar (Descriptor Indexing als Pflicht, `VK_EXT_descriptor_heap` als messbare Option)

* **Status:** Proposed
* **Datum:** 2026-10-14
* **Entscheider:** Mensch (Freigabe), `architect` (Entwurf, WP-ARCH-05 = WP-SC-10). Mitzeichnung durch `gpu-engineer`
  und `shader-compiler-engineer` empfohlen
* **Betrifft:** `compiler/air2spirv` (SPIR-V-Form der Ressourcenzugriffe, Reflexion `a2s_binding`/`a2s_reflection`),
  `crates/ritsu-gpu-host` (Descriptor-Verwaltung, Encoder, Hazard-Tracking, `caps`), `crates/ritsu-gpu-proto` (nur
  Variante G: Vergabe von Ressourcen-IDs), [gpu.md](../subsystems/gpu.md) §3.2, §4.5, §7,
  [shader-compiler.md](../subsystems/shader-compiler.md) §4.4, §4.5, §5

## Kontext

**Problem.** Ein Metal-Shader findet seine Ressourcen auf drei Wegen:

* über die **Slots** seiner Stufe (`setVertexBuffer:offset:atIndex:`, `setFragmentTexture:atIndex:`, `setBytes` …),
* über **Argument Buffers**, also gewöhnlichen Speicher mit GPU-Adressen und Ressourcen-IDs,
* in Metal 4 über **Argument-Tabellen**.

Vulkan bindet über Descriptor-Sets, Push-Konstanten und Buffer-Adressen. air2spirv muss eine SPIR-V-Form wählen, und
`ritsu-gpu-host` muss die passenden Deskriptoren verwalten. Beide Seiten hängen an derselben Entscheidung.
shader-compiler.md §5 schlägt „Argument-Tabelle + Bindless-Heaps“ vor und verlangt dafür einen ADR vor WP-SC-12.
Die Reflexion in der C-ABI ist bis dahin als PROVISIONAL markiert (`air2spirv.h`, `a2s_binding`).

Belege mit Fundstelle und Konfidenz stehen in [R-ARCH-07](../research/R-ARCH-07-family-policy.md). Dort sind auch
die festgehaltenen Stände der Quellen genannt: Feature-Set-Tabellen 2026-05-21, Mesa `81dff7c6`, Vulkan-Docs
`VK_HEADER_VERSION` 365. Die Zeilenangaben zu Mesa beziehen sich auf diesen Commit.

### Fakten: Metal

* **MT-1 Slots pro Stufe** ([Metal Feature Set Tables](https://developer.apple.com/metal/Metal-Feature-Set-Tables.pdf),
  Ausgabe 2026-05-21, S. 7 und S. 10, Fußnoten 2 bis 4). Ab Apple6 gibt es pro Stufe 31 Buffer-Slots, 128
  Textur-Slots und 16 Sampler-Slots. Inline-Sampler im Shader zählen zu den 16. `setBytes` ist auf 4 KB begrenzt.
  Stufen sind Vertex, Fragment, Kernel, Tile, Object und Mesh.
* **MT-2 Argument Buffers.**
  * Tier 2 ab Apple6 (Feature-Tabelle S. 4).
  * Laut Apple-Artikel [Improving CPU performance by using argument buffers](https://developer.apple.com/documentation/metal/improving-cpu-performance-by-using-argument-buffers)
    hat ein Tier-2-Argument-Buffer ab macOS 13 das Layout der entsprechenden C-Struktur. Die App schreibt
    `MTLBuffer.gpuAddress` und für Texturen, Sampler und Beschleunigungsstrukturen `gpuResourceID` als 64-Bit-Werte
    direkt hinein. Offsets addiert sie selbst.
  * Bei Tier 1 und älterem macOS ist das Layout privat und wird über `MTLArgumentEncoder` geschrieben.
  * Pro Stufe sind aus Argument Buffers erreichbar: unbegrenzt viele Buffer, 1M Texturen und 996 Sampler (Apple7)
    (S. 8). Die Grenzen gelten auch für indirekt erreichbare Argument Buffers (S. 10, Fußnote 8).
* **MT-3 Residency und Hazards bei Argument Buffers**
  ([Tracking the resource residency of argument buffers](https://developer.apple.com/documentation/metal/tracking-the-resource-residency-of-argument-buffers)).
  Metal verfolgt diese Ressourcen nicht automatisch. Die App meldet sie mit `useResource`/`useHeap`.
* **MT-4 Metal 4** (macOS 26, Symbolseiten [MTL4ArgumentTable](https://developer.apple.com/documentation/metal/mtl4argumenttable)
  und [MTL4ArgumentTableDescriptor](https://developer.apple.com/documentation/metal/mtl4argumenttabledescriptor)).
  * Eine Argument-Tabelle hat höchstens 31 Buffer-, 128 Textur- und 16 Sampler-Slots.
  * Buffer werden als GPU-Adresse gebunden (`setAddress`), optional mit Attribut-Stride. Texturen und Sampler
    werden als `MTLResourceID` gebunden.
  * Beim Encoden eines Draws oder Dispatches nimmt Metal einen **Schnappschuss** der Tabelle
    (`setArgumentTable(_:stages:)`). Eine Tabelle kann mehreren Stufen zugeordnet werden.
  * Metal 4 hat kein automatisches Hazard-Tracking ([gpu.md](../subsystems/gpu.md) §7.1).
* **MT-5 Bereits entschieden.**
  * `gpuAddress` ist die echte `VkDeviceAddress` des Host-Buffers (gpu.md §4.2).
  * `device`/`constant`-Zeiger werden zu `PhysicalStorageBuffer64` (shader-compiler.md §4.3).
  * Tier-2-IDs sind Indizes in globale Heaps pro Typ (gpu.md §4.5).
  * Zielprofil ist Apple7 (ADR-0011, Proposed). Damit ist **Tier 2 Pflicht**, und jede Bindungslösung braucht
    ohnehin Bindless-Heaps, die aus beliebigem Speicher indiziert werden.

### Fakten: Vulkan

* **V-1 Baseline** ([ADR-0002](0002-linux-host-vulkan.md), gpu.md §3.1). Vulkan 1.3 mit `bufferDeviceAddress` und
  Descriptor Indexing (update-after-bind, partially bound, non-uniform) ist Pflicht. Nach Vulkan-Docs
  `chapters/limits.adoc` gelten diese Mindestwerte:
  * `maxPushConstantsSize` 128 B (Kern), 256 B erst ab Vulkan 1.4,
  * `maxBoundDescriptorSets` 4,
  * `maxSamplerAllocationCount` 4000,
  * mit Descriptor Indexing mindestens 500 000 update-after-bind-Deskriptoren pro Stufe für Sampled Images,
    Storage Images und Sampler.
* **V-2 `VK_EXT_descriptor_buffer`** ist in der Registry als „deprecated by `VK_EXT_descriptor_heap`“ geführt
  (`xml/vk.xml`, `VK_HEADER_VERSION` 365).
* **V-3 `VK_EXT_descriptor_heap`** ([Proposal](https://github.com/KhronosGroup/Vulkan-Docs/blob/main/proposals/VK_EXT_descriptor_heap.adoc),
  veröffentlicht mit Vulkan 1.4.340 im Februar 2026 laut Sekundärquellen).
  * Es gibt genau einen Sampler-Heap und genau einen Ressourcen-Heap. Deskriptoren liegen in gewöhnlichem Speicher,
    Pipeline- und Set-Layouts entfallen.
  * Push-Daten ersetzen Push-Konstanten. Garantiert sind mindestens 256 B.
  * `DescriptorSet`/`Binding`-Dekorationen lassen sich pro Stufe auf Heap-Offsets abbilden. Möglich sind auch Indizes
    in Push-Daten oder in Speicher, dessen Adresse in den Push-Daten steht (`HEAP_WITH_INDIRECT_INDEX`).
  * Mindestgrößen: 4000 Sampler und (2²⁰ − 2¹⁵) Ressourcen-Deskriptoren.
  * Einen neuen Heap zu binden kann teuer sein. Die Empfehlung lautet: einen Heap für die ganze Laufzeit.
  * Direkter Heap-Zugriff im Shader braucht `VK_KHR_shader_untyped_pointers`. Wer nur die Abbildung nutzt, braucht
    das nicht.
* **V-4 Verfügbarkeit.** Mesa-Werte am Commit `81dff7c6`, NVIDIA proprietär aus Herstellerangaben. Der Rest ist
  **Annahme** bis WP-GPU-03.

| | RTX proprietär | NVK (RTX 4050) | ANV (Iris Xe, gfx12.0) | lavapipe (CI) |
|---|---|---|---|---|
| Descriptor Indexing + BDA | **Annahme** ja | ja | ja (`anv_physical_device.c` Z. 770, 800) | ja |
| Push-Konstanten | **Annahme** 256 B | 256 B (`nvk_private.h` Z. 14) | 256 B (`anv_types.h` Z. 35) | – |
| `VK_EXT_descriptor_buffer` | **Annahme** ja | ja ab Maxwell (Z. 247) | ja. Auf gfx12.0 aber nur **ein** Ressourcen- und **ein** Sampler-Buffer (Z. 1986 bis 1995) | ja |
| `VK_EXT_descriptor_heap` | ab Treiber **610** ([NVIDIA-Blog](https://developer.nvidia.com/blog/streamlining-resource-binding-with-end-to-end-support-for-vulkan-descriptor-heaps/), 2026-06-25) | **nein** | ja (Z. 537, 1292 bis 1294), `maxPushDataSize` 256 B | ja (`lvp_device.c` Z. 253, 1318) |
| `VK_KHR_shader_untyped_pointers` | **Annahme** ja | ja (Z. 209) | ja (Z. 481) | ja (Z. 219) |
| `VK_KHR_unified_image_layouts` | **Annahme** ? | ja (Z. 219) | **nein** (nicht in der Erweiterungstabelle) | ja (Z. 230) |
| `nullDescriptor` (robustness2) | **Annahme** ja | ja (Z. 774) | ja (Z. 976) | ja (Z. 790) |

* **V-5 Wie die Hardware Deskriptoren adressiert.**
  * **NVIDIA:** Ein Sampled-Image-Deskriptor ist bei NVK 4 Byte groß, ein 20-Bit-Index in einen globalen Bildpool
    plus ein 12-Bit-Index in einen globalen Sampler-Pool (`nvk_descriptor_types.h` Z. 42 bis 57). Bindless über
    Indizes ist dort das native Modell. Für den proprietären Treiber ist das **Annahme**, gleiche Hardware.
  * **Intel gfx12.0:** Klassische Sets kann ANV über Binding Tables auflösen
    (`anv_nir_apply_pipeline_layout.c` Z. 2338 ff.). Bindless läuft auf gfx12.0 über „indirekte Deskriptoren“:
    Der Shader liest erst eine Struktur aus dem Descriptor-Speicher und adressiert dann die Hardware
    (`anv_private.h` Z. 1558 bis 1575; `anv_physical_device.c` Z. 3214 bis 3216, gilt für `verx10` < 125). Laut
    Kommentar in `anv_physical_device.c` Z. 304 bis 313 sind höchstens etwa 1M Surface States möglich, also rund
    500K Image Views.
* **V-5a Aliasing.** Mehrere Shader-Variablen dürfen dieselbe Set/Binding-Kombination mit verschiedenen Typen
  tragen. Benutzt werden darf nur der Typ, den der gebundene Deskriptor unterstützt (Vulkan-Docs
  `chapters/interfaces.adoc`, Anmerkung ab Z. 1702). Wird eine Textur mit falscher Dimension oder falschem
  Sample-Typ angesprochen, ist das Verhalten undefiniert.
* **V-6 Reife.**
  * vkd3d-proton bildet D3D12-Bindless seit Jahren auf Descriptor Indexing ab. Es verlangt mindestens 1 000 000
    update-after-bind-Deskriptoren und empfiehlt `VK_EXT_descriptor_buffer`
    ([README](https://github.com/HansKristian-Work/vkd3d-proton/blob/master/README.md), `master`, abgerufen
    2026-10-14). Laut Sekundärberichten ist sein Heap-Pfad Mitte 2026 wegen offener NVIDIA-Fehler noch nicht Default
    (Konfidenz niedrig).
  * Ein Nutzerbericht im NVIDIA-Forum meldet `DEVICE_LOST` mit dem Heap-Pfad auf Treiber 610 (Konfidenz niedrig).

### Anforderungen

| ID | Anforderung | Quelle |
|---|---|---|
| R-1 | Tier-2-Argument-Buffers mit Adressen und IDs aus beliebigem Gerätespeicher funktionieren ohne Umschreiben durch den Host | MT-2, MT-5, gpu.md §4.5 |
| R-2 | Metal-4-Argument-Tabellen mit Schnappschuss-Semantik | MT-4 |
| R-3 | Indizes aus Gast-Speicher führen nie zu Zugriffen außerhalb eines Deskriptor-Arrays | CLAUDE.md §3.2, Q-SEC-1 |
| R-4 | Eine Übersetzung pro AIR-Funktion und Fähigkeitsprofil, deterministisch. Welcher Heap-Mechanismus verwendet wird, darf das SPIR-V nicht ändern | ADR-0007, shader-compiler.md §7, §8 |
| R-5 | Der Hazard-Tracker kennt die Zugriffsmenge jedes Encoders | gpu.md §7.2 |
| R-6 | Pflicht ist nur die Vulkan-1.3-Baseline. lavapipe in CI muss den Pfad vollständig ausführen | ADR-0002, gpu.md §3.1 |
| R-7 | Wenig CPU-Aufwand pro Draw: Metal-Apps binden pro Draw neu | Q-PERF-3 |
| R-8 | Kapazitätsgrenzen scheitern laut, nie still | CLAUDE.md §3.5 |

## Optionen

Alle Optionen setzen MT-5 voraus: Buffer sind BDA, und Tier-2-Texturen und -Sampler brauchen globale Heaps (R-1). Sie
unterscheiden sich darin, wie **direkte Slots** und Metal-4-Tabellen ankommen und welcher Vulkan-Mechanismus die
Heaps trägt.

### Option 1: Klassische Descriptor-Sets pro Stufe für direkte Slots, zusätzlich Bindless für Argument Buffers

air2spirv vergibt pro Stufe ein Set mit festen Bindings `buffer(n)`, `texture(n)` und `sampler(n)`. Der Host
schreibt pro Draw Push-Descriptors (`VK_KHR_push_descriptor`) oder Sets aus einem Pool. Daneben gibt es ein globales
Bindless-Set für Tier 2.

* **Vorteile:** Auf ANV gfx12.0 kann der Treiber Binding Tables nutzen, ohne Indirektion (V-5). Das Vulkan-Muster
  ist bekannt.
* **Nachteile:**
  * Zwei Bindungsmodelle in Compiler und Host, und Tier 2 braucht trotzdem Bindless.
  * Metal-4-Tabellen müssten in Sets umkopiert werden.
  * Buffer-Slots mit Offsets brauchen dynamische Offsets oder Neuschreiben pro Draw.
  * Sets sind pipelineweit, Metal-Slots aber stufenweise. Bei Vertex und Fragment braucht es also getrennte
    Set-Bereiche, und `maxBoundDescriptorSets` (Minimum 4, V-1) wird knapp: Bindless-Set, Vertex, Fragment, ein
    Set pro Pass.
  * `setBytes` (4 KB) passt nicht in Push-Konstanten (128 B).
* **Kosten:** etwa doppelter Aufwand in WP-SC-12/20 und WP-GPU-07/13. **Annahme:** am meisten CPU pro Draw durch
  Descriptor-Writes.
* **Risiko:** gering technisch, hoch beim Aufwand.
* **Widerlegt durch:** M-2 (siehe *Überprüfung*). Ist die Indirektion über Tabelle und Heap auf ANV höchstens 10 %
  teurer als Binding Tables, hat Option 1 keinen Vorteil mehr.

### Option 2: Argument-Tabelle im Gerätespeicher + globale Bindless-Arrays über Descriptor Indexing

Wie shader-compiler.md §5:

* Pro Stufe liegt eine kompakte **Argument-Tabelle** in einem Upload-Ring. Sie enthält Buffer-Adressen und
  Heap-Indizes. Ihre Adresse steht in den Push-Konstanten.
* Texturen und Sampler sind Indizes in **ein** globales update-after-bind-Set mit Runtime-Arrays (Set 0). Das Set
  wird einmal pro Command Buffer gebunden.
* Direkte Slots, Tier 2 und Metal 4 benutzen denselben Heap und dieselben Indizes.

* **Vorteile:**
  * Ein Modell für alles. Metal-4-Tabellen bilden sich fast 1:1 ab.
  * Pro Draw kosten Bindungen nur einen Ring-Write und einen Push. Bind-Offsets und `setBytes` sind gratis.
  * Nur Vulkan-1.2-Kernfunktionen, läuft also auf allen vier Treibern einschließlich lavapipe (R-6).
  * Auf NVIDIA ist das das native Modell (V-5).
* **Nachteile:**
  * Jeder Ressourcenzugriff kostet eine Indirektion (Tabelle, dann Heap). Auf ANV gfx12.0 kommt die indirekte
    Deskriptorstufe dazu (V-5).
  * Deskriptoren tragen das Image-Layout. Bindless zwingt Texturen deshalb in ein festes Layout (siehe E-6).
  * Indizes aus dem Speicher müssen im Shader begrenzt werden (R-3).
* **Kosten:** ein Modell in Compiler und Host, ungefähr der heutige Plan (WP-SC-12/13/20, WP-GPU-07/13).
* **Risiko:** mittel. GPU-Kosten auf der Iris und das Layout-Thema auf ANV (V-4: keine `unified_image_layouts`).
* **Widerlegt durch:** M-2 auf der Iris (Indirektion mehr als 10 % teurer als Option 1 in der Kompositionsszene)
  oder M-4 (Layout `GENERAL` auf ANV mehr als 10 % teurer).

### Option 3: wie Option 2, aber die Heaps liegen in `VK_EXT_descriptor_buffer`

* **Vorteile:** Deskriptoren lassen sich per `memcpy` in Host-Speicher schreiben, ohne `vkUpdateDescriptorSets`.
  Das Shader-SPIR-V ändert sich nicht.
* **Nachteile:**
  * Die Erweiterung ist zugunsten von `descriptor_heap` als deprecated markiert (V-2).
  * Auf ANV gfx12.0 gibt es nur einen Ressourcen- und einen Sampler-Buffer. Für dieses Modell reicht das, es zeigt
    aber, dass der Weg dort nicht ausgebaut wird.
  * Zusätzlicher Host-Pfad ohne Funktionsgewinn gegenüber Option 2.
* **Kosten:** ein zweiter Host-Pfad. **Risiko:** gering, aber totes Ende.
* **Widerlegt durch:** schon jetzt durch V-2. Ein Spike würde nur Leistung gegen Option 2 und 4 messen. Ohne
  messbaren Vorteil (M-2) entfällt die Option.

### Option 4: wie Option 2, aber die Heaps liegen in `VK_EXT_descriptor_heap`

Zwei Varianten:

* **4a – Abbildung:** Dasselbe SPIR-V wie in Option 2. Set 0 wird über `VkShaderDescriptorSetAndBindingMappingInfoEXT`
  (`HEAP_WITH_CONSTANT_OFFSET` mit Array-Stride) auf die Heaps abgebildet, Push-Daten ersetzen Push-Konstanten.
* **4b – direkter Heap-Zugriff:** SPIR-V mit `SPV_EXT_descriptor_heap` und Untyped Pointers. Ein anderes SPIR-V als
  Option 2, verletzt also R-4, solange NVK keinen Heap hat.

* **Vorteile:** Das Modell ist Metal am nächsten: zwei Heaps, ein Bind für die Laufzeit, Push-Daten. Auf ANV kommt
  die Größe des Ressourcen-Heaps aus einer eigenen Größenfunktion für den bindless Bereich
  (`anv_physical_device.c` Z. 2053 bis 2061). Ob die Kapazität dort höher liegt als bei Option 2 und ob der
  Heap-Pfad schneller ist, ist **offen** (M-2, M-3).
* **Nachteile:**
  * Nicht auf NVK (V-4). Auf NVIDIA erst ab Treiber 610, mit gemeldeten Stabilitätsproblemen (V-6).
  * Junge Erweiterung, Khronos will sie noch zu KHR weiterentwickeln (NVIDIA-Blog).
* **Kosten:** 4a ist ein zweiter Host-Pfad bei gleichem SPIR-V. 4b wäre ein zweiter Compiler-Pfad.
* **Risiko:** hoch für 4b, mittel für 4a (Treiberreife).
* **Widerlegt durch:** M-6 (Stabilität) oder M-2 (kein messbarer Vorteil gegenüber Option 2).

### Option 5: Mischform

Direkte Slots über klassische Sets oder Push-Descriptors (wie Option 1), Tier 2 und Metal 4 über Tabelle und Heaps
(wie Option 2).

* **Vorteile:** Binding Tables auf ANV für den häufigsten Fall. **Nachteile:** alle Nachteile von Option 1
  (zwei Modelle, Set-Knappheit, `setBytes`). **Widerlegt durch:** dieselbe Messung wie Option 1. Die Mischform ist
  der Rückfall, falls M-2 Option 2 auf der Iris widerlegt.

### Zusammenfassung

| | 1 | 2 | 3 | 4a | 4b | 5 |
|---|---|---|---|---|---|---|
| Ein Bindungsmodell | nein | ja | ja | ja | ja | nein |
| Gleiches SPIR-V auf allen Treibern (R-4) | ja | ja | ja | ja | **nein** (NVK) | ja |
| Läuft auf RTX / NVK / ANV / lavapipe | ja/ja/ja/ja | ja/ja/ja/ja | ja/ja/ja/ja | ≥ 610/**nein**/ja/ja | ≥ 610/**nein**/ja/ja | ja/ja/ja/ja |
| Indirektion pro Zugriff bei direkten Slots | nein | ja | ja | ja | ja | nein |
| `setBytes` > 128 B ohne Sonderweg | nein | ja | ja | ja | ja | nein |
| Status der Vulkan-Erweiterung | Kern | Kern 1.2 | deprecated | neu (EXT) | neu (EXT) | Kern |

## Entscheidung (vorgeschlagen)

**Option 2 ist das verbindliche Modell. Option 4a darf später als zweiter Host-Pfad hinzukommen, wenn M-2 und M-6 es
rechtfertigen. Option 3 wird nicht umgesetzt. Option 5 ist der benannte Rückfall, falls M-2 auf der Iris scheitert.**

**E-1 Eine SPIR-V-Form („Tabelle + Heaps“).** air2spirv erzeugt für jede Einstiegsfunktion genau diese Form,
unabhängig vom Treiber:

1. **Set 0 ist der globale Heap.** Bindings mit festen Nummern, jeweils `OpTypeRuntimeArray`, ohne
   `NonWritable`/Format-Annahmen über das Nötige hinaus. Mehrere Variablen mit verschiedenen `OpTypeImage`-Varianten
   dürfen dieselbe Bindung teilen (V-5a).

   | Binding | Inhalt | Deskriptortyp |
   |---|---|---|
   | 0 | Texturen zum Sampeln und Lesen | `SAMPLED_IMAGE` |
   | 1 | Texturen mit Schreib- oder Read-Write-Zugriff (Format `Unknown`) | `STORAGE_IMAGE` |
   | 2 | Sampler ohne Vergleich | `SAMPLER` |
   | 3 | Sampler mit Vergleichsfunktion (zweites Objekt desselben Metal-Samplers, gpu.md §4.4) | `SAMPLER` |
   | 4 | `texture_buffer` lesend | `UNIFORM_TEXEL_BUFFER` |
   | 5 | `texture_buffer` schreibend | `STORAGE_TEXEL_BUFFER` |

   Beschleunigungsstrukturen brauchen keinen Deskriptor. Sie werden aus der 64-Bit-Adresse erzeugt
   (`OpConvertUToAccelerationStructureKHR`).
2. **Set 1 ist für pass-lokale Deskriptoren reserviert.** Gedacht ist es für Input-Attachments, falls WP-GPU-18
   Framebuffer Fetch über Local Read nutzt. Sie werden per Push-Descriptor pro Pass gesetzt. Alle anderen Sets sind
   verboten.
3. **Push-Konstanten: ein Block von höchstens 128 B**, das ist das Kern-Minimum (V-1). Er wird für alle Stufen gemeinsam
   deklariert:

   | Offset | Inhalt |
   |---|---|
   | 0 bis 47 | `u64 table[6]`: Adresse der Argument-Tabelle je Metal-Stufe (Vertex, Fragment, Object, Mesh, Kernel, Tile). Jede Stufe liest nur ihr Feld. Teilen sich Stufen eine Metal-4-Tabelle, steht dieselbe Adresse mehrfach da |
   | 48 bis 71 | Dispatch-Daten für Compute (Gitter-Offset und -Größe, shader-compiler.md §4.14) |
   | 72 bis 127 | reserviert, Inhalt null |

   Die genaue Belegung legen WP-SC-12 und WP-GPU-07 gemeinsam fest. Sie gehört zum Cache-Schlüssel (als Version des
   Layouts in `a2s_get_version_string`).
4. **Argument-Tabelle: kompakt pro (Pipeline, Stufe).** air2spirv vergibt nur für benutzte Slots einen Platz und
   meldet ihn als `a2s_binding::table_offset`. Einträge:

   | Metal | Eintrag | Bedeutung |
   |---|---|---|
   | `[[buffer(n)]]` (auch `setBytes`) | `u64` | GPU-Adresse einschließlich Bind-Offset |
   | `[[texture(n)]]` | `u32` | Index im Heap (Binding 0 und/oder 1, siehe E-3) |
   | `[[sampler(n)]]` | `u32` | Index im Sampler-Heap (Binding 2 bzw. 3) |
   | Attribut-Stride (Metal 4, Vertex-Pulling) | `u32` | Stride in Byte |
   | `constexpr sampler` | `u32` | Index, vom Host beim Pipeline-Bau eingetragen (shader-compiler.md §4.5) |

   Ausrichtung ist 8 Byte für `u64`. Die größte mögliche Tabelle ist 31 × 8 + 31 × 4 + 128 × 4 + 16 × 4 B plus
   `constexpr`-Sampler, also unter 1 KB.
5. **Zugriff im Shader.**
   * Buffer: `u64` laden, als `PhysicalStorageBuffer`-Zeiger benutzen.
   * Textur oder Sampler aus einem direkten Slot: `u32` laden, begrenzen (Punkt 6), in das Runtime-Array indizieren.
     Der Index ist pro Draw dynamisch uniform, ein `NonUniform` wird **nicht** dekoriert.
   * Tier-2-IDs aus Argument Buffers: niedrige 32 Bit der `MTLResourceID` als Index, begrenzen, `NonUniform` nur,
     wenn die Uniformität nicht beweisbar ist.
6. **Begrenzung (R-3).** Jeder Heap-Index wird vor dem Zugriff auf `index < capacity` geprüft. Ungültige Werte werden
   auf Slot 0 umgelenkt, der immer den Null-Deskriptor enthält. `capacity` ist pro Heap eine Zweierpotenz und wird als
   Spezialisierungskonstante mit fester SpecId übergeben. Ein anderer Wert erzeugt also nur eine neue Pipeline, keine
   neue Übersetzung.

**E-2 Der Host hält die Heaps hinter einer Schnittstelle mit zwei Implementierungen.** In `ritsu-gpu-host` gibt es
ein internes Trait (Name im WP). Es kapselt: Heap anlegen, Slot belegen und freigeben, Deskriptor schreiben, Heap an
einen Command Buffer binden, Tabellen-Adresse übergeben.

* **`indexing` (Pflicht, Default):** ein Descriptor-Set aus einem Pool mit `UPDATE_AFTER_BIND` und
  `PARTIALLY_BOUND`, einmal pro Command Buffer gebunden, Push-Konstanten nach E-1.3. Läuft auf allen vier Treibern.
* **`ext_heap` (optional, Option 4a):** `VK_EXT_descriptor_heap` mit einer Abbildung von Set 0 auf die beiden Heaps,
  Push-Daten statt Push-Konstanten, **dasselbe SPIR-V**. Wird pro Treiber nur aktiviert, wenn M-2 einen Vorteil
  und M-6 Stabilität zeigt. Bis dahin ist es nicht im Default-Pfad.
* `VK_EXT_descriptor_buffer` wird nicht implementiert (V-2).

**E-3 Ressourcen-IDs und Lebensdauer der Slots.**

1. Jede Textur, jede Texture-View und jeder Texture-Buffer bekommt bei der Erzeugung **einen** Index *i*.
   * Erlaubt die Nutzung Lesen, steht unter *i* ein Deskriptor in Binding 0 (bzw. 4).
   * Erlaubt sie Schreiben, steht unter *i* einer in Binding 1 (bzw. 5).
   * Der Gast sieht `gpuResourceID = (Generation << 32) | i`. Der Shader wertet nur *i* aus. Die Generation dient der
     Host-Validierung direkter Bindungen und der Fehlersuche.
2. Sampler werden nach Deskriptor dedupliziert (gpu.md §4.4).
   * Ein Sampler mit Vergleichsfunktion belegt denselben Index in Binding 2 und 3.
   * Die Kapazität richtet sich nach `maxSamplerAllocationCount` (mindestens 4000, V-1) bzw. bei NVK nach 4096 (V-5).
   * Den Apple-Wert `maxArgumentBufferSamplerCount` meldet Ritsu höchstens bis zu dieser Kapazität. Er wird in
     S-GPU-1a gemessen.
3. Slot 0 jedes Arrays enthält immer den Null-Deskriptor (`nullDescriptor`).
4. Ein freigegebener Slot wird sofort auf den Null-Deskriptor gesetzt. Wiederverwendet wird er erst, wenn alle
   Submissions abgeschlossen sind, die vor der Freigabe eingereicht wurden (Timeline). So liest eine veraltete ID
   den Null-Deskriptor statt einer fremden Ressource.
5. **Kapazität voll** → Erzeugen der Ressource schlägt mit einem Gast-Fehler fehl und wird gezählt (R-8).
   Liegt die Kapazität einer GPU unter dem Apple7-Grenzwert (1M Texturen, MT-2), ist das eine Abweichung von
   ADR-0011 E-1. Sie wird nach M-3 gemessen und dort entschieden, siehe *Konsequenzen*.
6. **Wer die ID vergibt:**
   * Variante G: Der Host ist Eigentümer der Heaps. Wie der Gast-Treiber die ID ohne Rundreise erfährt, entscheidet
     der `gpu-engineer` im Protokoll-WP. Empfohlen ist, dass der Gast aus einem im `HelloAck` ausgehandelten Bereich
     selbst vergibt und der Host prüft (frei, im Bereich, nicht in Benutzung).
   * Variante P: Ob der Host die Kodierung von `gpuResourceID` und `gpuAddress` wählen kann, ist **[Spec nötig]**
     (S-GPU-1). Ist sie von Apple festgelegt, braucht es eine Übersetzungstabelle im GPU-Speicher. Das ist eine neue
     Indirektion, und dann muss dieser ADR überarbeitet werden.

**E-4 Direkte Bindungen pro Draw.**

* Der Encoder-Puffer zeichnet `set*`-Aufrufe auf.
* Vor einem Draw oder Dispatch mit geänderten Bindungen schreibt der Host eine **neue Version** der Tabelle der
  betroffenen Stufe in einen Upload-Ring pro Command Buffer (host-sichtbar, BDA) und pusht die Adresse.
  Unveränderte Tabellen werden wiederverwendet. Ältere Versionen bleiben gültig, solange der Command Buffer läuft.
  Das ist die Schnappschuss-Semantik von MT-4.
* `setBytes` kopiert die Bytes in denselben Ring und trägt deren Adresse als Buffer-Slot ein.
* Metal-4-Argument-Tabellen sind Host-Objekte mit demselben Inhalt. `setArgumentTable` erzeugt beim nächsten Draw eine
  Version der Tabelle.
* Bevor ein direkt gebundener Eintrag geschrieben wird, prüft der Host ihn gegen die Reflexion: Texturtyp,
  Sample-Typ, Zugriff (Lesen/Schreiben) und Gültigkeit von Handle und Generation. Bei einem Widerspruch gibt es einen
  Gast-Fehler. In Vulkan wäre das undefiniertes Verhalten (V-5a).

**E-5 Tier 1 und `MTLArgumentEncoder`.** Es gibt keine eigene SPIR-V-Form für Tier 1. Ritsu meldet in allen Profilen
mit Apple-Familie Tier 2 (ADR-0011). `MTLArgumentEncoder` schreibt in das Tier-2-Layout der C-Struktur, also
Adressen und IDs nach E-3. Bei G macht das der Gast-Treiber, bei P ist es **[Spec nötig]**. Option 1 wird damit auch
als Tier-1-Fallback nicht gebraucht.

**E-6 Hazard-Tracking und Image-Layouts** (Ergänzung zu gpu.md §7, ändert dessen Entscheidungen nicht).

1. **Zugriffsmenge eines Encoders:**
   * direkt gebundene Slots mit `access` aus der Reflexion,
   * Attachments,
   * `useResource`/`useHeap`,
   * bei Metal 4 die expliziten Barrieren.

   Shader, die Ressourcen über Argument Buffers oder rohe Zeiger erreichen, markiert die Reflexion
   (`indirect_resource_access`, siehe E-7). Ohne `useResource` behandelt sie der Tracker wie untracked
   (gpu.md §7.3).
2. **Deskriptor- und Tabellen-Writes** sind Host-Writes vor dem Submit. Sie brauchen keine GPU-Barriere. Die
   Sicherheit kommt allein aus der verzögerten Wiederverwendung (E-3.4) und den Ring-Versionen (E-4).
3. **Layouts:**
   * Ein Bild mit Heap-Slot steht immer in `VK_IMAGE_LAYOUT_GENERAL`, wenn es nicht gerade Attachment ist, denn der
     Deskriptor legt das Layout fest.
   * Der Tracker wechselt Attachments am Pass-Ende nach `GENERAL`.
   * Auf Treibern mit `VK_KHR_unified_image_layouts` ist das ohne Nachteil.
   * Auf ANV fehlt die Erweiterung (V-4). Ob `GENERAL` dort Kompression kostet, misst M-4. Bei mehr als 10 %
     Verlust wird für **direkt** gebundene Texturen ein zweiter Deskriptor mit `SHADER_READ_ONLY_OPTIMAL` eingeführt.
     Tier-2-Zugriffe bleiben bei `GENERAL`, weil der Host den Index im Gast-Speicher nicht kennt.

**E-7 Vertrag der Reflexion zwischen air2spirv und ritsu-gpu-host.** Die ABI ändert der `shader-compiler-engineer`.
Hier steht nur der Vertrag:

* `a2s_binding` gibt es pro benutztem Metal-Slot. Es enthält `kind`, `metal_index`, `access` und `table_offset`
  (immer gesetzt). `descriptor_set`/`binding` sind `A2S_NONE`, außer bei Set-1-Ressourcen.
* Neu für Textur-Slots: Texturtyp (Dimension, Array, MS, Depth) und Sample-Typ. Für Sampler-Slots: „wird mit
  Vergleich benutzt“.
* Neu in `a2s_reflection`: Größe der Argument-Tabelle je Stufe, Maske der benutzten Set-0-Bindings, Flag
  `indirect_resource_access`, SpecIds der Kapazitätskonstanten.
* Das PROVISIONAL-Kennzeichen fällt erst, wenn dieser ADR `Accepted` ist und WP-SC-12 den Vertrag umgesetzt hat.

## Konsequenzen

* **Positiv:**
  * Ein Bindungsmodell für direkte Slots, Tier 2 und Metal 4. Die heutigen Pläne in shader-compiler.md §5 und
    gpu.md §4.5 werden bestätigt, nicht umgeworfen.
  * Läuft vollständig auf lavapipe (R-6). Der Heap-Mechanismus ist austauschbar, ohne das SPIR-V oder den
    Shader-Cache anzufassen (R-4).
  * Gast-Indizes sind im Shader begrenzt (R-3). Veraltete IDs lesen den Null-Deskriptor.
* **Negativ / Kosten:**
  * Eine Indirektion pro Ressourcenzugriff. Auf der Iris zusätzlich die indirekte Deskriptorstufe (V-5). Der Preis
    ist unbekannt, bis M-2 läuft.
  * Bildressourcen stehen meist in `GENERAL`. Auf ANV kann das Bandbreite kosten (M-4).
  * Die Heap-Kapazität der Iris (etwa 500K Image Views) liegt möglicherweise unter dem Apple7-Grenzwert von 1M
    Texturen pro Stufe aus Argument Buffers. Das berührt ADR-0011 E-1 (Befund P-5 in R-ARCH-07).
  * Variante P kann E-3 brechen, falls Apple die Kodierung der IDs festlegt.
* **Abhängigkeitsregel ([ARCHITECTURE.md](../ARCHITECTURE.md) §4):** unverändert. Der Vertrag liegt in der C-ABI
  von air2spirv und in `ritsu-gpu-host`. `ritsu-gpu-proto` bekommt höchstens neue Felder (ID-Bereich im `HelloAck`)
  nach den bestehenden Regeln zur Vorwärtskompatibilität.
* **Folgearbeiten (Vorschläge, Freigabe durch den Orchestrator):**
  * **S-GPU-BIND-1 ★** (vor WP-GPU-07, `gpu-engineer`): Mikro-Benchmarks M-1, M-2, M-4 auf lavapipe, RTX und Iris.
  * **WP-SC-12/13/20:** SPIR-V-Form nach E-1, Begrenzung nach E-1.6, Reflexion nach E-7.
  * **WP-GPU-07:** Upload-Ring, Tabellen-Versionen und Push-Layout (E-4).
  * **WP-GPU-13:** Heap-Trait mit `indexing`, Slot-Lebensdauer, Sampler-Deduplizierung, Kapazität (E-2, E-3).
  * **WP-GPU-03:** Werte aus V-4 für den proprietären Treiber belegen und Kapazitätstest M-3.
  * **WP-GPU-22:** Metal-4-Tabellen nach E-4.
  * Protokoll-WP (Variante G): ID-Vergabe nach E-3.6.
  * gpu.md §3.2, §4.5, §7.4 und shader-compiler.md §5, §6 nach Freigabe nachziehen (zuständige Spuren, Liste im
    Bericht zu WP-ARCH-05).

## Überprüfung

Die Messungen laufen als S-GPU-BIND-1 mit Zeitstempel-Queries bzw. CPU-Zeit. Angegeben ist jeweils der Median aus
mindestens 500 Wiederholungen, auf lavapipe (nur Korrektheit), RTX 4050 mit proprietärem Treiber, RTX 4050 mit NVK
und Iris Xe mit ANV. Ergebnisse kommen als Bericht nach `docs/research/` und als Benchmark nach `bench/`
(CLAUDE.md §3.8).

| ID | Messung | Kriterium → Folge |
|---|---|---|
| M-1 | **CPU pro Draw:** 10 000 Draws, je Draw 2 Buffer, 4 Texturen und 1 Sampler neu gebunden. Option 2 (Ring-Version + Push) gegen Option 1 (Push-Descriptors) | Option 2 braucht mehr als 1,5-mal die CPU-Zeit von Option 1 → Ring-Implementierung überarbeiten. Bleibt es nach der Überarbeitung so, ist Option 5 zu prüfen |
| M-2 | **GPU-Kosten der Indirektion:** (a) Vollbild-Kompositionsszene mit 1, 4 und 16 gesampelten Texturen aus direkten Slots, (b) Compute-Kernel mit 8 Buffer-Slots. Verglichen werden Option 1 (Binding Tables bzw. klassische Sets), Option 2 und, wo vorhanden, Option 4a | Option 2 mehr als 10 % langsamer als Option 1 auf einer Ziel-GPU in (a) → Option 5 für diese GPU (neuer ADR). Option 4a mindestens 5 % schneller als Option 2 und M-6 erfüllt → `ext_heap` wird Default auf diesem Treiber |
| M-3 | **Kapazität:** Texturen und Views erzeugen, bis der Heap voll ist, auf jeder GPU | Kapazität unter 2²⁰ auf einer GPU → ADR-0011 E-1 bekommt eine dokumentierte Ausnahme für diesen Grenzwert, oder die GPU meldet kein Apple7 (Entscheidung durch den Menschen). Kapazität unter 2¹⁸ → E-3 überarbeiten (z. B. Views ohne eigenen Slot) |
| M-4 | **Layout auf ANV:** gleiche Szene wie M-2 (a) mit Texturen in `GENERAL` gegen `SHADER_READ_ONLY_OPTIMAL`, dazu Render-to-Texture-Bandbreite | mehr als 10 % Verlust → zweiter Deskriptor für direkte Slots (E-6.3) |
| M-5 | **Robustheit gegen feindliche Indizes:** Fuzz-Shader mit zufälligen Tier-2-IDs, falschen Texturtypen hinter gültigen Indizes und veralteten IDs, eine Stunde pro GPU | jedes `DEVICE_LOST` oder Hängen → Blocker. Abhilfe: Typklassen in eigenen Heap-Bereichen und Klassen-Bits in der ID (Änderung an E-1/E-3 per neuem ADR) |
| M-6 | **Stabilität von `ext_heap`:** Testszenen und M-5 im Dauerlauf, 24 h pro Treiber | ein `DEVICE_LOST` → `ext_heap` bleibt auf diesem Treiber aus |
| M-7 | **Korrektheit:** Bindless-Fixture (verschachtelte Argument Buffers, WP-SC-20) und Metal-4-Tabellen-Szene, Bildvergleich auf lavapipe und beiden GPUs, auf jedem aktiven Heap-Pfad | jede Abweichung → Blocker für die Freigabe des Pfads |

**Was die Entscheidung insgesamt kippt:** M-2 auf beiden Ziel-GPUs schlechter als 10 % (dann Option 5 überall), M-5
nicht lösbar ohne Typprüfung im Shader, oder S-GPU-1 zeigt für Variante P eine Apple-feste ID-Kodierung, die sich
nicht ohne zweite Indirektion abbilden lässt.
