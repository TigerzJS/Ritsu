# R-ARCH-07: Belege zu ADR-0011 (Metal-Familienpolitik) und Spike-Briefs S-GPU-1a, S-GPU-2a, S-GPU-18a

> Status: **Belegsammlung** (2026-10-14). Autor-Rolle: `architect` (WP-ARCH-07). Anlass:
> [ADR-0011](../adr/0011-metal-family-policy.md) stützt sich auf Fakten F-1 bis F-8, deren Quellen bisher nur im
> Arbeitsverzeichnis der Recherche lagen. Dieser Bericht hält jede Quelle dauerhaft fest (URL, Ausgabe oder Stand,
> Abschnitt, Seite oder Zeile), gibt die Aussage in Kurzform wieder und bewertet die Konfidenz. Außerdem enthält er
> die drei Spike-Briefs, die ADR-0011 unter *Überprüfung* verlangt. Er ändert weder Code noch ADR-0011. Was beim
> Nachprüfen an ADR-0011 auffiel, steht in §4.

## 1. Methode und Regeln

* **Clean Room ([ADR-0008](../adr/0008-clean-room.md)).** Apple-Dokumente werden nur sinngemäß und in Kurzform
  wiedergegeben, mit Fundstelle. Aus Mesa-Quelltext steht hier nur Datei, Zeile, Commit und die Aussage, kein Code.
  Gerätenamen und Einstellungsschlüssel des paravirtuellen Geräts werden nicht zitiert, auch wenn Drittquellen sie
  nennen.
* **Abrufdatum:** alle Quellen 2026-10-14, sofern nicht anders angegeben.
* **Konfidenz:**
  * **hoch:** Primärquelle selbst gelesen, Aussage steht dort eindeutig.
  * **mittel:** Primärquelle mit Auslegung (z. B. Spaltenzuordnung in einer aus dem PDF extrahierten Tabelle) oder
    glaubwürdige Sekundärquelle.
  * **niedrig:** Drittbericht ohne Primärbeleg oder Quelle beim Nachprüfen nicht mehr abrufbar.
* **Festgehaltene Stände** (damit Zeilenangaben reproduzierbar bleiben):

