# ADR-0011: Ritsu meldet als Ziel auf beiden GPUs genau das Apple7-Bodenprofil (Apple7 + Metal3 + Metal4); bis dahin gelten nur Zwischenprofile aus einer festen Menge, und ein nicht gemeldetes Feature scheitert immer laut

* **Status:** Proposed
* **Datum:** 2026-10-14
* **Entscheider:** Mensch (Freigabe, beantwortet zugleich O-6 / gpu.md G-4), `architect` (Entwurf, WP-ARCH-07)
* **Betrifft:** `crates/ritsu-gpu-host` (`caps`, Frontends, Validierung), `crates/ritsu-gpu-proto` (Fähigkeitsblock im
  `HelloAck`), `compiler/air2spirv` (Feature-Erkennung in Shadern), `guest/` (nur Variante G),
  [gpu.md](../subsystems/gpu.md) §4.3, §4.9, §5, [ADR-0005](0005-gpu-guest-path.md)

## Kontext

**Problem.** Der Gast fragt über `-[MTLDevice supportsFamily:]` und über einzelne `MTLDevice`-Eigenschaften ab, was
die GPU kann. Was Ritsu dort meldet, entscheidet zweierlei:

* (a) ob WindowServer, Core Animation, Core Image und SwiftUI/AppKit-Apps überhaupt laufen, und
* (b) welche Features Ritsu auf zwei IMR-GPUs ohne Tile-Speicher korrekt liefern muss. Die beiden GPUs sind die
  RTX 4050 (Ada) und die Iris Xe (Xe-LP). Regel CLAUDE.md §3.5 („niemals still falsch“) gilt ohne Ausnahme.

Der Code meldet heute nach `caps::reported_feature_set_with` die Familien Apple7, Mac2, Common3, Metal3 und Metal4
([gpu.md](../subsystems/gpu.md) §5.1). Die GPU-Spur hat in WP-GPU-02 festgestellt, dass Apples
Feature-Set-Tabellen 2026 keine Mac2-Spalte mehr haben (G-3). Die Annahme „Mac2 + Metal3 reicht für den Desktop“ aus
G-1 ist damit fraglich.

### Fakten (mit Quelle)