| Quelle | Stand | Prüfmerkmal |
|---|---|---|
| Apple, *Metal Feature Set Tables* (PDF, 18 Seiten), [developer.apple.com/metal/Metal-Feature-Set-Tables.pdf](https://developer.apple.com/metal/Metal-Feature-Set-Tables.pdf) | Ausgabe **21. Mai 2026** (Fußzeile jeder Seite) | Die frühere Seite `developer.apple.com/metal/feature-sets/` antwortet mit HTTP 301 auf dieses PDF |
| Apple Developer Documentation, Symbolseiten zu `MTLGPUFamily` und `MTLDevice` | Stand 2026-10-14 | Ausgewertet wurde das JSON der Symbolseiten (Felder `metadata.platforms[].introducedAt`, `deprecatedAt`, `renamed`) |
| Mesa, [gitlab.freedesktop.org/mesa/mesa](https://gitlab.freedesktop.org/mesa/mesa), Zweig `main` | Commit **`81dff7c64fb0c1f2031d04ccff9972233e2e0524`** (2026-10-14 11:23 UTC) | Git-Blob-IDs: `anv_physical_device.c` `6e79d399`, `isl_format.c` `a9286f7c`, `intel_device_info.c` `15e835f4`, `nvk_physical_device.c` `c66a394e` |
| Khronos, [Vulkan-Docs](https://github.com/KhronosGroup/Vulkan-Docs), Zweig `main` | `xml/vk.xml` mit `VK_HEADER_VERSION` 365 | `chapters/renderpass.adoc` Blob `61335c18` |

Mesa-Links haben die Form
`https://gitlab.freedesktop.org/mesa/mesa/-/blob/81dff7c64fb0c1f2031d04ccff9972233e2e0524/<pfad>#L<zeile>`.
Die Pfade sind `src/intel/vulkan/anv_physical_device.c`, `src/intel/isl/isl_format.c`,
`src/intel/dev/intel_device_info.c` und `src/nouveau/vulkan/nvk_physical_device.c`.

## 2. Belege je Fakt

### 2.1 F-1: Feature-Set-Tabellen, Ausgabe 2026-05-21

| Nr. | Aussage (Kurzform) | Fundstelle | Konfidenz |
|---|---|---|---|
| F-1.1 | Die GPU-Liste nennt nur A8 bis A19 und M1 bis M5, zugeordnet zu Apple2 bis Apple10. Es gibt keine Mac2-Spalte und keine Intel- oder AMD-Zeile. A14, M1 und alles danach tragen „Metal 3 & 4“. | PDF S. 2, Tabelle „Metal GPUs“ | hoch |
| F-1.2 | Alle Metal-3- und Metal-4-Geräte erreichen mindestens die Grenzwerte der Apple7-Spalte. | PDF S. 10, Fußnote 1 zu „GPU implementation limits by family“ (Tabellen S. 7 bis 10) | hoch |
| F-1.3 | Dasselbe für die Texturfähigkeiten: mindestens die Apple7-Spalte. | PDF S. 15, Fußnote 1 zu den Texturfähigkeiten (Tabellen S. 11 bis 15) | hoch |
| F-1.4 | Das Metal-3-Programmiermodell gibt es auf allen Apple-Familien, das Metal-4-Programmiermodell ab Apple7. | PDF S. 6, Fußnote 1 zur Feature-Tabelle (S. 3 bis 6) | hoch |
| F-1.5 | Die Höchstzahl der Slots in Argument-Tabellen (Buffer 31, Texturen 128, Sampler 16 ab Apple6) entspricht den Bindungsgrenzen einer `MTL4ArgumentTable` desselben Typs. | PDF S. 7 (Zeilen „Maximum number of entries in the … argument table“), S. 10 Fußnote 2 | hoch |

### 2.2 F-2: Ab welcher Familie ein Feature verfügbar ist

Alle Zeilen stammen aus der Feature-Tabelle des PDF (Spalten „Metal“ und „Apple“). Die Texturformat-Fähigkeiten stehen
in den Texturtabellen. Die Konfidenz ist **hoch**, wenn die Zeile im extrahierten Text eindeutig „Apple*N*“ trägt,
und **mittel**, wenn die Familie aus der Spaltenzählung einer Texturtabelle folgt.

| ab | Feature (Kurzname der Zeile) | Seite | Konfidenz |
|---|---|---|---|
| Apple2 | Programmable Blending; getrennte Depth- und Stencil-Attachments; PVRTC; EAC/ETC; ASTC; Memoryless Render Targets; Dual-Source-Blending; programmierbare Sample-Positionen; Deferred Store Action | S. 3 | hoch |
| Apple3 | Extended-Range-Formate; MSAA-Depth-Resolve; komprimierte Volumentexturen | S. 3 | hoch |
| Apple3 | `BGRA10_XR`/`BGRA10_XR_sRGB`: Apple2 „nicht verfügbar“, ab Apple3 „All“ | S. 15 | mittel |
| Apple3 | `RGB9E5Float`: acht Spalten „All“ (Filter, Color, MSAA, Resolve, Blend), also Apple3 bis Apple10 | S. 13 | mittel |
| Apple4 | Imageblocks; Tile-Shader; Imageblock Sample Coverage Control; Post-Depth Coverage; Raster Order Groups; Quad-Permute | S. 4 | hoch |
| Apple6 | Argument Buffers Tier 2 | S. 4 | hoch |
| Apple6 | Sparse-Texturen; Variable Rasterization Rate; Vertex Amplification; Render- und Compute-Dynamic-Libraries; SIMD-Permute; Quad-Ballot | S. 4 | hoch |
| Apple6 | Ray Tracing (Compute, Render); Funktionszeiger (Compute, Render); Texture-Atomics; Residency Sets | S. 5 | hoch |
| Apple6 | HDR-ASTC: Apple2 bis Apple5 „nicht verfügbar“ | S. 14 | mittel |
| Apple7 | Mesh-Shading; Floating-Point-Atomics | S. 5 | hoch |
| Apple7 | SIMD-Reduktion; Quad-Reduktion; SIMD-Matrix-Multiplikation; Primitive ID; Baryzentrische Koordinaten | S. 4 | hoch |
| Apple7 | Border Color; Layered Rendering in Multisample-Texturen | S. 3 | hoch |
| Apple7 | Metal 4: Argument Tables; Command Allocators; entkoppelte Queues und Command Buffer | S. 5 | hoch |
| Apple7 | Metal 4: Machine-Learning-Encoding; Tensoren | S. 6 | hoch |
| Apple9 | BC-Formate (Zeile mit Fußnote 2) | S. 3 | hoch |
| – | BC unter macOS: Laut Fußnote 9 der Texturtabellen haben alle macOS-Geräte der Familien Apple7 und Apple8 BC. Fußnote 2 der Feature-Tabelle (S. 6) spricht dagegen nur von *manchen* Apple7/8-Geräten unter iPadOS. | S. 15, Fußnote 9 | hoch |

**Grenzwerte für Argument Buffers** (für ADR-0012, aus derselben Ausgabe): Pro Stufe aus Argument Buffers erreichbar
sind ab Apple6 unbegrenzt viele Buffer und 1M Texturen. Bei den Samplern sind es 128 (Apple6), 996 (Apple7, Apple8)
und 500 K (ab Apple9) (S. 8, Fußnote 8 auf S. 10: Die Grenzen gelten auch für indirekt erreichbare Argument
Buffers). `setBytes` ist auf 4 KB begrenzt (S. 7, Fußnote 4 auf S. 10). **Konfidenz:** hoch.

### 2.3 F-3: `MTLGPUFamily`- und `MTLDevice`-Symbolseiten

Quelle: Apple Developer Documentation, z. B.
[MTLGPUFamily.mac2](https://developer.apple.com/documentation/metal/mtlgpufamily/mac2). Ausgewertet wurden die
Plattformfelder im JSON der Symbolseite.

| Symbol | macOS: eingeführt / deprecated / Ersatz laut Feld `renamed` | Konfidenz |
|---|---|---|
| `MTLGPUFamily.mac2` | 10.15 / **27.0** / `MTLGPUFamily.apple7` | hoch |
| `MTLGPUFamily.common1` | 10.15 / **27.0** / `MTLGPUFamily.apple1` | hoch |
| `MTLGPUFamily.common2` | 10.15 / **27.0** / `MTLGPUFamily.apple3` | hoch |
| `MTLGPUFamily.common3` | 10.15 / **27.0** / `MTLGPUFamily.apple5` | hoch |
| `MTLGPUFamily.metal3` | 13.0 / – / – | hoch |
| `MTLGPUFamily.metal4` | 26.0 / – / – | hoch |
| `MTLGPUFamily.apple7` | 10.15 / – / – (Kurzbeschreibung: A14 und M1) | hoch |
| `MTLDevice.isLowPower`, `isHeadless`, `isRemovable`, `location`, `maxTransferRate`, `peerGroupID` | jeweils deprecated ab **27.0**, Begründung sinngemäß: auf Apple Silicon nicht anwendbar | hoch |
| `MTLDevice.argumentBuffersSupport`, `maxArgumentBufferSamplerCount` | nicht deprecated (seit 10.13 bzw. 10.14) | hoch |

Die Diskussion der `metal3`-Seite listet unter macOS noch AMD- und Intel-GPUs (u. a. Vega, Iris Pro, UHD 630) neben
„M1 and later“. Das ist ein veralteter Stand und kein Widerspruch zu F-1 (Konfidenz: hoch für den Inhalt der Seite,
mittel für die Wertung „veraltet“).

### 2.4 F-4 und F-5: Metal 4 nur auf Apple Silicon, Intel-Ende mit macOS 26

| Nr. | Aussage (Kurzform) | Quelle | Konfidenz |
|---|---|---|---|
| F-4.1 | Metal 4 gibt es nur auf Macs mit Apple Silicon unter macOS 26 oder neuer. Die Metal-3-Liste nennt dagegen noch Intel-Macs ab 2017/2018. | Apple Support [102894](https://support.apple.com/en-us/102894) „Support for Metal on Apple devices“, veröffentlicht 2026-03-10 | hoch |
| F-4.2 | Metal 4 setzt M1 bzw. A14 oder neuer voraus. | WWDC25 Session 205 „Discover Metal 4“ (beim Nachprüfen nicht erneut abgerufen) und F-1.4 | mittel (über F-1.4 abgesichert) |
| F-5.1 | macOS 26 Tahoe ist die letzte Version für Intel-Macs. Angekündigt wurde das in der WWDC25 Platforms State of the Union. | [PetaPixel, 2025-06-10](https://petapixel.com/2025/06/10/macos-26-tahoe-is-the-end-of-the-road-for-intel-macs/) (beim Nachprüfen HTTP 403), [Thurrott](https://www.thurrott.com/?p=321965), [Guru3D](https://www.guru3d.com/story/apple-ends-major-macos-support-for-intel-macs-after-tahoe-26/) | mittel (Sekundärquellen, übereinstimmend) |

### 2.5 F-6: Paravirtuelle GPU in macOS-VMs

Es gibt keine Apple-Dokumentation zur gemeldeten Familie. Alle Belege sind Drittberichte.

| Nr. | Aussage (Kurzform) | Quelle | Konfidenz |
|---|---|---|---|
| F-6.1 | Das paravirtuelle Gerät läuft auf echter Apple-GPU, meldet über `supportsFamily:` aber weder Apple7 noch Metal 3. Residency Sets und `MTLLogState` scheitern. Host der Tests: M3 Pro, Gast macOS 15. | [Metal.jl PR #789](https://github.com/JuliaGPU/Metal.jl/pull/789) „Limited support for virtual devices on macOS 15+“, gemergt 2026-06-04 | mittel |
| F-6.2 | Ab Werk meldet der Gast etwa eine Apple5-Stufe, 32 KB Threadgroup-Speicher, keine SIMD-Matrix, keine SIMD-Reduktion, kein bfloat. Gast macOS 26.5.2, Host M1 Ultra mit macOS 26.6.1. Eine hostseitige Einstellung hebt die Stufe an. | [Cua-Blog](https://github.com/trycua/cua/blob/main/blog/gpu-passthrough-macos-vms.md), 2026-08-11 | mittel |
| F-6.3 | Zu Metal 3 sagt F-6.2 nur indirekt etwas: Wird Metal 3 gemeldet, verlangt MLX Residency Sets, und die fehlen dem Gerät. Ein ausdrückliches „Metal 3 = NO ab Werk“ steht dort nicht. | wie F-6.2 | niedrig (Auslegung) |
| F-6.4 | In macOS 27 können macOS-Gäste Metal-Features wie Argument Buffers und Indirect Command Buffers nutzen. Das ist nur eine kurze Erwähnung im Überblick. | [WWDC26 Session 224](https://developer.apple.com/videos/play/wwdc2026/224/) „Expand the capabilities of your Virtualization app“ | hoch (für die Erwähnung), niedrig (für den Umfang) |
| F-6.5 | Apples VM-Beispiel zeigt einen vollständigen grafischen Desktop. | [Running macOS in a virtual machine on Apple silicon](https://developer.apple.com/documentation/virtualization/running-macos-in-a-virtual-machine-on-apple-silicon) | hoch |

### 2.6 F-7: Vulkan-Treiber der Ziel-GPUs (Mesa, Commit `81dff7c6`)

Die Iris Xe des G5 MF ist gfx12.0 (`verx10` = 120, Alder Lake). Die RTX 4050 ist Ada. NVK unterscheidet Generationen
über `cls_eng3d`.

| Feature | ANV / Iris Xe | NVK / RTX 4050 | Konfidenz |
|---|---|---|---|
| `textureCompressionETC2` | fest an: `anv_physical_device.c` Z. 703 | fest aus: `nvk_physical_device.c` Z. 375 | hoch |
| ASTC LDR | aktiv, wenn das Sampling von `ASTC_LDR_2D_4X4_FLT16` geht oder die Emulation per drirc erzwungen ist (`anv_physical_device.c` Z. 704 bis 705, 3137 bis 3141). Laut `isl_format.c` Z. 337 bis 364 gibt es ASTC-LDR-Sampling ab gfx 9.0. Ab gfx12.5 schließt `isl_format.c` Z. 756 bis 766 ASTC und FXT1 aus, der Kommentar dort nennt die Entfernung aus der Hardware. Für gfx12.0 gilt also: **nativ** | nur auf SoCs ab Kepler C (Tegra): `nvk_physical_device.c` Z. 105 bis 109, 377 | hoch |
| ASTC HDR | Vulkan-Feature fest aus: `anv_physical_device.c` Z. 821. Die Formattabelle trägt Sampling für `ASTC_HDR_*` ab gfx 11.0 (`isl_format.c` Z. 365 ff.), siehe §4 P-3 | nicht vorhanden | hoch (Treiber), mittel (Hardware) |
| `VK_EXT_fragment_shader_interlock` | an: Z. 563, Pixel- und Sample-Interlock Z. 878 bis 879 | nicht in der Erweiterungstabelle | hoch |
| `VK_KHR_dynamic_rendering_local_read` | an: Z. 395, Feature Z. 1218 | an: Z. 143, Feature Z. 502 | hoch |
| `shaderBufferFloat32AtomicAdd` | nur mit LSC: Z. 983. `has_lsc` steht in `XEHP_FEATURES` (`intel_device_info.c` Z. 1041 bis 1048, gfx12.5), nicht in `GFX12_FEATURES` (Z. 898 bis 901) | fest an: Z. 778 | hoch |
| Mesh-Shader | `EXT_mesh_shader` = `has_mesh_shading` (Z. 591). Das Flag steht erst in `XEHP_FEATURES` | ab Turing: Z. 275 | hoch |
| Ray Tracing | `rt_enabled` braucht `has_ray_tracing` (Z. 361), erst ab XeHP | **keine** RT-Erweiterung in der Tabelle (weder `acceleration_structure` noch `ray_query` noch `ray_tracing_pipeline`) | hoch |
| `fragmentShaderBarycentric` | Erweiterung erst ab `ver` ≥ 20 (Z. 404), Feature Z. 1240 bis 1241 | ab Turing: Z. 151, 525 | hoch |
| `requiredSubgroupSizeStages` | nur Compute, Task, Mesh: Z. 1507 bis 1509 | 0, Subgroup fest 32: Z. 1089 bis 1093 | hoch |
| `VK_EXT_sample_locations` | an: Z. 614 | ab Maxwell B: Z. 298 | hoch |
| `VK_EXT_rasterization_order_attachment_access` | nicht in der Tabelle | nicht in der Tabelle | hoch |
| `textureCompressionBC` | an: Z. 706 | an: Z. 376 | hoch |

**Proprietärer NVIDIA-Treiber:** Für ihn gibt es keine Primärquelle im Quelltext. Alle Zellen dieser Spalte in
ADR-0011 F-7 bleiben **Annahme**, bis WP-GPU-03 `vulkaninfo`-Dumps vom G5 MF unter `docs/compat/` ablegt. Ein
öffentlicher Datenbankeintrag für die RTX 4050 Laptop unter Linux war beim Nachprüfen nicht automatisch abrufbar.

### 2.7 F-8: Vulkan, Local Read und Rasterreihenfolge

| Nr. | Aussage (Kurzform) | Fundstelle | Konfidenz |
|---|---|---|---|
| F-8.1 | `VK_RENDERING_ATTACHMENT_INPUT_ATTACHMENT_FEEDBACK_BIT_KHR` erlaubt die Rückkopplung innerhalb **eines** Fragments ohne `BY_REGION`-Barriere. Die Pflicht zur `BY_REGION`-Barriere, wenn ein Fragment schreibt und ein **späteres anderes** Fragment dieselbe Stelle als Input-Attachment liest, bleibt bestehen. | Vulkan-Docs `chapters/renderpass.adoc` Z. 1487 ff. (Anker `rendering-attachment-input-attachment-feedback`) | hoch |
| F-8.2 | Barrieren gibt es nur zwischen Draws. Innerhalb eines Draws ordnet erst `VK_EXT_rasterization_order_attachment_access` Zugriffe auf Attachments von Fragment zu Fragment in Rasterreihenfolge, ohne explizite Synchronisation. | Vulkan-Docs `appendices/VK_EXT_rasterization_order_attachment_access.adoc`, Abschnitt *Description* | hoch |
| F-8.3 | Fragment Shader Interlock ist die zweite Möglichkeit, überlappende Fragmente zu ordnen. Das ist eine Schlussfolgerung aus der Semantik von `VK_EXT_fragment_shader_interlock` und wurde hier nicht mit Zeile belegt. | Vulkan-Registry, `VK_EXT_fragment_shader_interlock` | mittel |

## 3. Belege zu den Annahmen A-1 bis A-6

Für die Annahmen gibt es bewusst keine Primärquelle. Sie bleiben Annahmen. Dieser Abschnitt hält nur fest, welche
Messung sie bestätigt oder widerlegt.

| Annahme | Stütze | Bestätigt oder widerlegt durch |
|---|---|---|
| A-1 (Apple testet macOS 27 nur mit Apple7+ und dem Paravirt-Gerät) | F-1.1, F-3 (Mac2 deprecated, Ersatz Apple7), F-5 | S-GPU-2a |
| A-2 (Systemcode nutzt Apple-Features ohne Abfrage) | F-6.2 (das Gerät führt neuere Kernel trotz niedriger Meldung aus) | S-GPU-2a (Zensus), S-GPU-1a (Probe „trotzdem nutzbar“) |
| A-3 (Paravirt-Profil in macOS 27 eher höher) | F-6.4 | S-GPU-1a |
| A-4 (Profilwechsel zwischen Boots ist verträglich) | – | Wechseltest nach M5 (ADR-0011) |
| A-5 (Tile-Shader, Mesh, Float-Atomics usw. nur über die Familie sichtbar) | K2-Liste in ADR-0011 aus den Symbolseiten | S-GPU-1a (vollständige Liste der Abfragen) |
| A-6 (echte Apple-Silicon-Macs melden zusätzlich Mac2 und Common1–3) | frühere Feature-Set-Seite, WWDC20 | S-GPU-1a, Lauf 1 |

## 4. Befunde aus dem Nachprüfen (Vorschläge an ADR-0011, nicht eingearbeitet)

ADR-0011 ist **Proposed**. Die Punkte sind Präzisierungen für den Menschen bzw. für eine Überarbeitung durch den
`architect` vor der Freigabe:

* **P-1 BC-Fußnote:** Die Aussage „alle Apple7/8-Geräte unter macOS haben BC“ stammt aus Fußnote 9 der
  Texturtabellen (S. 15). Die Fußnote der Feature-Tabelle (S. 6) nennt nur iPadOS. Die Fundstelle in F-2 sollte S. 15
  angeben.
* **P-2 NVK und Ray Tracing:** Am Stand `81dff7c6` hat NVK keine RT-Erweiterung. In F-7 kann aus „**Annahme** nein“
  ein belegtes „nein“ werden.
* **P-3 ASTC HDR auf der Iris:** Der Treiber meldet es nicht. Die Formattabelle von ISL führt HDR-Sampling aber ab
  gfx 11. „Nein“ in F-7 stimmt für Vulkan. Ein nativer Pfad bräuchte eine Treiberänderung und ist für Ritsu keine
  Option.
* **P-4 Cua-Bericht:** Die Aussage „kein Metal 3 ab Werk“ ist dort nur indirekt belegt (F-6.3). In F-6 sollte sie als
  Auslegung markiert sein.
* **P-5 Neue Lücke, Grenzwert für Argument Buffers auf der Iris:** Laut Kommentar in `anv_physical_device.c`
  Z. 304 bis 313 ist der bindless Surface-Handle bis gfx12.0 20 Bit breit, also höchstens etwa 1M Surface States. Die
  meisten Image Views brauchen zwei davon, das ergibt etwa 500K gleichzeitig existierende Image Views. Apple7 verlangt
  1M Texturen pro Stufe aus Argument Buffers (S. 8). Für ADR-0011 E-1 („Grenzwerte genau Apple7“) ist das ein
  möglicher Konflikt auf der Iris. Ob er praktisch zählt, misst WP-GPU-03 (Kapazitätstest, siehe ADR-0012 M-3).
* **P-6 Einzigartige Sampler:** `maxArgumentBufferSamplerCount` ist auf Apple7 unter macOS nicht dokumentiert
  (Symbolseite ohne Zahl). Vulkan garantiert 4000 Sampler-Objekte (`maxSamplerAllocationCount`). NVK kodiert den
  Sampler-Index mit 12 Bit (`nvk_descriptor_types.h`, Commit `81dff7c6`, Z. 42 bis 45), das ergibt höchstens 4096.
  S-GPU-1a soll den Apple-Wert messen.

## 5. Spike-Briefs

Die Briefs folgen dem Skill `/spike`. Der Bericht entsteht jeweils als `docs/research/S-<ID>-<kurz>.md`. Jeder Brief
nennt Frage, Methode und Abbruchkriterium. Das Abbruchkriterium sagt, wann der Spike endet, auch ohne volle Antwort,
und was dann gilt.

### 5.1 S-GPU-1a: Welches Profil meldet ein macOS-27-Gast unter Virtualization.framework?

* **Teil von:** S-GPU-1 (ADR-0005). **Rolle:** `reverse-engineer`, Black-Box nach ADR-0008 Nr. 3. **Braucht:** einen
  Mac mit Apple Silicon und macOS 27 (O-2). **Timebox:** 3 Arbeitstage.
* **Frage:**
  1. Welche `supportsFamily:`-Werte, K2-Eigenschaften und Grenzwerte meldet ein macOS-27-Gast mit der
     Standard-Host-Einstellung, und welche mit der uneingeschränkten Stufe?
  2. Ist die Stufe hostgesteuert, ohne Änderung im Gast?
  3. Antwortet echte Apple7-Hardware unter macOS 27 auf Mac2 und Common1–3 mit YES (A-6)?
  4. Lassen sich im Gast Apple-Features nutzen, die über der gemeldeten Familie liegen (A-2)?
  5. Für ADR-0012: Welchen Wert hat `maxArgumentBufferSamplerCount`, und wie verhält sich `gpuResourceID` über
     Erzeugen und Freigeben hinweg (nur beobachtbare Eigenschaften wie Eindeutigkeit und Wiederverwendung, keine
     Kodierungsdetails)?
* **Methode:**
  * Ein eigenes Testprogramm (öffentliche Metal-API, MIT-Lizenz, liegt im Repo unter `tests/`) protokolliert als
    JSON:
    * alle `MTLGPUFamily`-Rohwerte aus dem SDK,
    * alle K2-Abfragen aus ADR-0011,
    * die Grenzwerte (`maxThreadsPerThreadgroup`, `maxThreadgroupMemoryLength`, `maxBufferLength`,
      `maxArgumentBufferSamplerCount`, `argumentBuffersSupport`, `readWriteTextureSupport`,
      `supportsTextureSampleCount(1…8)`, `recommendedMaxWorkingSetSize`).
  * Lauf 1 auf dem Host nativ (Referenz), Lauf 2 im Gast mit Standard-Einstellung, Lauf 3 im Gast mit
    uneingeschränkter Stufe. Den Namen der Einstellung schreibt der Spike-Bericht nicht ins Repo.
  * Probe zu Frage 4: Im Gast werden erzeugt und benutzt: eine memoryless Textur, eine ASTC-Textur, eine
    Tile-Pipeline, ein Fragment-Shader mit `[[color(0)]]`-Eingang. Protokolliert wird Erfolg, `nil` oder
    Command-Buffer-Fehler.
  * Die Shader des Testprogramms sind selbst geschrieben und mit Apples Compiler übersetzt (ADR-0008 Nr. 4).
* **Ergebnis-Artefakt:** `docs/research/S-GPU-1a-paravirt-profile.md` mit Tabelle je Lauf. Rohdaten nur als selbst
  erzeugtes JSON des eigenen Programms, keine Apple-Dateien.
* **Abbruchkriterium:**
  * Ohne Mac-Zugang (O-2 offen) wird der Spike nicht begonnen. Profil V bleibt dann undefiniert, und E-7 von ADR-0011
    überspringt V.
  * Startet macOS 27 nach 1 Arbeitstag nicht als Gast: Abbruch. Die Läufe 1 und 3 werden, soweit möglich, mit
    macOS 26 als Näherung gemacht und so markiert.
  * Liefert Lauf 2 in allen Familien mindestens Apple7: Lauf 3 und die Probe entfallen, denn V = A7 (ADR-0011,
    Option 2a entfällt).

### 5.2 S-GPU-2a: Läuft der macOS-27-Desktop mit Profil M?

* **Teil von:** S-GPU-2 (ADR-0005), beantwortet G-1. **Rolle:** `gpu-engineer` (Prototyp im Gast, macOS-SDK nötig).
  **Voraussetzung:** S-GPU-2 hat gezeigt, dass sich ein eigenes Metal-Device-Bundle im Gast laden lässt.
  **Timebox:** 5 Arbeitstage.
* **Frage:** Startet WindowServer mit Profil M (Mac2, Common1–3, Metal3, keine Apple-Familie)? Komponiert Core
  Animation? Rendern eine SwiftUI- und eine AppKit-Test-App? Welche Apple-Features werden trotz Profil M benutzt
  (Feature-Zensus nach ADR-0011 E-4)?
* **Methode:**
  * Der Prototyp-Treiber meldet Profil M. Jede Nutzung eines Features außerhalb des Profils lehnt er ab und zählt sie
    mit Feature und, soweit bekannt, Prozess. Er nähert nichts an.
  * Testfolge:
    1. Boot bis Login-Fenster,
    2. Anmeldung und Desktop,
    3. Finder-Fenster verschieben,
    4. SwiftUI-Test-App mit Liste, Blur, Animation,
    5. AppKit-Test-App mit `NSVisualEffectView`,
    6. Core-Image-Filter in der Vorschau-App.
  * Zum Vergleich läuft dieselbe Folge mit einer Ablehnungsliste, die leer ist: Der Prototyp meldet dann M, lässt aber
    alles durch, was der Host nativ kann. Das zeigt, ob Fehlschläge an der Meldung oder an der Nutzung liegen.
* **Ergebnis-Artefakt:** `docs/research/S-GPU-2a-profile-m.md` mit Ergebnis je Schritt und dem Zensus als Tabelle
  (Feature, Anzahl, Prozess).
* **Abbruchkriterium:**
  * Bleibt WindowServer mit Profil M ohne jede abgewiesene Apple-Funktion hängen oder stürzt ab (Absturzbericht im
    Gast): Abbruch nach Schritt 1. Ergebnis: A-1 ist bestätigt, M ist kein Nutzer-Default, und WP-GPU-18 ist
    M5-Pflicht.
  * Zeigt der Zensus nach Schritt 2 mehr als null abgewiesene Apple-Features in WindowServer oder Core Animation:
    Abbruch. Dasselbe Ergebnis, und die Zensusliste legt die Reihenfolge in WP-GPU-18 fest.
  * Laufen alle sechs Schritte ohne abgewiesene Apple-Features: Erfolg. M darf Nutzer-Default bis A7⁻ werden
    (ADR-0011 E-7).
  * Ohne Ergebnis von S-GPU-2 (Bundle nicht ladbar): nicht beginnen.

### 5.3 S-GPU-18a: Was kosten Tile-Shader und ordnungstreues Programmable Blending auf RTX und Iris?

* **Vor:** WP-GPU-18. **Rolle:** `gpu-engineer`. **Braucht:** lavapipe (CI) für die Korrektheit, das G5 MF für die
  Kosten (RTX proprietär, Iris ANV). **Timebox:** 5 Arbeitstage.
* **Frage:**
  1. Liefert Programmable Blending über Fragment Shader Interlock plus Storage-Image dasselbe Ergebnis wie eine
     Referenz mit strikter Rasterreihenfolge, auch bei überlappenden Fragmenten in einem Draw?
  2. Was kostet es pro Frame bei 1920×1080?
  3. Was kostet ein Tile-Dispatch über Pass-Split (Rendering beenden, Compute über Kacheln, mit `LOAD` fortsetzen),
     und was ein expliziter Imageblock als Buffer pro Pixel?
* **Methode:**
  * Drei Vulkan-Testszenen ohne Gast:
    * (a) 10 000 halbtransparente, überlappende Partikel in einem Draw mit Blending im Shader (liest die Farbe am
      eigenen Pixel),
    * (b) Deferred Shading mit G-Buffer im Imageblock (4 Attachments, 16 B/px) und Licht-Pass als Tile-Dispatch,
    * (c) dieselbe Szene als Basis mit festem Blending bzw. klassischen Render Targets.
  * Messung mit Zeitstempel-Queries, Median aus 500 Frames, Zielauflösung 1920×1080 ohne MSAA und mit 4× MSAA.
  * Korrektheit: bit-genauer Vergleich (a) gegen eine CPU-Referenz mit strikter Reihenfolge auf lavapipe und auf
    beiden GPUs.
* **Ergebnis-Artefakt:** `docs/research/S-GPU-18a-tbdr-cost.md` mit Tabelle (Szene × GPU × MSAA → ms/Frame,
  Mehrkosten gegenüber (c)).
* **Abbruchkriterium:**
  * Liefert (a) auf einer GPU ein anderes Bild als die Referenz: Abbruch für diese GPU. Interlock gilt dort als
    nicht ordnungstreu, und ADR-0011 E-5 muss neu bewertet werden (keine Apple-Familie auf dieser GPU).
  * Kostet (a) oder (b) auf der Iris mehr als **8 ms pro Frame** zusätzlich zu (c) ohne MSAA: Abbruch der Messungen
    auf der Iris. Damit ist das Budget von Q-PERF-4 (Ziel M8 < 8 ms) allein durch die Emulation verbraucht. Folge
    nach ADR-0011: E-6 kippt für die Iris, Option 3 wird dauerhaft (Iris bleibt bei V oder M).
  * Kostet es auf der RTX mehr als 2 ms zusätzlich: weitermessen, aber WP-GPU-18 bekommt ein Leistungsziel und einen
    eigenen Optimierungs-Spike.
  * Die Timebox endet ohne Iris-Zugang: Ergebnis nur für lavapipe und RTX, die Iris-Frage bleibt offen und wird so
    gemeldet.