**F-1 Feature-Set-Tabellen, Ausgabe 21. Mai 2026** ([PDF](https://developer.apple.com/metal/Metal-Feature-Set-Tables.pdf),
abgerufen 2026-10-14; die frühere HTML-Seite `developer.apple.com/metal/feature-sets` leitet inzwischen auf dieses
PDF um):

* Die GPU-Liste enthält nur A8…A19 und M1…M5, also nur die Familien Apple2 bis Apple10. Es gibt keine Mac2-Spalte und
  keine Intel- oder AMD-Zeile. A14 und M1 und alles danach sind als „Metal 3 & 4“ markiert.
* Nach der Fußnote zu den Grenzwert- und Texturtabellen erreichen alle Metal-3- und Metal-4-Geräte mindestens die
  Grenzwerte und Texturfähigkeiten der **Apple7**-Spalte.
* Nach Fußnote 1 der Feature-Tabelle gibt es das Metal-3-Programmiermodell auf allen Apple-Familien, das
  Metal-4-Programmiermodell ab Apple7.

**F-2 Ab welcher Familie ein Feature verfügbar ist** (gleiche Quelle, Auszug der für IMR-GPUs kritischen Zeilen):

| ab | Features |
|---|---|
| Apple2 | Programmable Blending, getrennte Depth- und Stencil-Attachments, PVRTC, EAC/ETC, ASTC (LDR), Memoryless Render Targets, programmierbare Sample-Positionen, Deferred Store Action, Dual-Source-Blending |
| Apple3 | Extended-Range-Formate (`BGRA10_XR` …) mit allen Fähigkeiten, `RGB9E5Float` mit allen Fähigkeiten (inkl. Blend, MSAA), MSAA-Depth-Resolve, komprimierte Volumentexturen |
| Apple4 | **Imageblocks, Tile-Shader, Raster Order Groups**, Imageblock-Sample-Coverage, Post-Depth-Coverage |
| Apple6 | Argument Buffers Tier 2, Ray Tracing (Compute und Render), Funktionszeiger, dynamische Bibliotheken, Sparse-Texturen, Variable Rasterization Rate, Vertex Amplification, Texture-Atomics, Residency Sets, HDR-ASTC, SIMD-Permute |
| Apple7 | **Mesh-Shading**, **Float-Atomics**, SIMD-/Quad-Reduktionen, SIMD-Matrix-Multiplikation, Primitive ID, Baryzentrische Koordinaten, Border Color, Layered Rendering in MSAA-Texturen, Metal-4-API (Argument Tables, Command Allocators, Barrieren, Compiler-Kontexte), Tensoren und ML-Encoding |

BC-Formate stehen in der Tabelle erst bei Apple9. Laut Fußnote unterstützen aber alle Apple7- und Apple8-Geräte unter
macOS BC.

**F-3 MTLGPUFamily-Referenz** (Apple Developer Documentation, JSON der Symbolseiten, abgerufen 2026-10-14):

| Symbol | Status macOS |
|---|---|
| `MTLGPUFamily.mac2` | deprecated ab **27.0**, Ersatz laut Doku `MTLGPUFamily.apple7` |
| `MTLGPUFamily.common1` / `common2` / `common3` | deprecated ab **27.0**, Ersatz `apple1` / `apple3` / `apple5` |
| `metal3` (seit 13.0), `metal4` (seit 26.0), `apple6` … `apple10` | nicht deprecated |
| `MTLDevice.isLowPower`, `isHeadless`, `isRemovable`, `location`, `maxTransferRate`, `peerGroupID` | deprecated ab 27.0 |

Die `metal3`-Seite listet unter macOS noch AMD- und Intel-GPUs. Diese Liste ist veraltet und widerspricht F-1 nicht.

**F-4 Metal 4 setzt M1 bzw. A14 oder neuer voraus.** Quellen: WWDC25 Session 205 „Discover Metal 4“ und die
Apple-Support-Seite [102894](https://support.apple.com/en-us/102894) (Stand 2026-03-10), die Metal 4 nur für Macs mit
Apple Silicon nennt.

**F-5 macOS 26 Tahoe ist die letzte Version für Intel-Macs** (WWDC25 Platforms State of the Union, z. B.
[PetaPixel](https://petapixel.com/2025/06/10/macos-26-tahoe-is-the-end-of-the-road-for-intel-macs/)). Intel- und
AMD-GPUs sind Metal-3-Geräte ohne Apple-Familie (F-3, `metal3`-Seite). Bis macOS 26 liefen WindowServer, Core
Animation und SwiftUI also auf Geräten mit Mac2 + Metal3 ohne Apple-Familie, allerdings im **x86_64**-Build der
Frameworks. macOS 27 läuft nur auf Apple Silicon ([ARCHITECTURE.md](../ARCHITECTURE.md) §2).

**F-6 Paravirtuelle GPU in macOS-VMs** (Virtualization.framework). Es gibt nur öffentliche Drittberichte und keine
Apple-Dokumentation zur gemeldeten Familie. Gerätenamen und Einstellungsschlüssel werden hier nicht zitiert
([ADR-0008](0008-clean-room.md)).

* [Metal.jl PR #789](https://github.com/JuliaGPU/Metal.jl/pull/789) (gemergt 2026-06-04, macOS-15-Gast auf M3 Pro):
  Das paravirtuelle Gerät läuft auf echter Apple-GPU, meldet über `supportsFamily:` aber **weder Apple7 noch Metal3**.
  Residency Sets und `MTLLogState` scheitern.
* [Cua-Blog](https://github.com/trycua/cua/blob/main/blog/gpu-passthrough-macos-vms.md) (2026-08-11, Gast macOS 26.5.2,
  Host M1 Ultra mit macOS 26.6.1): Der Gast meldet ab Werk ungefähr **Apple5**, 32 KB Threadgroup-Speicher, keine
  SIMD-Matrix/-Reduktion/bfloat und kein Metal3 (Residency Sets fehlen). Der Bericht nennt eine hostseitige
  Einstellung für eine „uneingeschränkte Feature-Stufe“ und zeigt, dass das Gerät neuere Kernel trotz der niedrigen
  Meldung ausführt.
* [WWDC26 Session 224](https://developer.apple.com/videos/play/wwdc2026/224/): In macOS 27 können macOS-Gäste
  „Metal-Features wie Argument Buffers und Indirect Command Buffers“ nutzen.
* Die VM-Beispiele von Apple zeigen einen vollständigen grafischen Desktop mit `VZMacGraphicsDeviceConfiguration`
  ([Running macOS in a virtual machine on Apple silicon](https://developer.apple.com/documentation/virtualization/running-macos-in-a-virtual-machine-on-apple-silicon)).

**F-7 Vulkan-Treiber der Ziel-GPUs** (öffentlicher Mesa-Quelltext, Zweig `main`, Stand 2026-10-14: `anv_physical_device.c`,
`isl_format.c`, `intel_device_info.c`, `nvk_physical_device.c`):

| Feature | ANV / Iris Xe (gfx12) | NVK / RTX 4050 | proprietär / RTX 4050 |
|---|---|---|---|
| `textureCompressionETC2` | **ja** (fest `true`) | nein | **Annahme** nein (gpu.md §3.2) |
| ASTC LDR | **ja, nativ bis gfx12**; ab gfx12.5 aus der Hardware entfernt (Kommentar in `isl_format.c`), dort Emulation | nur Tegra-SoCs | **Annahme** nein |
| ASTC HDR (`textureCompressionASTC_HDR`) | nein | nein | **Annahme** nein |
| `fragmentShaderPixelInterlock` | ja | **nein** (Erweiterung fehlt) | **Annahme** ja |
| `dynamicRenderingLocalRead` | ja | ja | **Annahme** ja |
| `shaderBufferFloat32AtomicAdd` | nein (nur mit LSC, ab gfx12.5) | ja | **Annahme** ja |
| Mesh-Shader / Ray Tracing | nein (erst XeHP, gfx12.5) | Mesh ab Turing; RT **Annahme** nein | **Annahme** ja / ja |
| `fragmentShaderBarycentric` | nein (erst Xe2, ver ≥ 20) | ja (Turing+) | **Annahme** ja |
| `requiredSubgroupSizeStages` | nur Compute, Task, Mesh (**nicht Fragment**) | – | **Annahme** Warp = 32 überall |
| `VK_EXT_sample_locations` | ja | ja | **Annahme** ja |
| `VK_EXT_rasterization_order_attachment_access` | nein | nein | **Annahme** nein |

**F-8 Vulkan-Spezifikation, Local Read** ([Vulkan-Docs](https://github.com/KhronosGroup/Vulkan-Docs),
`chapters/renderpass.adoc`, Flag `VK_RENDERING_ATTACHMENT_INPUT_ATTACHMENT_FEEDBACK_BIT_KHR`). Auch mit Local Read
braucht es eine `BY_REGION`-Barriere, wenn ein Fragment ein Attachment schreibt und ein späteres anderes Fragment
dieselbe Stelle als Input-Attachment liest. Barrieren gibt es nur **zwischen** Draws. Die Reihenfolge überlappender
Fragmente **innerhalb** eines Draws garantiert erst `VK_EXT_rasterization_order_attachment_access` oder Fragment
Shader Interlock.

### Befunde zum Ist-Stand in Ritsu

* **B-1** Die Apple7-Konjunktion in `caps::reported_feature_set_with` ist unvollständig. Es fehlen Mesh-Shading
  (Apple7), getrennte Depth- und Stencil-Attachments (Apple2, laut gpu.md §4.8 `Unsupported`), programmierbare
  Sample-Positionen (Apple2, `Unsupported`), die vollen Fähigkeiten von XR- und `RGB9E5`-Formaten (Apple3), die
  Apple7-Grenzwerte und die Shader-Features (SIMD-Reduktion und -Matrix, Barycentrics). Die Grenzwerte sind
  `min(Gerät, APPLE7)`. Ein Gerät unter Apple7 bekäme also `apple7 = true` mit Grenzwerten unter Apple7, und das
  widerspricht F-1.
* **B-2** gpu.md §5.1 setzt Framebuffer Fetch auf „N (local read)“. Nach F-8 ist das nur korrekt, wenn sich Fragmente
  desselben Draws nicht überlappen. Sonst ist das Ergebnis **still falsch**. Programmable Blending auf Apple-GPUs
  ordnet überlappende Fragmente auch innerhalb eines Draws (z. B. Partikel in einem Draw). Gebraucht werden Interlock
  (proprietär, ANV) oder ROAA (auf keiner der beiden GPUs vorhanden).
* **B-3** Die Annahme in gpu.md §4.3, Gen12 habe keine ASTC-Einheit, widerspricht F-7. Die Iris Xe hat ASTC LDR und
  ETC2/EAC nativ. Nur ASTC HDR und PVRTC fehlen dort.
* **B-4** gpu.md §4.9 sagt, ohne Tile-Shader-Emulation melde Ritsu keine Apple-Familie. Das stimmt mit F-2 überein:
  Schon Apple4 und damit auch das „reduzierte“ Paravirt-Profil (≈ Apple5, F-6) enthält Tile-Shader, Imageblocks, ROG,
  Programmable Blending, Memoryless und ASTC/ETC/PVRTC.

### Annahmen

* **A-1 (Annahme)** Apple testet die Systemkomponenten von macOS 27 nur gegen zwei Konfigurationen: (i) echte GPUs
  ab Apple7 und (ii) das paravirtuelle Gerät in VMs. Ob der arm64-Build in macOS 27 noch einen Pfad ohne Apple-Familie
  hat, ist unbekannt. Bis macOS 26 ist ein solcher Pfad nur für x86_64 belegt (F-5).
* **A-2 (Annahme)** Die gemeldete Familie begrenzt nicht, was Systemkomponenten tatsächlich benutzen. arm64-Code kann
  Memoryless, Programmable Blending oder ASTC ohne `supportsFamily:`-Abfrage nutzen, weil jede Apple-Silicon-GPU sie
  hat. Beim paravirtuellen Gerät führt Apples Host alles auf einer echten Apple-GPU aus und bemerkt solche Nutzung
  nicht (F-6: „führt neuere Kernel trotz niedriger Meldung aus“).
* **A-3 (Annahme)** Das Paravirt-Profil eines macOS-27-Gasts weicht von macOS 15/26 ab und ist eher höher (F-6,
  WWDC26).
* **A-4 (Annahme)** Ein macOS-Volume auf Macs mit verschiedenen GPU-Familien zu booten (externe Startvolumes,
  Migration) ist ein von Apple getestetes Szenario. Wenn sich das gemeldete Profil zwischen zwei Boots ändert, verträgt
  der Gast das also, solange jedes Profil für sich realistisch ist.
* **A-5 (Annahme)** Für Mesh-Shading, Float-Atomics, Tile-Shader, Imageblocks, Programmable Blending, Memoryless und
  ASTC/ETC/PVRTC gibt es keine eigene `MTLDevice`-Abfrage. Diese Features kennt der Gast nur über die Familie.
* **A-6 (Annahme)** Echte Apple-Silicon-Macs beantworten `supportsFamily:` mit YES für Mac2 und Common1–3 und
  zusätzlich für ihre Apple-Familie. Das sagt Apples frühere Feature-Set-Seite und WWDC20 „Bring your Metal app to
  Apple silicon Macs“. Ob macOS 27 das trotz der Deprecation (F-3) beibehält, ist ungeprüft.

### Abfrageklassen

Für die Ehrlichkeit macht es einen Unterschied, ob ein Feature nur über die Familie (**K1**) oder auch über eine eigene
`MTLDevice`-Eigenschaft (**K2**) sichtbar ist. Ein K2-Feature kann Ritsu bei gemeldeter Familie einzeln mit NO
beantworten. Gegenüber echter Hardware ist das eine Abweichung, aber eine laute: Die App fragt ab oder bekommt beim
Erzeugen `nil`. Bei K1 geht das nicht. Wer Apple7 meldet, sagt jedes K1-Feature von Apple2 bis Apple7 zu.

K2-Abfragen, die es laut Apple-Doku unter macOS gibt (abgerufen 2026-10-14): `supportsRaytracing`,
`supportsRaytracingFromRender`, `supportsFunctionPointers(FromRender)`, `supportsDynamicLibraries`,
`supportsRenderDynamicLibraries`, `supportsShaderBarycentricCoordinates`, `supportsRasterizationRateMap(layerCount:)`,
`supportsVertexAmplificationCount(_:)`, `areRasterOrderGroupsSupported`, `areProgrammableSamplePositionsSupported`,
`supportsQueryTextureLOD`, `supportsBCTextureCompression`, `supports32BitFloatFiltering`, `supports32BitMSAA`,
`supportsPullModelInterpolation`, `supportsTextureSampleCount(_:)`, `supportsPlacementSparse` (seit 26.4),
`argumentBuffersSupport`, `readWriteTextureSupport`.

### Anforderungen

| ID | Anforderung | Quelle |
|---|---|---|
| R-1 | (a) Desktop und SwiftUI-/AppKit-Apps laufen | ARCHITECTURE.md §1, M5 |
| R-2 | (b) Nichts wird still falsch gerendert. Was nicht korrekt geht, wird nicht gemeldet oder scheitert laut | CLAUDE.md §3.5 |
| R-3 | Dasselbe Gast-Image bootet auf RTX und auf Iris (Wahl der GPU vor dem Boot) | gpu.md §5.2 Regel 4 |
| R-4 | Die Regel gilt für Frontend P und Frontend G | ADR-0005 (protokollneutraler Kern) |
| R-5 | Auf Proposed-ADRs wird nichts Irreversibles gebaut | CLAUDE.md §3.6 |

## Optionen

### Kosten pro GPU, wenn Apple7 vollständig gemeldet wird

N = nativ, E = Emulation nötig, – = auf diesem Treiber nicht korrekt machbar. Speicherfaktoren sind gerechnet, alle
Aufwands- und Laufzeitangaben sind **Annahmen (Schätzung)** und durch die Spikes unter *Überprüfung* zu ersetzen.

| Feature (ab Familie, Klasse) | RTX proprietär | NVK | Iris Xe (ANV) | Mechanismus und Kosten |
|---|---|---|---|---|
| ASTC LDR (Apple2, K1) | E | E | **N** | Dekodieren nach `RGBA8` beim Upload. Speicher ×4 (4×4) bis ×36 (12×12). Aufwand M (WP-GPU-05) |
| ASTC HDR (Apple6, K1) | E | E | E | nach `RGBA16Float`, ×8 bis ×72 |
| ETC2/EAC (Apple2, K1) | E | E | **N** | nach `RGBA8`/`R16`/`RG16`, ×4 bis ×8. Aufwand S |
| PVRTC (Apple2, K1) | E | E | E | nach `RGBA8`, ×8 (4 bpp) bzw. ×16 (2 bpp). Aufwand M, braucht Referenzbilder von Apple-Hardware (O-2) |
| Programmable Blending (Apple2, K1) | E (Interlock + Storage-Image) | – | E (Interlock) | Local Read allein reicht nicht (B-2). Interlock serialisiert überlappende Fragmente, und auf Intel ist das Attachment dabei womöglich unkomprimiert (**Annahme**). Aufwand L |
| Memoryless (Apple2, K1) | E | E | E | transiente Attachments, billig |
| Getrennte Depth/Stencil-Attachments (Apple2, K1) | E | E | E | kombiniertes `D32S8`-Schattenbild + Kopien. Aufwand M |
| Programmierbare Sample-Positionen (Apple2, K2) | N | N | N | `VK_EXT_sample_locations` |
| XR-Formate, `RGB9E5` als Render-Target mit Blending (Apple3, K1) | E | E | E | Proxy-Format + Packen. Blending über denselben Interlock-Pfad wie Programmable Blending. Aufwand M (WP-GPU-15, G-6) |
| Tile-Shader, Imageblocks (Apple4, K1) | E | – (explizite Imageblocks brauchen Interlock) | E | Pass-Split (Rendering beenden, Compute über Kacheln, mit `LOAD` fortsetzen) und Buffer pro Pixel für explizite Imageblocks: 1920×1080 × ≤ 64 B ≈ 133 MB, mit 4× MSAA ≈ 531 MB. Zusätzliche Bandbreite pro Split ≈ 2 × Attachment-Größe. Aufwand XL, ★ (WP-GPU-18) |
| Raster Order Groups (Apple4, K2) | N | – | N | Interlock |
| Argument Buffers Tier 2 (Apple6, K2) | N | N | N | BDA + Descriptor Buffer/Indexing |
| Ray Tracing (Apple6, K2) | N | – | E oder K2 = NO | Compute-BVH, Aufwand XL (WP-GPU-21) |
| Variable Rasterization Rate (Apple6, K2) | K2 = NO | K2 = NO | K2 = NO | Fragment Shading Rate ist keine exakte Entsprechung (die Koordinatenabbildung fehlt) |
| Vertex Amplification (Apple6, K2) | N | N | N | Multiview |
| Mesh-Shading (Apple7, K1) | N | N | E | Compute + indirekte Draws. Aufwand XL (WP-GPU-20) |
| Float-Atomics (Apple7, K1) | N | N | E | CAS-Schleife für Buffer-Add. Aufwand S–M |
| SIMD-/Quad-Reduktion, SIMD-Matrix (Apple6/7, K1) | N | N | E mit Restrisiko | In Fragment-Shadern lässt sich die Subgroup-Breite 32 auf ANV nicht erzwingen (F-7). Matrix: Emulation im Shader |
| Barycentrics (Apple7, K2) | N | N | K2 = NO | – |
| Metal-4-API (Apple7, K1 über `metal4`) | E | E | E | API-Abbildung (WP-GPU-22) |
| Tensoren, ML-Encoding (Apple7/Metal 4, K1) | offen | offen | offen | Umfang unbekannt (G-9), größte Unbekannte für `metal4` |
| Apple7-Grenzwerte (F-1, K1) | **Annahme** ja | ? | **Annahme** ja | durch `vulkaninfo` belegen (WP-GPU-03) |

**Folgerung aus der Tabelle:** Auf NVK ist eine Apple-Familie ohne Interlock nicht korrekt meldbar (relevant für
G-16). Der teure Kern, also Tile-Shader, Imageblocks, Programmable Blending und die Texturformate, ist für **jede**
Apple-Familie ab Apple4 nötig, auch für das reduzierte Paravirt-Profil.

### Option 1: Apple7 vollständig auf beiden GPUs, als einziges Profil

Ritsu meldet erst dann irgendetwas, wenn Apple7 + Metal3 + Metal4 auf beiden GPUs komplett ist. Davor gibt es keinen
Desktop.

* **Vorteile:** Das entspricht exakt echter macOS-27-Hardware (F-1, F-4) und damit der einzigen Konfiguration, die
  Apple sicher testet. Apps einschließlich iOS-/iPadOS-Apps auf dem Mac (ASTC, PVRTC) und Metal-4-Apps laufen. Die
  Meldung ist auf beiden GPUs gleich (R-3).
* **Kosten:** alle Zeilen der Tabelle, auf Iris zusätzlich Mesh und Float-Atomics. M5 hängt an WP-GPU-18, -20, -22 und
  an G-9 (ML).
* **Risiko:** Der Zeitplan (R-6 in ARCHITECTURE.md) verschiebt M5 um mehrere große ★-Pakete. Auf NVK ist das nicht
  erreichbar.
* **Widerlegt durch:** S-GPU-2a (siehe *Überprüfung*). Zeigt der Spike, dass der Desktop mit Profil M ohne jede
  Apple-Feature-Nutzung läuft, muss Option 1 für M5 nicht vollständig sein.

### Option 2: reduziertes Profil

* **2a – Profil V (Abbild des Paravirt-Profils).** Ritsu meldet genau die Familien, Grenzwerte und K2-Antworten, die
  ein macOS-27-Gast unter Virtualization.framework meldet. Das muss erst gemessen werden.
  * **Vorteile:** Es ist das einzige reduzierte Profil, gegen das Apple macOS 27 selbst testet (A-1). Es spart Mesh,
    Float-Atomics, Metal 4, SIMD-Matrix und RT.
  * **Kosten:** Der teure TBDR-Kern bleibt (B-4). Die Messung braucht einen Mac (O-2).
  * **Risiko:** Das Profil von macOS 27 ist unbekannt (A-3). Apps, die Metal 3/4 voraussetzen, verweigern den Start.
    Das ist ehrlich, aber weniger kompatibel.
  * **Widerlegt durch:** S-GPU-1a. Meldet der macOS-27-Gast mindestens Apple7, ist V gleich A7 und Option 2a fällt
    weg.
* **2b – Profil M (Mac2 + Metal3 ohne Apple-Familie, „wie ein Intel-Mac“).**
  * **Vorteile:** Auf der RTX ist das heute nativ möglich. Auf der Iris ist Mac2 nativ, Metal3 nur mit
    Mesh-Emulation. WP-GPU-18 wäre für M5 nicht nötig.
  * **Kosten:** am niedrigsten.
  * **Risiko:** für (a) am höchsten. Mac2 ist in 27 deprecated und als Ersatz `apple7` genannt (F-3). Der arm64-Code
    von macOS ist nie mit einer Nicht-Apple-GPU gelaufen (A-1). Nutzt eine Systemkomponente Apple-Features ohne
    Abfrage (A-2), scheitert sie laut. iOS-Apps mit ASTC oder PVRTC scheitern ebenfalls.
  * **Widerlegt durch:** S-GPU-2a.

### Option 3: pro GPU das ehrlich jeweils Maximale

Die RTX meldet so früh wie möglich Apple7 (eventuell später Apple8+ wie 64-Bit-Atomics), die Iris bleibt darunter.

* **Vorteile:** Die RTX erreicht früher volle Features.
* **Kosten:** Die Testmatrix verdoppelt sich.
* **Risiko:** Es entstehen Kombinationen, die es auf echter Hardware nie gibt, z. B. Apple7 ohne Metal4 oder
  Apple9-Einzelfeatures ohne Apple8. Apple testet diese Kombinationen nicht. Wechselt der Gast zwischen zwei Boots die
  GPU, ändert sich die Familie. Das ist nach A-4 nur unkritisch, wenn beide Profile realistisch sind. Betrifft O-6.
* **Widerlegt durch:** einen Wechseltest (gleiches Image abwechselnd auf RTX und Iris), der Fehler in persistiertem
  Zustand zeigt (Caches, App-Einstellungen).

### Option 4 (vorgeschlagen): feste Profilmenge, Ziel A7 auf beiden GPUs, strenge Durchsetzung

Eine Kombination aus 1, 2 und 3 mit klaren Grenzen, siehe *Entscheidung*.

* **Vorteile:** Es gibt nur realistische Profile, und das Ziel ist Hardware-konform. Ein früher Desktop ist möglich,
  falls S-GPU-2a es erlaubt. K2 entlastet die Iris (RT, Barycentrics, VRR = NO).
* **Kosten:** Die Profilmenge muss im Code gepflegt werden, und es braucht einen Profil-Validator.
* **Risiko:** Gilt nach den Spikes nur Profil A7 als lauffähig, gibt es keinen frühen Desktop. Option 4 fällt dann
  auf Option 1 zurück, ohne dass Arbeit verloren geht.
* **Widerlegt durch:** dieselben Spikes wie 1–3. Option 4 ist so geschnitten, dass jedes Spike-Ergebnis nur die
  Zwischenprofile ändert, nicht das Ziel.

## Entscheidung (vorgeschlagen)

**E-1 Erlaubte Profile.** Ritsu meldet dem Gast ausschließlich eines der folgenden Profile. Das Profil ist pro Boot
fest (Host-Konfiguration vor dem Gast-Start) und gilt für alle Abfragen des Boots.

| Profil | Familien = YES | Grenzwerte | Rolle |
|---|---|---|---|
| **A7** (Ziel) | Apple1–Apple7, Metal3, Metal4. Mac2 und Common1–3 so, wie echte Apple7-Hardware unter macOS 27 antwortet (A-6, zu messen) | genau Apple7 (nie darunter, nie darüber) | Ziel auf **beiden** GPUs |
| **A7⁻** | wie A7, aber Metal4 = NO | genau Apple7 | Zwischenprofil, solange Metal-4-API/ML (WP-GPU-22, G-9) fehlt |
| **V** | exakt das gemessene macOS-27-Paravirt-Profil (Familien, Grenzwerte, K2) | wie gemessen | Zwischenprofil, erst nach S-GPU-1a |
| **M** | Mac2, Common1–3, Metal3 (nur wenn Mesh N/E), keine Apple-Familie | Minimum aus Gerät und Apple7 | nur für Bring-up (M4) und S-GPU-2a. Default für Nutzer nur, wenn S-GPU-2a es freigibt |

Nicht erlaubt sind: Apple8 oder höher und einzelne Apple8+-Features ohne neuen ADR, Apple7 ohne Metal3, eine
Apple-Familie mit Grenzwerten unter ihrer Spalte und jede andere Mischform.

**E-2 Familie heißt alle K1-Features.** Eine Apple-Familie N wird nur gemeldet, wenn **jedes** K1-Feature der Spalten
Apple2 bis N laut F-2 und F-1 (Texturfähigkeiten aller Formate, Grenzwerte) nativ oder durch eine implementierte und
per Bildvergleich getestete Emulation korrekt ist (gpu.md §5.2 Regel 1 bleibt). Die Zuordnung lebt als **eine**
Tabelle im Code, die Zeile für Zeile auf die Ausgabe 2026-05-21 der Feature-Set-Tabellen verweist. `apple7` ist eine
Konjunktion über diese Tabelle und keine Auswahl (behebt B-1).

**E-3 K2 darf einzeln NO sein.** Bei gemeldeter Familie darf Ritsu ein K2-Feature mit NO beantworten, wenn es auf der
GPU nicht korrekt machbar ist. Für A7 gilt das z. B. für Ray Tracing und Barycentrics auf der Iris und für VRR überall.
Das jeweilige Erzeugen (`newAccelerationStructure…`, Rate-Map) liefert dann `nil` oder einen Gast-Fehler.

**E-4 Laut scheitern.** Der Host prüft jeden Befehl, jede Pipeline und jeden Shader gegen das **aktive Profil**, nicht
nur gegen die Gerätefähigkeit. Beispiele: Tile-Dispatch, Imageblock-Zugriff, `[[color(n)]]`-Eingang, ASTC-Upload,
Mesh-Pipeline, Float-Atomic im AIR. Nutzt der Gast ein Feature außerhalb des Profils, folgt `Unimplemented`: Der
Command Buffer wird vergiftet, Pipeline oder Textur werden nicht erzeugt. Die Ausführung wird nie angenähert. Jeder
solche Fall wird mit Feature und aufrufendem Gast-Prozess (soweit bekannt) gezählt („Feature-Zensus“). Das gilt für
Frontend P und G gleichermaßen. Bei P ist es die einzige Sicherung gegen A-2.

**E-5 Nur ordnungstreue Programmable-Blending-Emulation.** Framebuffer Fetch, `RGB9E5`- und XR-Blending sowie
explizite Imageblocks gelten nur mit pixelgenauer Rasterreihenfolge auch innerhalb eines Draws als korrekt (Interlock
oder ROAA). Local Read mit Barrieren zwischen Draws reicht nicht (B-2, F-8). Ohne Interlock gibt es keine
Apple-Familie, deshalb hat NVK höchstens Profil M.

**E-6 Ziel: dasselbe Profil auf beiden GPUs** (Vorschlag zu O-6/G-4). Das Ziel ist A7 auf RTX **und** Iris. Ritsu
meldet nichts über Apple7, auch auf der RTX nicht. Die Apple7-Spalte ist die Untergrenze von macOS 27 (F-1), ein
höheres Profil brächte für Systemkomponenten nichts und bräche R-3. Bis das Ziel erreicht ist, dürfen RTX und Iris
**verschiedene** Profile aus E-1 melden (z. B. RTX A7⁻, Iris V). Das ist nach A-4 tragbar, weil jedes Profil
realistisch ist.

**E-7 Reihenfolge der Zwischenprofile.** Der Default pro GPU ist das höchste erreichte Profil in der Reihenfolge
A7 > A7⁻ > V > M. M ist nur dann ein Default für Nutzer, wenn S-GPU-2a zeigt, dass der Desktop damit laut Zensus ohne
abgewiesene Apple-Features läuft.

**E-8 Der Fähigkeitsblock bleibt kompatibel.** Der `HelloAck` (gpu.md §9.3) trägt schon Familien-Bits und
`SupportLevel`s. Neue Profil-Information (Apple-Familien 1–6, K2-Bits) kommt nur über neue Familien-Bits bzw. ein
neues Feature-Bit nach den bestehenden Regeln zur Vorwärtskompatibilität. Diese ADR ändert keine eingefrorene Payload.

## Konsequenzen

* **Positiv:**
  * Jedes gemeldete Profil hat ein reales Vorbild (echte Hardware oder Apples VM).
  * Das Endziel hängt nicht vom Ausgang von ADR-0005 ab.
  * (b) ist durch E-2, E-4 und E-5 prüfbar abgesichert.
  * Der Feature-Zensus liefert erstmals Daten darüber, was macOS 27 wirklich nutzt.
  * Die Iris ist bei Texturformaten näher an Apple7 als angenommen (B-3).
* **Negativ / Kosten:**
  * Für jedes Apple-Profil ist der TBDR-Kern Pflicht, also Tile-Shader, Imageblocks, ordnungstreues Programmable
    Blending, getrennte Depth/Stencil-Attachments und Formate. Ein „billiges“ Apple-Profil gibt es nicht (B-4).
  * Programmable Blending über Interlock ist langsamer als über Local Read.
  * Auf NVK gibt es keine Apple-Familie (G-16).
  * Ohne Spike-Freigabe für M hängt M5 an WP-GPU-18.
* **Auswirkung auf [ADR-0005](0005-gpu-guest-path.md) (P gegen G):** Die Familienpolitik entscheidet P/G nicht, sie
  verschiebt aber die Kriterien:
  * **Für P:** Profil V ist bei P per Konstruktion „gratis“ und von Apple getestet.
  * **Gegen P:**
    * Der Gast-Treiber legt fest, was gemeldet wird. Ob der Host die Stufe steuern kann, ist offen (F-6 nennt einen
      hostseitigen Schalter, **Annahme**).
    * Für das Ziel A7 müsste das Protokoll eine Apple7-Meldung mit K2 = NO tragen können. Sonst braucht die Iris auch
      RT- und Barycentrics-Emulation.
    * Nach A-2 kann der Treiber Features außerhalb der Meldung senden. Der Host muss sie dann laut abweisen (E-4),
      und der Desktop bricht.
  * **Für G:** Jedes Profil aus E-1 ist einstellbar, K2 ist frei.
  * **Gegen G:** Jedes G-Profil ist eine Treiber-Konfiguration, die Apple für macOS 27 nie getestet hat.
  * **Neue Pflichtfragen:** an S-GPU-1 „Welches Profil meldet ein macOS-27-Gast? Ist es hostgesteuert? Werden
    K2-Eigenschaften übertragen?“, an S-GPU-2 „Was passiert mit Profil M?“.
* **Auswirkung auf die Emulations-WPs:**
  * **WP-GPU-18 ★:** wird Voraussetzung für **jede** Apple-Familie, also für A7, A7⁻ und V. Liegt auf dem kritischen
    Pfad zu M5, solange S-GPU-2a M nicht freigibt. Neu im Umfang:
    * Programmable Blending über Interlock statt Local Read (E-5),
    * getrennte Depth/Stencil-Attachments (Apple2),
    * Akzeptanz mit einer Testszene „überlappende Fragmente in einem Draw“.
    * Empfehlung: vorher einen Tile-Shader-Spike S-GPU-18a ziehen.
  * **WP-GPU-05:**
    * ASTC-Dekoder erweitern um ETC2/EAC, PVRTC und ASTC HDR, Akzeptanz bit-genau gegen Referenzdekoder
      (PVRTC gegen Apple-Referenzbilder, O-2).
    * Auf der Iris ASTC LDR und ETC2 nativ nutzen und nur HDR und PVRTC transkodieren.
    * Speicherfaktoren bis ×36 bzw. ×72 im Budget (§8.3) berücksichtigen. ASTC → BC7 bleibt die verlustbehaftete
      Option nach G-5 und erfüllt E-2 nicht.
  * **WP-GPU-15 ★:**
    * XR-Formate und `RGB9E5` brauchen unter A7 **alle** Fähigkeiten einschließlich Blend und MSAA (F-2, Apple3).
    * Blending in Proxy-Formaten läuft über den Interlock-Pfad aus WP-GPU-18.
    * Die Quantisierung muss pro Draw genau wie im Zielformat erfolgen. Sonst entstehen still abweichende Werte
      (höhere Präzision im Proxy).
  * **WP-GPU-20 / -21 / -22:** Mesh auf der Iris ist für A7 und A7⁻ Pflicht (K1). RT auf der Iris ist für A7 nicht
    Pflicht (K2 = NO, E-3). WP-GPU-22 plus G-9 trennt A7⁻ von A7.
* **Folgearbeiten (Vorschläge, Freigabe durch den Orchestrator):**
  * **WP-GPU-24 „Familientabelle + Profile“** (`caps`): Tabelle Feature → (Familie, K1/K2, Support), Profile aus E-1,
    `reported_feature_set_with` gibt ein Profil zurück, Tests gegen die Tabelle, Grenzwert-Bedingung.
  * **WP-GPU-25 „Profil-Durchsetzung + Feature-Zensus“** (Frontend G, `NullBackend`, air2spirv-Reflexion für
    Shader-Features).
  * **S-GPU-1a / S-GPU-2a / S-GPU-18a** als Teil von S-GPU-1, S-GPU-2 bzw. vor WP-GPU-18 (siehe unten).
  * gpu.md §4.3, §4.9, §5 nachziehen (gpu-engineer, nach Freigabe).

## Überprüfung

Das **Ziel** A7 kippt nur, wenn macOS 27 auf echter Hardware nicht mehr Apple7 als Untergrenze hat. Das wäre eine
neue Feature-Set-Tabelle mit niedrigerer Untergrenze, was unwahrscheinlich ist. Die **Zwischenprofile** und die
Reihenfolge der Arbeit kippen durch diese Experimente:

| Experiment | Methode | Ergebnis → Folge |
|---|---|---|
| **S-GPU-2a** (Teil von S-GPU-2, beantwortet G-1) | G-Prototyp meldet im macOS-27-Gast Profil M. Beobachtet wird: Startet WindowServer, komponiert Core Animation, rendern eine SwiftUI- und eine AppKit-App? Zensus nach E-4 | Desktop läuft ohne abgewiesene Apple-Features → M wird Nutzer-Default bis A7⁻, WP-GPU-18 verlässt den M5-Pfad, Option 2b ist tragfähig. Desktop scheitert oder der Zensus zeigt Apple-Features → bestätigt A-1/A-2, WP-GPU-18 ist M5-Pflicht. Der Zensus legt die Reihenfolge in WP-GPU-18 fest (z. B. Memoryless und Programmable Blending vor Tile-Shadern) |
| **S-GPU-1a** (Teil von S-GPU-1) | Black-Box auf einem Mac (O-2, ADR-0008 Nr. 3): In einem macOS-27-Gast unter Virtualization.framework alle `supportsFamily:`-Werte, K2-Eigenschaften und Grenzwerte über die öffentliche API protokollieren, mit Standard-Host-Einstellung und mit uneingeschränkter Stufe. Dasselbe auf einem M1-Host als Referenz für A-6 | Gast meldet mindestens Apple7 → V = A7, Option 2a entfällt. Gast meldet weniger → V ist definiert und wird Zwischenprofil. Stufe hostgesteuert → Argument für P (Profilwahl über das Protokoll) |
| **S-GPU-18a** (vor WP-GPU-18) | Prototyp auf lavapipe und Iris: Programmable Blending über Interlock, Tile-Dispatch über Pass-Split, Kosten pro Frame bei 1920×1080 | Kosten auf der Iris sprengen Q-PERF-4 → E-6 kippt für die Iris, Option 3 wird dauerhaft (Iris bleibt bei V oder M) |
| **WP-GPU-03** | `vulkaninfo` auf G5 MF (proprietär, NVK, ANV) | Proprietärer Treiber ohne Interlock → keine Apple-Familie auf der RTX (E-5), Entscheidung neu. ANV ohne natives ASTC oder ETC2 → nur Kostenänderung. Gerätegrenzen unter Apple7 → kein A7 auf der GPU ohne Emulation der Grenze |
| Wechseltest (nach M5) | Gleiches Gast-Image abwechselnd auf RTX und Iris mit verschiedenen Profilen booten | Fehler in persistiertem Zustand → E-6 verschärfen: gleiches Profil schon in der Übergangszeit |
