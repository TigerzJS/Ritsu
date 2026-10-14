# GPU – Metal → Vulkan

> Status: **Entwurf v0.1** (WP-M0-GPU). Zuständig: `gpu-engineer`. Gilt zusammen mit
> [ADR-0002](../adr/0002-linux-host-vulkan.md) (Linux + Vulkan 1.3), [ADR-0005](../adr/0005-gpu-guest-path.md)
> (Gastpfad P/G, **Proposed**), [ADR-0007](../adr/0007-shader-translation-on-host.md) (AIR → SPIR-V auf dem Host)
> und [ADR-0008](../adr/0008-clean-room.md) (Clean Room). Bei Widerspruch gilt der ADR.
> Crates: `ritsu-gpu-proto`, `ritsu-gpu-host`. Shader-Übersetzung: [shader-compiler.md](shader-compiler.md).

## 0. Kennzeichnung und Quellenlage

| Marke | Bedeutung |
|---|---|
| **[öffentlich]** | Öffentliche Apple-Dokumentation (Metal-API-Referenz, Metal Feature Set Tables, WWDC-Vorträge), Khronos-Spezifikationen, Mesa-/NVIDIA-Treiberdokumentation. |
| **[Annahme]** | Plausibel, aber nicht gemessen bzw. nicht belegt. Code, der darauf baut, trägt die Annahme im Doku-Kommentar (z. B. `Capabilities::rtx4050_assumed()`). |
| **[Spec nötig]** | Muss erst als Clean-Room-Spezifikation (`docs/specs/gpu-paravirt/`, `docs/specs/metal-driver/`) entstehen. |
| **[Entscheidung]** | Festlegung dieses Dokuments (änderbar per Review, nicht per ADR, solange sie nur dieses Subsystem betrifft). |
| **[Offen]** | Offene Frage, gesammelt in §15. |

Treiber-Fähigkeiten (welche Erweiterung ANV/NVK/NVIDIA in welcher Version anbietet) sind **ohne Messung auf dem G5 MF
alle [Annahme]**. WP-GPU-03 ersetzt sie durch `vulkaninfo`-Dumps unter `docs/compat/`.

## 1. Überblick

```
 GAST (macOS 27)                    HOST (ritsu-vmm, Linux)
 ───────────────                    ─────────────────────────────────────────────────────────────────────
 Metal.framework                    ┌ Frontend P ─ Apple-Paravirt-Protokoll ─┐
   │                                │  (nur nach Spec, ADR-0008)              │
 Gast-GPU-Treiber ── virtio/MMIO ──►┤                                         ├─► Metal-Objektmodell (model)
   P: Apples Paravirt-Treiber       │                                         │    validierte Deskriptoren,
   G: Ritsu-Treiber (guest/)        └ Frontend G ─ ritsu-gpu-proto ──────────┘    Encoder-Puffer, Handles
                                                                                   │
                                     Kern: Hazard-Tracking, Lebensdauer, Queues,   │
                                     Speicherplatzierung, Shader-/Pipeline-Cache ◄─┤
                                                                                   ▼
                                                                  trait GpuBackend (backend)
                                                       ┌──────────────────┼──────────────────┐
                                                       ▼                  ▼                  ▼
                                              Vulkan „nvidia“    Vulkan „intel“       NullBackend
                                              RTX 4050 (prop./NVK) Iris Xe (ANV)       (Tests, CI)
                                                       │                  │
                                                       └──── Präsentation: PRIME / dma-buf / KMS (§12) ──► internes Panel
```

**Grundsätze**

1. **Protokollneutraler Kern** [Entscheidung, aus ADR-0005]: Frontends übersetzen ihr Drahtformat in das
   Objektmodell (`ritsu_gpu_host::model`) und in `GpuCommand`s. Kein Frontend-Detail gelangt in Kern oder Backend.
   Das Skelett zeigt das: `frontend_g` ist das einzige Modul, das `ritsu-gpu-proto` importiert.
2. **Validieren, dann ausführen.** Jeder Gast-Wert (Handle, Größe, Offset, Enum, Shader) wird geprüft, bevor er
   Vulkan erreicht. Vulkan-Validation-Layer-Meldungen sind in Tests Fehler.
3. **Niemals still falsch** (CLAUDE.md §3.5). Ein Feature ist entweder nativ, korrekt emuliert oder dem Gast
   gegenüber nicht gemeldet. Unbekannte Befehle → `Error::Unimplemented`/Gast-Fehler, nie „ignorieren“.
4. **Encoder werden gepuffert** [Entscheidung]: Ein Metal-Encoder wird zunächst in eine Zwischenliste aufgezeichnet
   und erst bei `endEncoding` (bzw. spätestens bei `commit`) nach Vulkan übersetzt. Nur so sind Hazard-Analyse (§7),
   nachträglich gesetzte Store-Actions (`MTLStoreActionUnknown`) und Pass-Zusammenlegung möglich.
5. **Ein Backend pro physischem Gerät.** Ein Gast-`MTLDevice` entspricht genau einem Backend. Multi-GPU-Rendering
   (RTX + Iris gleichzeitig) ist kein Ziel; die Iris ist nur Anzeigegerät, wenn die RTX rendert (§12).

### 1.1 Stand der Implementierung (WP-M0-GPU)

| Baustein | Pfad | Inhalt |
|---|---|---|
| Drahtformat v1 | `ritsu-gpu-proto` | `Header`, `Opcode` (`#[non_exhaustive]`, feste `u16`-Werte, Pflicht-Klasse), Record-Format, zero-copy `RecordIter`, typisierte `Command`s, `StreamEncoder`, `decode_stream` (Fuzz-Ziel); seit WP-GPU-24 alle Objekte des Modells: Texture-Views, Sampler, Depth/Stencil-States, Render-Pipelines, `BeginRenderPass2`, `SetStoreAction`, Binds (§9.2.1); variable Teile als zero-copy `RecordList` |
| Aushandlung (WP-GPU-01) | `ritsu-gpu-proto::negotiate` | `Hello`, `Features` (64-Bit-Menge), `negotiate`, `decode_hello_stream`, `decode_session_stream` (Version, Feature-Gating, doppeltes HELLO) |
| Antwortkanal (WP-GPU-01) | `ritsu-gpu-proto` (`reply`) | `ReplyOpcode`, `Reply` (`HelloAck` mit `CapabilityBlock`, `HelloReject`, `SessionReset`, `RecordError`, `CommandBufferStatus`, `EventSignaled`), `ErrorCode`, `ReplyEncoder`, `decode_reply_stream` (Fuzz-Ziel) |
| Handles | `ritsu-gpu-host::handle` | `HandleTable<T>` mit Generationen, Stale-Erkennung, Slot-Stilllegung bei Generationsüberlauf, Kapazitätsgrenze |
| Objektmodell (WP-GPU-02) | `ritsu-gpu-host::model` | Vollständige `MTLPixelFormat`-Tabelle (`model::format`: Block, Kompression, Numerik/sRGB, Aspekte, Metal-Fähigkeiten, Texture-Buffer-Zugriff, View-Klassen), `TextureDesc` (+ Swizzle) / `BufferDesc`, Texture-Views (`model::view`), Sampler (`model::sampler`), Depth/Stencil-State (`model::depth_stencil`), Render-Pass inkl. Resolve und Clear-Werten (`model::render_pass`), Vertex-Deskriptor (`model::vertex`), Render-Pipeline ohne Shader (`model::pipeline`), `ResourceLimits` – Details §4.0 |
| Fähigkeiten | `ritsu-gpu-host::caps` | `Capabilities` (+ angenommene Profile RTX 4050 / Iris Xe, inkl. `FormatCapsTable`, `VertexInputCaps`), `EmulationPolicy`, `reported_feature_set(_with)` → `ReportedFeatures`; Lückenprüfung Metal ↔ GPU: `format_support`, `texture_support`, `vertex_input_support`, `render_pass_support` |
| Encoder-Zustand (WP-GPU-24) | `ritsu-gpu-host::encoder` | `EncoderValidator`: Reihenfolge der Encoder-Befehle (Binds/Draws nur im Render-Pass, keine Verschachtelung), Pipeline ↔ Pass beim Setzen, Topologie-Klasse und gebundene Vertex-Buffer beim Draw, `setStoreAction` nur für `Unknown` mit Neuvalidierung des Passes; derselbe Code im Frontend (Aufzeichnung) und im `NullBackend` (Submit) |
| Backend | `ritsu-gpu-host::backend` | `trait GpuBackend` (Buffer, Texturen, Texture-Views, Sampler, Depth/Stencil-States, Render-Pipelines), `GpuCommand` (`BeginRenderPass` trägt `RenderPassDesc`, `SetStoreAction`, Binds), `Submission`, `NullBackend` (validiert jede Submission mit `EncoderValidator` gegen die Gerätegrenzen, Bounds, Handles; prüft beim Erzeugen Gerätelücken mit `texture_support`/`render_pipeline_support` unter seiner `EmulationPolicy` – Default `ALL`, weil er nichts ausführt; Views halten ihre Eltern-Textur am Leben; Speicherbudget) |
| Frontend G | `ritsu-gpu-host::frontend_g` | `GuestSession`: Handshake, Gast-IDs → Backend-Handles (fünf Namensräume: Buffer, Texturen inkl. Views, Sampler, Depth/Stencil, Pipelines), jede Payload vollständig validiert (Modell + `guest_limits`, Gerätelücken → `Unimplemented`) bevor das Backend sie sieht, Encoder-Validierung zur Aufzeichnungszeit, Fehler pro Record, „poisoned“ Command Buffer (nach `OpcodeClass`), Completion strikt in Commit-Reihenfolge (`poll_completions`, Protokollgrenze `MAX_OUTSTANDING_SUBMISSIONS`), Sitzungs-Reset, verzögertes Zerstören bis `Commit` (zählt pro Art zur Objektgrenze); `capability_block` (`ReportedFeatures` → Drahtform) |

## 2. Begriffe

| Begriff | Bedeutung |
|---|---|
| TBDR / IMR | Tile-Based Deferred Rendering (Apple-GPUs) / Immediate-Mode Rendering (NVIDIA, Intel). Tile-Speicher, memoryless Attachments, Tile-Shader und Imageblocks sind TBDR-Konzepte ohne direkte IMR-Entsprechung. |
| BDA | `bufferDeviceAddress`: 64-Bit-GPU-Zeiger auf Buffer, Gegenstück zu `MTLBuffer.gpuAddress`. |
| Descriptor Buffer / Heap | Deskriptoren liegen in normalem GPU-Speicher und werden per Index adressiert (bindless). Gegenstück zu Argument Buffers Tier 2 / `MTLResourceID`. |
| DGC | Device Generated Commands: GPU erzeugt Draw-/Dispatch-Befehle. Gegenstück zu Indirect Command Buffers. |
| Timeline-Semaphore | Monoton steigender 64-Bit-Zähler, Gegenstück zu `MTLSharedEvent`. |
| Encoder-Puffer | Ritsus Zwischenaufzeichnung eines Metal-Encoders vor der Übersetzung (§1, Grundsatz 4). |
| Submission-Token | Position auf der Timeline einer Host-Queue (`SubmissionToken` im Skelett). |

## 3. Vulkan-Baseline und Erweiterungen

### 3.1 Pflicht (Gerät wird sonst abgelehnt) [Entscheidung]

Vulkan **1.3** mit diesen Features (fehlt eines, meldet `Capabilities::missing_baseline()` es, und Ritsu nutzt das
Gerät nicht, statt es halb zu nutzen):

| Feature (Core) | Wofür |
|---|---|
| `timelineSemaphore` (1.2) | Queues, `MTLEvent`/`MTLSharedEvent`, Completion (§8) |
| `bufferDeviceAddress` (1.2) | `gpuAddress`, Argument Buffers Tier 2, Ray Tracing |
| `descriptorIndexing` inkl. `runtimeDescriptorArray`, `descriptorBindingPartiallyBound`, `…UpdateAfterBind`, `shaderSampledImageArrayNonUniformIndexing` (1.2) | bindless Texturen/Sampler über `MTLResourceID` |
| `dynamicRendering` (1.3) | Render-Encoder ohne `VkRenderPass`-Objekte |
| `synchronization2` (1.3) | Barrieren mit 64-Bit-Stage/Access-Masken, passend zu Metal-Stages |
| `maintenance4` (1.3) | `maxBufferSize`, Speicheranforderungen ohne Objekt |
| `shaderDrawParameters`, `drawIndirectFirstInstance`, `multiDrawIndirect` | `[[base_vertex]]`, `[[base_instance]]`, indirekte Draws |
| `vulkanMemoryModel`, `scalarBlockLayout`, `storageBuffer8BitAccess`, `storageBuffer16BitAccess`, `shaderInt64` | MSL-Speicher- und Layoutsemantik (siehe shader-compiler.md) |
| `imageCubeArray`, `independentBlend`, `sampleRateShading`, `depthClamp`, `fillModeNonSolid`, `samplerAnisotropy`, `shaderOutputLayer`/`shaderOutputViewportIndex` | direkte Metal-Gegenstücke |
| `samplerMirrorClampToEdge` (1.2) | `MTLSamplerAddressModeMirrorClampToEdge` |
| `textureCompressionBC` | Mac2-Familie, BC-Texturen (beide Ziel-GPUs haben es) |

### 3.2 Erweiterungen pro Feature

„RTX“ = proprietärer NVIDIA-Treiber, „NVK“ = Mesa NVK, „Iris“ = Mesa ANV auf Xe-LP. Alle Spalten **[Annahme]** bis
WP-GPU-03 (ja / nein / ?).

| Erweiterung | Metal-Feature | RTX | NVK | Iris | Rückfall ohne Erweiterung |
|---|---|---|---|---|---|
| `VK_EXT_descriptor_buffer` | Argument Buffers Tier 2, `MTLResourceID`, bindless | ja | ? | ja | Descriptor Indexing mit einem großen update-after-bind-Set (Pflicht-Baseline) |
| `VK_EXT_descriptor_heap` (neu, falls verfügbar) | wie oben, näher an Metals Modell | ? | ? | ? | Descriptor Buffer |
| `VK_KHR_dynamic_rendering_local_read` | Framebuffer Fetch / programmierbares Blending (`[[color(n)]]` als Eingabe) | ja | ? | ja | Interlock + Storage-Image (langsam) oder nicht melden |
| `VK_EXT_fragment_shader_interlock` | Raster Order Groups | ja | ? | ja | nicht melden |
| `VK_EXT_extended_dynamic_state` 1/2 (Core 1.3) | Cull, Winding, Depth/Stencil-Zustand, Topologie pro Draw | ja | ja | ja | – (Core) |
| `VK_EXT_extended_dynamic_state3` | `setTriangleFillMode`, `setDepthClipMode`, Logic-Op, Sample-Mask, Alpha-to-Coverage ohne Pipeline-Varianten | ja | ja | ja | Pipeline-Varianten (mehr Kompilate) |
| `VK_EXT_vertex_input_dynamic_state` | nur für Vertex-Pulling-freie Pipelines mit geteiltem Shader | ja | ja | ja | Vertex-Deskriptor bleibt Pipeline-Zustand (Metal backt ihn ohnehin) |
| `VK_EXT_graphics_pipeline_library` | schnelle PSO-Erstellung, Stotter-Vermeidung (§11) | ja | ja | ja | async-Kompilierung + Cache |
| `VK_EXT_mesh_shader` | Object-/Mesh-Shader | ja | ? | nein | Compute-Emulation (§4.11) oder nicht melden |
| `VK_KHR_acceleration_structure` + `VK_KHR_deferred_host_operations` | `MTLAccelerationStructure` | ja | nein | nein | Compute-BVH (Policy) oder nicht melden |
| `VK_KHR_ray_query` | `intersector<>` in Compute/Fragment | ja | nein | nein | Compute-Traversierung (Policy) |
| `VK_KHR_ray_tracing_pipeline` | Intersection-Function-Tables effizient | ja | nein | nein | Switch-Dispatch in Ray-Query-Schleife |
| `VK_KHR_shader_float16_int8` (Core 1.2), `VK_KHR_16bit_storage` | `half`, `char` in MSL | ja | ja | ja | – |
| Subgroup Size Control (Core 1.3), `VK_KHR_shader_subgroup_extended_types` | SIMD-Gruppen mit fester Breite 32 (Apple) | ja (32 fest) | ja | ja (8/16/32) | `requiredSubgroupSize = 32` erzwingen; ohne → nur Shader ohne Annahmen zur Breite |
| `VK_EXT_shader_atomic_float` (+`_float2`) | `atomic<float>` | ja | ? | teilweise (kein Add auf Buffern) | CAS-Schleife im Shader (Policy) |
| `VK_EXT_shader_image_atomic_int64`, `shaderBufferInt64Atomics` | 64-Bit-Atomics (Apple8+/Apple9) | ja | ? | ? | nicht melden |
| `VK_EXT_custom_border_color`, `VK_EXT_border_color_swizzle` | Border-Farben bei Texturen mit Swizzle/Formaten ohne Alpha korrekt | ja | ja | ja | Formate mit Swizzle + Border nicht kombinierbar → Gast-Fehler |
| `VK_EXT_robustness2` (`nullDescriptor`) | ungültige/stale `MTLResourceID` liefern 0 statt GPU-Fehler | ja | ja | ja | Platzhalter-Deskriptor |
| `VK_EXT_device_generated_commands` | Indirect Command Buffers (GPU-kodiert) | ja | ? | nein | Indirect-Draws / CPU-Replay (§4.10) |
| `VK_KHR_fragment_shading_rate` | Rasterization Rate Maps (Näherung) | ja | ? | ? | nicht melden |
| `VK_EXT_sample_locations` | programmierbare Sample-Positionen | ja | ? | ja | nicht melden |
| `VK_KHR_cooperative_matrix` | Metal-4-Tensoren / `simdgroup_matrix` beschleunigt | ja | ? | nein | Shader-Emulation (korrekt, langsam) |
| `VK_EXT_external_memory_host` | Gast-RAM direkt als GPU-Speicher (Unified Memory, §6) | ja | ? | ja | Spiegelung + Dirty-Tracking |
| `VK_EXT_memory_budget`, `VK_EXT_memory_priority`, `VK_EXT_pageable_device_local_memory` | Budget, Residency-Sets als Hinweis | ja | ja | ja | konservatives Budget |
| `VK_EXT_host_image_copy` | `replaceRegion`/`getBytes` ohne Staging | ja | ? | ja | Staging-Buffer |
| `VK_KHR_external_memory_fd`, `VK_EXT_external_memory_dma_buf`, `VK_EXT_image_drm_format_modifier`, `VK_KHR_external_semaphore_fd` | PRIME, KMS-Scanout, Sync-Files (§12) | ja | ja | ja | CPU-Kopie (nur Notfall) |
| `VK_KHR_swapchain`, `VK_KHR_display`, `VK_EXT_acquire_drm_display`, `VK_KHR_present_id`/`present_wait` | Anzeige (§12) | ja | ja | ja | – |
| `VK_KHR_calibrated_timestamps` | `sampleTimestamps:gpuTimestamp:` | ja | ja | ja | grobe Korrelation |
| `VK_KHR_pipeline_binary` | Pipeline-Cache ohne Treiber-Blob-Unsicherheit | ? | ? | ? | `VkPipelineCache` |
| `VK_KHR_unified_image_layouts` | `GENERAL` überall ohne Leistungsverlust | ? | ? | ? | Layout-Policy pro Backend (§7.4) |
| `VK_EXT_texture_compression_astc_hdr` / ASTC-LDR-Feature | ASTC | nein | nein | nein | Transkodierung (§4.3) |
| `VK_EXT_depth_clip_control`, `VK_EXT_depth_bias_control`, `VK_EXT_provoking_vertex`, `VK_EXT_line_rasterization` | Metal-Rasterisierungsdetails | ja | ja | ja | Abweichung dokumentieren, Testszene |
| `VK_KHR_maintenance5`/`6` | `VK_FORMAT_A8_UNORM_KHR`, Bindings ohne Pipeline-Layout-Pein | ja | ja | ja | Swizzle-Tricks |
| `VK_EXT_legacy_vertex_attributes` (+ `VK_EXT_vertex_input_dynamic_state`) | Vertex-Attribute mit 1-Byte-Ausrichtung von Offset/Stride (Apple7-Regel; Core-Vulkan verlangt Komponentenausrichtung) | ja | ? | ja | Vertex-Pulling im übersetzten Shader; bis dahin `Unsupported` (`caps::vertex_input_support`, `VertexInputCaps::unaligned_fetch`) |
| `VK_KHR_vertex_attribute_divisor` (Core 1.4) inkl. `vertexAttributeInstanceRateZeroDivisor` | `MTLVertexStepFunctionPerInstance` mit `stepRate` > 1, `MTLVertexStepFunctionConstant` | ja | ja | ja | Vertex-Pulling; bis dahin `Unsupported` (`VertexInputCaps::instance_divisor/zero_divisor`) |
| `depthStencilResolve` (Core 1.2), `VK_RESOLVE_MODE_MIN/MAX`, `independentResolveNone`, `independentResolve` | `MTLMultisampleDepthResolveFilterMin/Max`; Resolve nur eines Aspekts bzw. mit unterschiedlichen Modi bei kombinierter Depth/Stencil-Textur | ja | ? | ja | Min/Max → `Unsupported` (`Capabilities::depth_resolve_min_max`); ein Aspekt bzw. unterschiedliche Modi ohne `independent_resolve_none`/`independent_resolve` → `Unsupported`; verschiedene Resolve-Ziele für Tiefe und Stencil einer Textur sind nie ausdrückbar (`VUID-VkRenderingInfo-pDepthAttachment-06086`) → `Unsupported` |
| `VK_EXT_depth_range_unrestricted` | `clearDepth` außerhalb [0, 1] auf `Depth32Float` | ? | ? | ? | `Unsupported` (Modell lehnt ab) |
| `dualSrcBlend` (Core-Feature, `maxFragmentDualSrcAttachments` = 1) | `MTLBlendFactorSource1*` | ja | ja | ja | – (Modell erlaubt Dual-Source nur auf Attachment 0 ohne weitere Farb-Attachments) |

## 4. Abbildung Metal → Vulkan

Die Tabellen beschreiben das **Zielbild**. Was im Skelett schon existiert, steht in §1.1.

### 4.0 Objektmodell (WP-GPU-02)

`ritsu_gpu_host::model` beschreibt jedes Metal-Objekt, das M4 („erstes Dreieck“) und M5 (WindowServer-Formate)
brauchen, protokollneutral (ADR-0005) und mit `validate()` vor jeder Backend-Nutzung:

| Modul | Metal | Inhalt |
|---|---|---|
| `format` | `MTLPixelFormat` | alle Rohwerte, Block, Kompression, Numerik/sRGB, Aspekte, Metal-Fähigkeiten (Apple7), Texture-Buffer-Zugriff, View-Kompatibilität (§4.3) |
| `mod` | `MTLTextureDescriptor`, `MTLBuffer` | `TextureDesc` (+ `swizzle`), `BufferDesc`, `ResourceLimits` (Apple7: 16384 px 2D, 2048 Layer, 8 Farb-Attachments, 4 Samples, 31 Vertex-Attribute/-Buffer) |
| `view` | `MTLTextureSwizzleChannels`, Texture-Views | Swizzle, `TextureViewDesc` (§4.3) |
| `sampler` | `MTLSamplerDescriptor` | §4.4 |
| `depth_stencil` | `MTLDepthStencilDescriptor`, `MTLStencilDescriptor`, `MTLCompareFunction`, `MTLStencilOperation` | Defaults laut SDK, Masken u32 (8 Bit wirksam) |
| `render_pass` | `MTLRenderPassDescriptor` | Farb-/Tiefen-/Stencil-Attachments, Level/Slice/Ebene, Resolve, Clear-Werte, Layered Rendering; generisch über die Textur-Referenz, Validierung per Lookup-Funktion; Ergebnis `RenderTargetLayout` (§4.8) |
| `vertex` | `MTLVertexDescriptor`, `MTLVertexFormat`, `MTLVertexStepFunction` | §4.8 |
| `pipeline` | `MTLRenderPipelineDescriptor` ohne Shader | §4.8; `PipelineTargets` = was ein gebundener PSO mit Pass und Draw teilen muss (Formate, Samples, Topologie, genutzte Vertex-Buffer) – klein genug pro Objekt |
| `bind` | Argument-Tabellen des Render-Encoders | Apple7: 31 Buffer, 128 Texturen, 16 Sampler pro Stufe [öffentlich, Feature Set Tables 2026-05-21]; nicht im HELLO_ACK, also Gast-Grenze; Vulkan-Seite über bindless Heaps (§4.5), keine Gerätelücke darunter [Annahme bis WP-GPU-03/13] |

**Quellen der Rohwerte** [öffentlich]: öffentliche SDK-Header (`MTLPixelFormat.h`, `MTLSampler.h`,
`MTLDepthStencil.h`, `MTLRenderPass.h`, `MTLRenderPipeline.h`, `MTLVertexDescriptor.h`, `MTLTexture.h`) über Apples
Entwicklerdokumentation, gegengeprüft mit den daraus generierten `objc2-metal`-Bindings; Regeln und Grenzwerte aus
Apples Eigenschaftsdokumentation und den *Metal Feature Set Tables* (2026-05-21). Jede Tabelle nennt ihre Quelle im
Modulkommentar; keine Header-Texte im Repo. Rohwerte tragen **keine** [Annahme] mehr (Review: alle Enums gegen
`objc2-metal` 0.3.2 bestätigt; Tests `table_matches_sdk_values_exactly` für Pixel- und Vertex-Formate mit
unabhängiger Rohwertliste). [Annahme] steht nur noch an Regeln, die Apple nicht dokumentiert (z. B. Zuordnung der
komprimierten Volumentexturen zu BC/ASTC, Resolve-Ziel braucht `renderTarget`, Clear-Wert-Rundung).

**Zwei Fragen, zwei Orte** [Entscheidung]: Das Modell sagt, was *Metal* erlaubt bzw. garantiert (Gast-Fehler =
`InvalidArgument`). `caps` sagt, was *diese GPU* (plus aktivierte Emulationen) davon leisten kann; Lücken sind
`ModelError::Unsupported`/`FormatUnsupportedOnDevice` und erreichen den Gast als `Unimplemented`. Frontend G prüft
jede Payload gegen `ReportedFeatures::guest_limits` und die Gerätelücken (`caps::texture_support` für Texturen und
Views, `caps::render_pipeline_support` für Pipelines inkl. `vertex_input_support`, `caps::render_pass_support` für
Pässe), bevor das Backend sie sieht; der `NullBackend` prüft beim Submit noch einmal gegen die Gerätegrenzen.
Reihenfolgefehler im Encoder (Bind/Draw außerhalb eines Passes, Draw ohne Pipeline, offener Encoder beim Commit)
sind `ModelError::EncoderState` → `InvalidCommand`.

**Grenzen, die der Gast kennt** [Entscheidung]: Im HELLO_ACK stehen Pufferlänge, Texturgrößen, Array-Layer, Threads
und Farb-Attachments; dort gilt `ReportedFeatures::limits` (Gerät ∩ Apple7). Sample-Anzahl, Vertex-Attribute/-Buffer
und Texture-Buffer-Breite stehen **nicht** auf dem Draht, der Gast nimmt Apple7 an (4 Samples, 31/31, 256 Mi px).
`guest_limits` enthält deshalb für diese Felder die Apple7-Werte: Überschreitung = Gast-Fehler; liegt die
Gerätegrenze darunter (Iris: 29 Attribute/28 Buffer [Annahme], `maxTexelBufferElements` 2^27 [Annahme]), ist das eine
Fähigkeitslücke → `Unsupported` (`caps::vertex_input_support`, `caps::texture_support`).

### 4.1 Gerät, Queues, Command Buffer, Encoder

| Metal | Vulkan / Ritsu | Anmerkungen |
|---|---|---|
| `MTLDevice` | `VkPhysicalDevice` + `VkDevice` (ein Backend) | Name, `registryID`, `recommendedMaxWorkingSetSize` aus `ReportedFeatures`/Budget. `hasUnifiedMemory` = YES (Gast ist „Apple Silicon“, §6). `supportsFamily:` aus §5. |
| `MTLCommandQueue` | Host-`Queue`-Objekt: Submission-Thread + eigene Timeline-Semaphore, multiplexed auf wenige `VkQueue`s | NVIDIA bietet viele Queues pro Familie, Iris Xe (Gen12) praktisch eine Render-Queue [Annahme]. Reihenfolge zwischen Metal-Queues entsteht nur über Events; Ritsu darf mehrere Metal-Queues auf eine `VkQueue` serialisieren (korrekt, ggf. weniger parallel). |
| `MTLCommandBuffer` | Encoder-Puffer → ein oder mehrere primäre `VkCommandBuffer`s aus einem Pool pro Aufnahme-Thread | `enqueue`/`commit` legen die Ausführungsreihenfolge fest; erst dann werden Barrieren zum Queue-Zustand aufgelöst (§7.3). `status`, `error`, `GPUStartTime/GPUEndTime` aus Timestamps. |
| `addCompletedHandler`/`waitUntilCompleted` | Timeline-Wert + Completion-Seite im Gastspeicher + Interrupt (§8) | Handler laufen im Gast, nie auf dem Host. |
| `MTLRenderCommandEncoder` | `vkCmdBeginRendering`/`EndRendering` + dynamischer Zustand | §4.8. Parallel-Render-Encoder (`MTLParallelRenderCommandEncoder`) → mehrere Encoder-Puffer, die zu einem Rendering-Block zusammengeführt werden (Sekundär-Command-Buffer mit `VK_RENDERING_CONTENTS_SECONDARY_COMMAND_BUFFERS_BIT`). |
| `MTLComputeCommandEncoder` | `vkCmdDispatch`/`vkCmdDispatchIndirect` | `MTLDispatchTypeSerial` (Default): Ritsu fügt zwischen Dispatches Barrieren für getrackte Ressourcen ein. `Concurrent`: nur bei `memoryBarrier`. `dispatchThreads` (nicht-uniforme Gitter) → aufgerundete Gruppen + Bounds-Check im übersetzten Shader. Gitter > `maxComputeWorkGroupCount` → Aufteilen in mehrere Dispatches mit Basis-Offset (Metal erlaubt bis 2³²−1 pro Dimension). |
| `MTLBlitCommandEncoder` | `vkCmdCopyBuffer/Image/BufferToImage`, `vkCmdFillBuffer`, `vkCmdBlitImage` (Mip-Generierung) | `synchronizeResource` (Managed) → Kopie Gerät→Host-Spiegel; `optimizeContentsForGPUAccess` → No-Op; `copyIndirectCommandBuffer` → §4.10. Überlappende Kopien im selben Buffer sind in Metal ungültig → Gast-Fehler (im `NullBackend` schon geprüft). |
| `MTLAccelerationStructureCommandEncoder` | `vkCmdBuildAccelerationStructuresKHR`, Refit = Update-Modus, Compaction über Query + Copy | Instanz-Deskriptor-Layouts unterscheiden sich → Umpack-Compute-Pass (§4.12). |
| `MTLResourceStateCommandEncoder` | Sparse-Bindung (`vkQueueBindSparse`) | Nur wenn beide GPUs `sparseResidencyImage2D` haben [Annahme: RTX ja, Iris ?]; sonst Sparse-Texturen nicht melden. |
| **Metal 4** (seit macOS 26 [öffentlich]): `MTL4CommandQueue`, `MTL4CommandBuffer`, `MTL4CommandAllocator` | `MTL4CommandAllocator` ≈ `VkCommandPool` (explizites Reset); Command Buffer sind von Queues entkoppelt | Metal 4 hat **kein** automatisches Hazard-Tracking: Barrieren (`barrierAfterQueueStages:beforeStages:` u. a.) bilden fast 1:1 auf `vkCmdPipelineBarrier2` ab – einfacher als Metal 3. |
| `MTL4ArgumentTable` | Bereich im Descriptor Buffer bzw. Push-Daten mit BDAs/Resource-IDs | §4.5 |
| `MTLResidencySet` (macOS 15+, in Metal 4 Pflicht) | kein Residency-Zwang in Vulkan; Hinweis für Budget/Priorität (`memory_priority`) | |
| `MTL4Compiler` (asynchrone Kompilierung) | Ritsus Pipeline-Compile-Pool (§11) | |
| `MTLTensor` / ML in Shadern (Metal 4) | `VK_KHR_cooperative_matrix` oder Shader-Emulation | [Offen] G-9: Umfang in macOS 27. |

### 4.2 Buffer und Speichermodi

| Metal | Vulkan / Ritsu |
|---|---|
| `MTLStorageModeShared` | Platzierung nach §6: (a) importierter Gastspeicher, (b) ReBAR-VRAM mit Gast-Mapping, (c) VRAM-Spiegel + Dirty-Tracking. Sichtbarkeitsregel von Metal: CPU-Schreiben vor `commit` ist für den Command Buffer sichtbar; GPU-Schreiben ist nach Completion für die CPU sichtbar. |
| `MTLStorageModeManaged` | VRAM + Host-Spiegel; `didModifyRange:` → Upload des Bereichs vor der nächsten Nutzung; `synchronizeResource` → Download im Blit-Encoder. Auf Apple Silicon selten, aber gültig. |
| `MTLStorageModePrivate` | `DEVICE_LOCAL`-Speicher, kein Host-Zugriff. |
| `MTLStorageModeMemoryless` | Nur Texturen (Buffer → `ModelError::MemorylessBuffer`). Transiente Attachments, §4.9. |
| `MTLCPUCacheModeWriteCombined` | Hinweis „CPU schreibt nur“ → bevorzugt ReBAR-VRAM (§6.2 b). |
| `MTLHazardTrackingMode` | Steuert §7: `Tracked` (Default für Geräte-Ressourcen) / `Untracked` (Default für Heap-Ressourcen). |
| `gpuAddress` | **Echte `VkDeviceAddress`** [Entscheidung]: Der Gast sieht die BDA des Host-Buffers. Shader dereferenzieren ohne Übersetzung, Zeigerarithmetik im Gast bleibt gültig. Sicherheitsfolge in §8.5. |
| `newBufferWithBytesNoCopy:` | Import der Gast-Seiten (§6.2 a) – kein Kopieren, wie auf Apple Silicon. |
| `contents` | Gast-virtuelle Adresse der Backing-Seiten (Sache des Gast-Treibers); der Host sieht nur gast-physische Seitenlisten. |

### 4.3 Texturen und Pixelformate

| Metal | Vulkan / Ritsu |
|---|---|
| `MTLTexture` (Typen 1D…3D, Cube, Arrays, Multisample) | `VkImage` + Default-`VkImageView`. Cube-Arrays: `array_length × 6` Layer (`TextureDesc::layer_count`). |
| `MTLTextureTypeTextureBuffer` / Buffer-Texturen (`newTextureWithDescriptor:offset:bytesPerRow:`) | Texel-Buffer-View oder linear gekacheltes Image auf dem Buffer-Speicher; `bytesPerRow`-Ausrichtung prüfen (`minTexelBufferOffsetAlignment`). |
| `MTLTextureUsagePixelFormatView` | `VK_IMAGE_CREATE_MUTABLE_FORMAT_BIT` + Formatliste |
| `MTLTextureSwizzleChannels` | `VkComponentMapping` (nur Sampled-Views; Metal erlaubt Swizzle ebenfalls nur lesend) |
| `replaceRegion:`/`getBytes:` | Staging-Kopie oder `VK_EXT_host_image_copy`; Zeilen-/Bildabstände des Gasts werden gegen die Region validiert (geprüfte Arithmetik). |
| `IOSurface`-Texturen | Eigenes Host-Objekt „Surface“ (VkImage + optional linearer CPU-Schatten), `IOSurfaceLock/Unlock` als Synchronisationspunkte. Kernpfad für WindowServer (M5). Transport in Variante P: [Spec nötig]. |
| Sparse-Texturen | §4.1 |
| Lossless Compression (Apple-intern) | unsichtbar; auf Intel ggf. CCS-Kompression des Treibers |

**Formatmodell (WP-GPU-02).** `model::format::PixelFormat` enthält **alle** Werte von `MTLPixelFormat` (öffentlicher
SDK-Header, gegengeprüft mit den daraus generierten `objc2-metal`-Bindings; Test `table_matches_sdk_values_exactly`
listet jeden Rohwert unabhängig von der Tabelle). `MTLPixelFormatUnspecialized` (263, Metal-4-Pipelines) ist bekannt,
aber `Unsupported`. Pro Format: Block (B×H, Bytes; PVRTC mit Mindestgröße 2×2 Blöcke), Kompressionsfamilie (BC,
ETC2/EAC, ASTC LDR, ASTC HDR, PVRTC), Numerik (Unorm/Snorm/Uint/Sint/Float/Extended Range), sRGB, Aspekte, Zugriff als
Texture-Buffer und die **von Metal garantierten Fähigkeiten** (`metal_caps`: Sample, Filter, Write, Color, Blend,
MSAA, Resolve, Sparse, Atomic, Depth, Stencil) aus der Spalte **Apple7** der *Metal Feature Set Tables* (Ausgabe
2026-05-21; laut Fußnote 1 die Untergrenze aller Metal-3/4-Geräte) [öffentlich]. `Depth24Unorm_Stencil8` und
`X24_Stencil8` stehen in keiner Apple-Spalte → nicht erzeugbar; `X32_Stencil8` nur als View.

**Format ≠ GPU** [Entscheidung]: Was Metal verlangt, steht am Format; was die GPU kann, steht in
`Capabilities::formats` (`FormatCapsTable`, für die Profile `rtx4050_assumed`/`iris_xe_assumed` **[Annahme]** bis
WP-GPU-03 die Werte per `vkGetPhysicalDeviceFormatProperties` einsetzt). `caps::format_support` vergleicht beides
plus aktivierte Emulationen (Transkodierung), `caps::texture_support` prüft jede Textur gegen die Fähigkeiten, die
ihre Nutzung braucht. Fehlt etwas, wird die Textur mit `Unimplemented` abgelehnt – nie still falsch.
Angenommene Lücken auf **beiden** GPUs (müssen vor M5 emuliert oder bestätigt werden):

| Format | Metal (Apple7) | GPU (angenommen) | Folge / nötige Emulation |
|---|---|---|---|
| alle `*_sRGB` | Write | kein Storage auf sRGB | UNORM-View + sRGB-Kodierung im Shader (shader-compiler) |
| `RGB9E5Float` | Color, Blend, MSAA, Resolve, Write | nur Sample/Filter | Render-Target über `R32Uint` + Packen; Blending nicht emulierbar → offen |
| `R8Unorm_sRGB`, `RG8Unorm_sRGB`, `A8Unorm`, `A1BGR5Unorm`, `ABGR4Unorm` | Color (… ) | nur Sample/Filter | Rendern in UNORM/R8 + Umrechnung im Shader |
| `BGRA10_XR(_sRGB)`, `BGR10_XR(_sRGB)` | alles | keine Entsprechung | `RGBA16Float` intern (G-6) – **WindowServer-relevant (M5)** |
| ASTC (LDR/HDR), ETC2/EAC, PVRTC | Sample, Filter | nichts | Transkodierung (`EmulationPolicy::astc_transcode`, `etc_eac_transcode`, `pvrtc_transcode`) |

**Formatabbildung** Metal → Vulkan (vollständige Tabelle in WP-GPU-05; Auszug):

| Metal | Vulkan | Bemerkung |
|---|---|---|
| `RGBA8Unorm(_sRGB)`, `BGRA8Unorm(_sRGB)`, `RGBA16Float`, `RGBA32Float`, `RG11B10Float`, `RGB10A2Unorm` | direkte Entsprechung | |
| `BGR10A2Unorm` | `A2R10G10B10_UNORM_PACK32` | Kanalreihenfolge geprüft per Testszene [Annahme] |
| `A8Unorm` | `A8_UNORM_KHR` (maintenance5) oder `R8_UNORM` + Swizzle `000R` | |
| `B5G6R5Unorm`, `A1BGR5Unorm`, `ABGR4Unorm`, `BGR5A1Unorm` (Apple-Familie) | `*_PACK16`-Formate + Swizzle | Renderbarkeit pro GPU prüfen |
| `RGB9E5Float` | `E5B9G9R9_UFLOAT_PACK32` | als Render-Target meist nicht unterstützt → `R32Uint` + Packen im Shader |
| `BGRA10_XR`, `BGRA10_XR_sRGB`, `BGR10_XR` (Extended Range, EDR) | **keine Entsprechung** | `RGBA16Float` intern + Umrechnung an IOSurface-/Anzeige-Grenzen [Offen] G-6 |
| `Depth16Unorm`, `Depth32Float`, `Stencil8`, `Depth32Float_Stencil8` | direkt (`D32_SFLOAT_S8_UINT` statt gepacktem Layout) | `Depth24Unorm_Stencil8` gibt es auf Apple Silicon nicht → abgelehnt |
| `X32_Stencil8`-Views | Stencil-Aspekt-View | |
| `GBGR422`, `BGRG422` | `G8B8G8R8_422_UNORM`/`B8G8R8G8_422_UNORM` | |
| BCn | `BC1…BC7` | nativ auf beiden GPUs |
| ASTC LDR/HDR | **keine Hardware** auf RTX 4050 und Iris Xe [Annahme für Iris: Gen12 hat die ASTC-LDR-Einheit von Gen9–11 nicht mehr] | Transkodierung beim Upload (s. u.) |

**ASTC-Strategie** [Entscheidung, Details WP-GPU-05]: ASTC-Dekodierung ist bit-genau spezifiziert. Default ist
**exaktes Dekodieren nach `RGBA8`** (LDR) bzw. `RGBA16Float` (HDR) – korrekt, kostet aber 4× (4×4-Blöcke) bis 16×
(8×8-Blöcke) Speicher. Optional (Policy-Schalter, Default aus) Neukodierung nach BC7: 1 B/px, aber verlustbehaftet,
also „Abweichung mit Ansage“ und nur mit Zustimmung [Offen] G-5. Dekodierung zunächst auf der CPU (E-Kerne),
später als Compute-Shader. Teil-Uploads (`replaceRegion` auf Blockgrenzen) werden blockweise transkodiert.
Ohne aktivierte Transkodierung meldet Ritsu ASTC nicht (`ReportedFeatures::astc`).

Weitere Regeln: Vulkan verbietet Storage-Images mit sRGB-Formaten; Metal-Shader-Writes auf sRGB → UNORM-View +
Kodierung im übersetzten Shader. Komprimierte Formate sind nie Render-Targets (im Modell bereits geprüft).

**Texturen im Modell:** Typ/Format-Regeln aus `metal_caps` (Render-Target nur mit Color/Depth/Stencil, Shader-Write
nur mit Write, Atomics nur `R32Uint`/`R32Sint` auf Apple7, MSAA-Typen nur mit MSAA, Texture-Buffer nur mit Formaten
aus Apples Texture-Buffer-Tabelle), Sample-Anzahl ≤ `ResourceLimits::max_sample_count` (Apple7: 4). Texturtypen pro
Kompressionsfamilie [öffentlich, Apple-Formatseiten]: ETC2/EAC und PVRTC nur 2D, 2DArray, Cube; BC und ASTC zusätzlich
CubeArray (Apple4) und 3D („Compressed volume texture formats“, Apple3; die Tabelle nennt keine Formate, die Zuordnung zu
BC/ASTC ist [Annahme], weil ETC2/EAC/PVRTC durch ihre eigenen Seiten ausgeschlossen sind). 4:2:2 (`GBGR422`,
`BGRG422`) [öffentlich, Apple-Formatseiten]: nur 2D ohne Mips, Breite gerade (sonst Gast-Fehler); beim Sampling
verlangt Apple `clampToEdge` (Sampler-seitig, erst bei der Bindung prüfbar). Texture-Buffer: Breite ≤
`max_texture_buffer_width` (Apple7: 256 Mi px) und ≤ `maxBufferLength` / Pixelgröße (nicht die 1D-Grenze). Größen
werden mit geprüfter Arithmetik berechnet; `Depth32Float_Stencil8` wird mit 8 B/px gerechnet (Apple: „some devices
allocate 64 bits per pixel“).

**Texture-Views** (`model::view::TextureViewDesc`): Formate kompatibel, wenn gleiche Bitbreite (unkomprimierte
Farbformate), sRGB↔linear oder Depth/Stencil ↔ Stencil-View; `PixelFormatView`-Usage nötig, sobald sich Anzahl, Größe
oder Reihenfolge der Komponenten ändern (Ausnahmen laut Apple: sRGB↔linear, RGBA8↔BGRA8 mit sRGB-Wechsel). Typ-Casts nach
Apples Tabelle (2D→2D/2DArray; 2DArray/Cube/CubeArray untereinander; 1D, 3D nur auf sich selbst; übrige Typen nur auf
sich selbst [Entscheidung]). Level-/Slice-Bereiche mit geprüfter Arithmetik; Cube = 6 Slices, Cube-Array = n × 6.
View-Swizzle wird mit dem Swizzle der Textur **kombiniert** [öffentlich, Apple-Doku zu
`makeTextureView(…swizzle:)`]: `final[c] = parent[view[c]]`, Konstanten bleiben Konstanten (Apples Beispiel
`[R,G,A,B]` auf `[R,1,1,G]` → `[R,1,G,1]` ist Testfall). Vulkan-Hinweis: Attachment- und Storage-Views brauchen
Identitäts-Swizzle → Backend hält eine zweite, unswizzelte View.

### 4.4 Sampler

Modell: `model::sampler::SamplerDesc` mit allen Feldern des Deskriptors und `validate()` [öffentlich: Apple-Doku der
Eigenschaften, Feature-Tabellen]: `maxAnisotropy` 1…16, LOD-Clamps ohne NaN und `min ≤ max`, nicht normalisierte
Koordinaten nur unter Apples Bedingungen (clampToEdge/clampToZero für s/t, notMipmapped, min = mag, Anisotropie 1 –
sonst „undefiniert“ in Metal und verboten in Vulkan → Gast-Fehler). Nicht normalisierte Koordinaten mit
`compareFunction` ≠ `never` verbietet Apple nicht, Vulkan schon (`VUID-VkSamplerCreateInfo-unnormalizedCoordinates-01076`)
→ `Unsupported`; `never` ist der SDK-Default und nicht von „kein Vergleich“ unterscheidbar, bleibt also erlaubt
(`sample_compare` über einen solchen Sampler muss bei der Bindung abgelehnt werden). `reductionMode` ≠ WeightedAverage und `lodBias` ≠ 0
sind Apple10-Features → `Unsupported`, solange Ritsu höchstens Apple7 meldet. **G-7 entschieden:** `lodAverage` ist laut
Apple ein Performance-Hinweis, der „auf manchen GPUs ignoriert“ wird → Ritsu akzeptiert und ignoriert ihn (konform).
Backend-Hinweise: Border-Farben brauchen Float- und Int-Varianten je nach Texturformat; ein Metal-Sampler mit
`compareFunction` wird zu zwei `VkSampler` (mit/ohne Compare), weil Vulkan normale Samples mit Compare-Sampler nicht
erlaubt; bei nicht normalisierten Koordinaten setzt das Backend `minLod = maxLod = 0` (äquivalent wegen notMipmapped
und min = mag).

`MTLSamplerDescriptor` → `VkSampler` (gecacht nach Deskriptor-Hash):
Filter/Mip-Filter 1:1; `clampToEdge`/`repeat`/`mirrorRepeat`/`mirrorClampToEdge` 1:1; `clampToZero` →
`CLAMP_TO_BORDER` + transparentes Schwarz; `clampToBorderColor` → drei Standard-Border-Farben (bei Formaten mit
Swizzle `border_color_swizzle` bzw. Custom Border Color); `lodMin/MaxClamp`, `maxAnisotropy`, `compareFunction` 1:1;
`normalizedCoordinates = NO` → `unnormalizedCoordinates` (Einschränkungen s. o.); `lodAverage` → ignoriert (s. o.).
`supportArgumentBuffers` → Sampler bekommt einen Platz im globalen Sampler-Heap (§4.5). `constexpr sampler` in MSL →
air2spirv meldet sie als Metadaten; der Host legt unveränderliche Sampler im Pipeline-Layout an.

### 4.5 Argument Buffers, `gpuAddress`, `gpuResourceID`

* **Tier 1** (Strukturen mit Bindings): air2spirv erzeugt ein Descriptor-Set-Layout pro Argument-Buffer-Struktur;
  der Host schreibt Deskriptoren beim Encoden. Nur Fallback.
* **Tier 2** (Ziel, Pflicht für Mac2/Apple7/Metal3):
  * Buffer-Felder enthalten `gpuAddress` = echte BDA (§4.2) → keine Übersetzung nötig.
  * Textur-/Sampler-Felder enthalten `MTLResourceID` (64 Bit). Ritsu vergibt sie als **Index in einen globalen
    Deskriptor-Heap pro Typ** (Sampled Image, Storage Image, Sampler; per Descriptor Buffer oder einem großen
    update-after-bind-Set) plus Generationsbits im oberen Wort. Der übersetzte Shader indiziert `heap[id & mask]`
    `nonuniform`. Freigegebene IDs werden auf Null-Deskriptoren (`robustness2`) gesetzt und erst nach Ablauf aller
    referenzierenden Submissions wiederverwendet → stale IDs lesen 0 statt den Host zu gefährden.
  * Argument-Buffer-Inhalte schreibt der Gast selbst (CPU oder GPU) – der Host muss **nichts** umschreiben. Das ist
    der Grund für BDA + globale Heaps: Metals Bindless-Modell bleibt im Speicher unverändert.
  * `useResource:usage:`/`useHeap:` liefern die Zugriffsmenge für das Hazard-Tracking (§7) und Residency-Hinweise.
* Puffergrößen für Deskriptor-Heaps: anfangs 1 Mi Sampled Images / 64 Ki Storage Images / 4 Ki Sampler [Annahme,
  Grenzen der Treiber prüfen: `maxDescriptorBufferBindings`, `maxSamplerDescriptorBufferRange`, ANV-Limits].

### 4.6 Heaps, Aliasing, Residency

| Metal | Vulkan / Ritsu |
|---|---|
| `MTLHeap` (`Automatic`) | ein `VkDeviceMemory`-Block; Ritsu suballokiert intern (gleicher Allocator wie §8.3, aber festes Budget = Heap-Größe) |
| `MTLHeap` (`Placement`) | Gast bestimmt Offsets → `vkBind*Memory(offset)`; Ausrichtung/Größe aus `heapBufferSizeAndAlignWithLength:` müssen **zu den Vulkan-Anforderungen passen** → Ritsu meldet die strengeren Vulkan-Werte (Maximum beider) |
| Aliasing (`makeAliasable`, überlappende Placement-Ressourcen) | mehrere Ressourcen auf demselben Speicher; Inhalt nach Wechsel undefiniert (wie in Metal); Layout-Übergang aus `UNDEFINED` beim ersten Zugriff |
| `MTLHeapTypeSparse` | Sparse-Speicher (§4.1) |
| Heap-Ressourcen sind standardmäßig `Untracked` | Synchronisation über `MTLFence`/Events/Barrieren des Gasts (§7.5) |
| `MTLResidencySet`, `useResidencySet:` | Hinweis, kein Zwang (§4.1) |

### 4.7 Synchronisationsobjekte

| Metal | Vulkan / Ritsu |
|---|---|
| `MTLEvent` | Timeline-Semaphore (gerätelokal) |
| `MTLSharedEvent` | Timeline-Semaphore + `signaledValue` gespiegelt in eine Gast-lesbare Seite; CPU-`setSignaledValue` → `vkSignalSemaphore`; `notifyListener` → Gast-Treiber über Completion-Interrupt |
| `encodeWaitForEvent:value:` / `encodeSignalEvent:value:` | Wait/Signal an Submission-Grenzen. Ein Wait **in der Mitte** eines Command Buffers teilt ihn in zwei Vulkan-Submissions. Wait-before-signal: Der Submission-Thread hält die Submission zurück, bis die passende Signal-Operation selbst übergeben wurde (wie DXVK/vkd3d), statt sich auf Treiberverhalten zu verlassen. |
| `MTLFence` | keine Vulkan-Entsprechung: `updateFence(afterStages)` merkt sich einen Signalpunkt im Encoder-Puffer; `waitForFence(beforeStages)` erzeugt eine globale Speicherbarriere (src = Stages des Signalpunkts) – innerhalb eines Command Buffers sofort, über Command-Buffer-Grenzen derselben Queue bei der Auflösung in §7.3 |
| `memoryBarrierWithScope:` / `…Resources:` | `VkMemoryBarrier2` bzw. Buffer-/Image-Barrieren; im Render-Encoder siehe §7.6 |
| `MTLSharedEventHandle` (prozessübergreifend) | nur im Gast relevant; der Host kennt Events über sitzungsglobale IDs |

### 4.8 Render-Pässe und Rasterisierung

| Metal | Vulkan / Ritsu |
|---|---|
| `MTLRenderPassDescriptor` | `VkRenderingInfo` (dynamic rendering) |
| `MTLLoadActionDontCare/Load/Clear` | `LOAD_OP_DONT_CARE/LOAD/CLEAR` |
| `MTLStoreActionStore/DontCare` | `STORE_OP_STORE/DONT_CARE` |
| `MultisampleResolve`, `StoreAndMultisampleResolve` | `resolveMode` (`AVERAGE`; Tiefe: `SAMPLE_ZERO/MIN/MAX` über `depthStencilResolve`; Stencil `Sample0` → `SAMPLE_ZERO`, `DepthResolvedSample` nur zusammen mit Tiefen-Resolve `Sample0`, sonst `Unsupported`). Modelliert in `RenderPassDesc` (WP-GPU-02): Resolve-Textur Pflicht, Quelle multisampled, Ziel single-sample, gleiches Format, Format mit Resolve-Fähigkeit, Ziel deckt die Render-Fläche ab. Kombinierte Depth/Stencil-Textur: `RenderTargetLayout` meldet verschiedene Resolve-Ziele (`ds_resolve_targets_differ`, nie ausdrückbar), Resolve nur eines Aspekts (`ds_resolve_one_aspect`, braucht `independentResolveNone`) und unterschiedliche Modi (`ds_resolve_modes_differ`, braucht `independentResolve`); `caps::render_pass_support` entscheidet. Auf dem Draht: `BeginRenderPass2` (Resolve-Textur mit Level/Slice/Ebene pro Attachment); die Kompaktform `BeginRenderPass` hat keine Resolve-Textur → resolvierende Store-Action ist dort ein Gast-Fehler (`InvalidArgument`, Command Buffer `Error`) |
| `MTLStoreActionUnknown` + späteres `setColorStoreAction:` / `setDepthStoreAction:` / `setStencilStoreAction:` | Drahtbefehl `SetStoreAction` (WP-GPU-24). Nur ein noch unbekannter Store-Action darf ersetzt werden, nur durch eine bekannte; der Pass wird mit dem neuen Wert vollständig neu validiert (Resolve-Textur vorhanden, Memoryless nicht gespeichert, Gerätelücken). Nie gesetzt → `EndRenderPass` scheitert (`InvalidCommand`). Durch den Encoder-Puffer erst bei `endEncoding` nach Vulkan übersetzt |
| `storeActionOptions: CustomSamplePositions`, `MTLStoreActionCustomSampleDepthStore` | `VK_EXT_sample_locations` – bis dahin `Unsupported` |
| Getrennte Depth- und Stencil-Texturen (Apple2-Feature) | Vulkan Dynamic Rendering verlangt **dieselbe** Image-View für Tiefe und Stencil → Emulation (kombiniertes `D32S8`-Schattenbild + Kopien) offen; bis dahin `Unsupported` (`caps::render_pass_support`) |
| Clear-Werte | Farbe (`double`) → Float/Uint/Sint je Format (Integer: Richtung null gerundet, gesättigt, NaN → 0 [Annahme]); `clearDepth` NaN → Gast-Fehler, außerhalb [0, 1] → `Unsupported` (ohne `depth_range_unrestricted`); `clearStencil` (u32) → untere 8 Bit |
| Render-Pipeline (ohne Shader, `model::pipeline`) | Farb-Attachments mit Blend-Faktoren/-Operationen und Write-Mask, Depth/Stencil-Formate (kombiniertes Format muss für beide gleich sein), `rasterSampleCount` (Apple7 ≤ 4), Alpha-to-Coverage/-One, Topologie-Klasse (Pflicht bei Layered Rendering), Vertex-Deskriptor. Blending nur mit Blend-fähigem Format, Dual-Source nur auf Attachment 0 ohne weitere Farb-Attachments [Annahme, konservativ wie `maxFragmentDualSrcAttachments` = 1]. Metal-4-„Unspecialized“-Werte → `Unsupported`. `check_compatible` prüft Pipeline ↔ Render-Pass (Formate je Slot, Tiefe/Stencil, Sample-Anzahl) bei `setRenderPipelineState:` (dort prüft auch Metal); beim Draw: Pipeline gebunden, Primitivtyp passt zur Topologie-Klasse, jeder Vertex-Buffer des Vertex-Deskriptors gebunden (dynamischer Stride nur mit `attributeStride`). Gerät: `caps::render_pipeline_support` (Attachment-Formate nativ renderbar/blendbar/MSAA, Sample-Grenze, Vertex-Input-Lücken) |
| Vertex-Deskriptor (`model::vertex`) | alle `MTLVertexFormat`-Werte (Größe, Komponenten, natürliche Ausrichtung, Numerik); Step-Regeln laut Apple (PerVertex: Rate 1, PerInstance: > 0, Constant: 0; PerPatch* → `Unsupported` bis Tessellation); Apple7: 1-Byte-Ausrichtung, keine Stride-Grenze, dynamischer Stride. Lücken zu Vulkan (Stride > `maxVertexInputBindingStride`, Offset > `maxVertexInputAttributeOffset`, unausgerichtete Zugriffe, Divisoren) prüft `caps::vertex_input_support` |
| `renderTargetArrayLength`, `[[render_target_array_index]]` | `layerCount` + `shaderOutputLayer` |
| `setVertexAmplificationCount:` | Multiview (`viewMask`) mit Einschränkungen [Offen] |
| Visibility-Result-Buffer (Okklusion, Boolean/Counting) | Occlusion-`VkQueryPool`; am Pass-Ende `vkCmdCopyQueryPoolResults` in den Gast-Buffer (Metal schreibt die Ergebnisse ebenfalls am Ende) |
| Viewport/Scissor/Cull/Winding/Depth-Bias/Stencil-Referenz/Blend-Color | dynamischer Zustand (Core 1.3 + EDS3) |
| `setDepthClipMode:Clamp` | `depthClampEnable` (EDS3) |
| `setTriangleFillMode:Lines` | `polygonMode` (EDS3) |
| Tessellation (Compute-Kernel schreibt Faktoren, Post-Tessellation-Vertex-Funktion) | Vulkan braucht einen TCS: Ritsu erzeugt einen Durchreich-TCS, der die Faktoren aus dem Metal-Faktor-Buffer liest; die Post-Tessellation-Vertex-Funktion wird zum TES. Partitionierung/Winding 1:1 [Annahme, Testszene nötig]. |
| Koordinatensysteme | Metal-NDC hat +y nach oben, Vulkan +y nach unten → negativer Viewport (Core seit 1.1); dadurch kehrt sich der Umlaufsinn → `frontFace` wird invertiert. Tiefenbereich [0,1] identisch. Muss per Testszene gegen Apple-Referenzbild belegt werden. |

### 4.9 TBDR-Konzepte auf IMR-GPUs

| Metal | Strategie | Gemeldet |
|---|---|---|
| Memoryless-Attachments | transiente Attachments (`TRANSIENT_ATTACHMENT`); Speicher aus einem Pool, der zwischen Pässen aliasiert wird (Desktop-GPUs haben keinen `LAZILY_ALLOCATED`-Speicher). Load/Store auf Memoryless = Gast-Fehler (geprüft im `NullBackend`). | ja (emuliert, korrekt) |
| Programmierbares Blending / Framebuffer Fetch (`[[color(n)]]` als Eingabe) | `dynamic_rendering_local_read` (Input-Attachment mit Local Read). Ein Kopier-Fallback zwischen Draws wäre **falsch** (überlappende Dreiecke desselben Draws), daher: ohne Erweiterung → Interlock + Storage-Image oder nicht melden. | wenn nativ |
| Raster Order Groups | `fragment_shader_interlock` (pixel-ordered) | wenn nativ |
| Tile-Shader (`dispatchThreadsPerTile`), Imageblocks, Threadgroup-Speicher im Render-Pass | Pass aufteilen: Rendering beenden → Compute-Dispatch über Kacheln mit Attachments als Storage-Images und Imageblock-Daten in einem pro-Pixel-Storage-Buffer → Rendering mit `LOAD` fortsetzen. Explizite Imageblocks im Fragment-Shader → pro-Pixel-Buffer + Interlock. Teuer, nur per Policy. | nur mit `EmulationPolicy::tile_shaders` |
| `tileWidth/Height`, `imageblockSampleLength` | werden validiert und gespeichert, wirken nur auf die Emulation | – |

Ohne Tile-Shader-Emulation meldet Ritsu **keine Apple-Familie** (§5). Ob macOS-27-Systemkomponenten (WindowServer,
Core Animation, Core Image) eine Apple-Familie voraussetzen, ist die wichtigste offene Frage dieses Subsystems
(G-1).

### 4.10 Indirect Command Buffers

* **CPU-kodierte ICBs** (häufigster Fall): Ritsu zeichnet die ICB-Befehle in eine eigene Liste auf und spielt sie
  bei `executeCommandsInBuffer` als normale Vulkan-Befehle ab (oder als vorbereiteter Sekundär-Command-Buffer).
* **GPU-kodierte ICBs** (MSL `command_buffer` in Kernels): mit `VK_EXT_device_generated_commands` übersetzt
  air2spirv die Encode-Aufrufe in DGC-Token-Ströme. Ohne DGC: ICBs mit geerbtem Pipeline-Zustand → indirekte Draws
  (`vkCmdDrawIndirectCount`); alles andere → **Readback und CPU-Replay** (Stall, aber korrekt). ICBs lassen sich dem
  Gast nicht verschweigen (sie gehören zu jeder relevanten Familie), deshalb muss es immer einen korrekten Pfad geben.

### 4.11 Mesh- und Object-Shader

* RTX: `VK_EXT_mesh_shader` (Object → Task, Mesh → Mesh). Grenzen von Payload, Ausgabevertices/-primitiven und
  Threadgroup-Größen werden aus beiden APIs geschnitten gemeldet.
* Iris Xe (kein Mesh-Shading auf Xe-LP [Annahme]): Emulation per Compute (Object- und Mesh-Stufe schreiben Vertex-
  und Index-Buffer + indirekte Draw-Argumente, danach Durchreich-Vertex-Shader). Nur per Policy; sonst meldet Ritsu
  keine Mesh-Shader und damit **nicht Metal 3** (`reported_feature_set` bildet das ab).

### 4.12 Ray Tracing

| Metal | RTX (nativ) | Iris Xe |
|---|---|---|
| `MTLPrimitiveAccelerationStructure` (Dreiecke, Bounding Boxes) | BLAS (Triangles/AABBs) | Compute-BVH-Builder (Policy) |
| Curves (seit macOS 14) | AABB + Intersection-Shader-Emulation | wie links |
| `MTLInstanceAccelerationStructure` | TLAS; Instanz-Deskriptoren (Default/UserID/Motion/Indirect) per Compute-Pass in `VkAccelerationStructureInstanceKHR` umpacken | Compute |
| Motion Blur | nur `VK_NV_ray_tracing_motion_blur` → sonst nicht melden | nicht melden |
| `intersector<>` in Compute/Fragment | `ray_query` | Compute-Traversierung im übersetzten Shader |
| Intersection-Function-Tables | Ray-Tracing-Pipeline + SBT, oder Switch-Dispatch in der Ray-Query-Kandidatenschleife | Switch-Dispatch |
| Refit, Compaction, Serialisierung | Update-Build, Compaction-Query + Copy; Serialisierung ist geräteabhängig → nur innerhalb desselben Geräts | – |

### 4.13 Funktionszeiger, Visible Function Tables, dynamische Bibliotheken, Binary Archives

SPIR-V für Vulkan kennt keine Funktionszeiger. [Entscheidung] Indirekte Aufrufe werden zu einem `switch` über alle
Funktionen, die zur Pipeline gelinkt sind (`linkedFunctions`, Einträge der Visible-Function-Table sind Indizes in
diese Menge). `MTLDynamicLibrary` → Linken der AIR-Module auf dem Host vor der SPIR-V-Erzeugung (shader-compiler.md).
`MTLBinaryArchive` enthält Apple-GPU-Code, den Ritsu nicht nutzen kann; Ritsu nutzt die Archive nur als Liste der
Pipeline-Deskriptoren zum Vorkompilieren (§11). Rekursion über Funktionszeiger → begrenzte Tiefe, sonst Gast-Fehler.

### 4.14 Zähler und Zeitstempel

`MTLCounterSampleBuffer` (Timestamp-Set) → `VkQueryPool` (Timestamps), Umrechnung mit `timestampPeriod`;
Stage-Boundary-Sampling → `vkCmdWriteTimestamp2` an Encoder-Grenzen; `sampleTimestamps:gpuTimestamp:` → calibrated
timestamps. Statistik-Zähler → Pipeline-Statistics-Queries, soweit abbildbar; nicht abbildbare Zähler-Sets werden
nicht gemeldet (`counterSets`).

### 4.15 MetalFX

MetalFX (Spatial/Temporal Scaler, seit macOS 26 auch Frame-Interpolation und Denoiser [öffentlich]) ist ein
Gast-Framework. Wenn es intern nur normale Metal-Compute-Shader nutzt, läuft es durch die Übersetzung ohne
Sonderbehandlung. Nutzt es private Treiberpfade, braucht der Host eigene Implementierungen: Spatial → eigener
Upscaling-Compute-Shader; Temporal/Interpolation → zunächst `supportsDevice:` = NO. [Offen] G-8, Spike nötig.

### 4.16 CAMetalLayer, Drawables, Anzeige

`CAMetalLayer`-Drawables sind IOSurface-gestützte Texturen; `presentDrawable:` reicht die Surface an den Compositor
(WindowServer) im Gast weiter. Der Host sieht erst das **fertig komponierte Bild** des Gast-Displays (Scanout-Surface
der virtuellen Anzeige) und bringt es per §12 auf das Panel. Frame-Pacing: Die virtuelle Anzeige liefert dem Gast
VSync-Interrupts, die aus echten Vblank-Zeitstempeln des Host-Displays abgeleitet sind.

## 5. Fähigkeitsmatrix und Melde-Policy

### 5.1 Matrix (Default-Policy = `EmulationPolicy::NATIVE_ONLY`; Ziel = `EmulationPolicy::ALL`)

Abgeleitet aus `Capabilities::rtx4050_assumed()` / `iris_xe_assumed()` mit `reported_feature_set_with` (die Tests
in `caps.rs` sichern genau diese Zeilen ab). N = nativ, E = emuliert, – = nicht gemeldet.

| Metal-Feature | RTX 4050 Default | RTX 4050 Ziel | Iris Xe Default | Iris Xe Ziel | Mechanismus |
|---|---|---|---|---|---|
| Argument Buffers Tier 2 | N | N | N | N | BDA + Descriptor Buffer/Indexing |
| Unified Memory (`hasUnifiedMemory`) | E | E | N | N | §6 |
| Memoryless | E | E | E | E | transiente Attachments |
| BC-Texturen | N | N | N | N | |
| ASTC | – | E | – | E | Transkodierung |
| `atomic<float>` | N | N | – | E | CAS-Schleife |
| Framebuffer Fetch | N | N | N | N | local read |
| Raster Order Groups | N | N | N | N | Interlock |
| Tile-Shader/Imageblocks | – | E | – | E | Pass-Splitting |
| Mesh-Shader | N | N | – | E | Compute-Emulation |
| Ray Tracing | N | N | – | E | Compute-BVH |
| **Familie Mac2** | ja | ja | ja | ja | Tier 2 + BC |
| ETC2/EAC-, PVRTC-Formate | – | E | – | E | Transkodierung (WP-GPU-02: Teil der Apple7-Bedingung) |
| **Familie Apple7** | nein | ja | nein | ja | braucht ASTC, ETC2/EAC, PVRTC, Tile-Shader, Float-Atomics, FB-Fetch, ROG |
| **Familie Metal3** | ja | ja | nein | ja | (Apple7 oder Mac2) + Mesh |
| **Familie Metal4** | nein | ja | nein | ja | Metal3 + Apple7 + Policy `metal4` |

### 5.2 Regeln [Entscheidung]

1. **Nativ oder implementiert.** Ein Policy-Schalter darf erst auf `true`, wenn die Emulation implementiert ist und
   Bildvergleichstests (§13) grün sind. Der Default meldet nur Natives.
2. **Familien sind Konjunktionen** der Einzel-Features (Code: `reported_feature_set_with`). Die genaue Zuordnung
   „Familie ⇒ Features“ ist eine Lesart von Apples öffentlicher Metal-Feature-Set-Tabelle und muss für macOS 27
   geprüft werden [Annahme, G-3].
3. **Grenzen** sind das Minimum aus Gerät und Apple7 (`ReportedFeatures::limits`). Nie mehr melden, als beide können.
4. **Gleiche Meldung auf beiden GPUs?** Ein Gast-Image soll zwischen RTX und Iris wechseln können (Akku-Modus).
   Default: jede GPU meldet ehrlich ihre eigene Menge; die Wahl der GPU erfolgt **vor** dem Gast-Boot. Optionaler
   Schalter „kleinster gemeinsamer Nenner“ [Offen] G-4.
5. **Variante P**: Was der Apple-Gast-Treiber selbst an Familien meldet, bestimmt Apple. Meldet er mehr, als Ritsu
   kann, muss Ritsu die betroffenen Befehle mit Gast-Fehlern ablehnen (laut, nicht falsch) – und das ist ein
   Argument für Variante G [Spec nötig: S-GPU-1].

## 6. Speicher: Unified Memory auf diskreter GPU

### 6.1 Problem

Der Gast glaubt an Apple Silicon: `MTLStorageModeShared` ist Default, CPU und GPU sehen dieselben Bytes, `contents`
ist ein normaler Zeiger, Synchronisation nur an Command-Buffer-Grenzen. Die Iris Xe teilt sich tatsächlich den
Systemspeicher; die RTX 4050 hat 6 GB eigenes VRAM hinter PCIe (4.0 x8 [Annahme] ≈ 16 GB/s statt > 100 GB/s
VRAM-Bandbreite). Gastspeicher ist ein großes Host-Mapping von `ritsu-mem`.

### 6.2 Strategien

| Strategie | Wie | Gut für | Kosten/Risiko |
|---|---|---|---|
| **(a) Gastspeicher importieren** | Die Backing-Seiten des Buffers werden per `VK_EXT_external_memory_host` als `VkDeviceMemory` importiert. Gastseiten (16 KiB) erfüllen die Import-Ausrichtung (typisch 4 KiB [Annahme]). Voraussetzung: **zusammenhängender Host-VA-Bereich**; gast-physisch verstreute Seiten werden über ein memfd-gestütztes Gast-RAM per zweitem `mmap` zu einem zusammenhängenden Alias gelegt. | Iris Xe (immer), RTX für kleine/CPU-lastige Buffer (Uniforms, Staging, Streaming) | Iris: keine. RTX: GPU-Zugriffe über PCIe. Braucht Schnittstelle zu `ritsu-mem` (memfd-Backing, Alias-Mappings) → architect [Offen] G-10 |
| **(b) ReBAR-VRAM in den Gast mappen** | Buffer liegt in `DEVICE_LOCAL | HOST_VISIBLE`-VRAM; dessen Host-Mapping wird als Backing der Gast-Seiten eingeblendet (in Variante G: Gast-Treiber alloziert aus einer „VRAM-Apertur“ im gast-physischen Adressraum). | `CPUCacheModeWriteCombined`, CPU-schreibt-GPU-liest (Vertex-/Konstantendaten) | CPU-Lesen ist sehr langsam (uncached über PCIe). Braucht ReBAR (> 256 MiB). |
| **(c) VRAM-Spiegel + Dirty-Tracking** | Buffer liegt in VRAM, Gast-Seiten bleiben normale Gast-RAM-Seiten. `ritsu-mem` schreibschützt die Seiten im SoftMMU (wie QEMUs Dirty-Logging); Schreibfehler markieren sie schmutzig. Vor jeder Submission, die den Buffer nutzt, werden schmutzige Bereiche hochgeladen. GPU-Schreibzugriffe (aus Shader-Reflexion/`useResource:usage:Write`) werden nach Completion **vor** der Completion-Meldung zurückkopiert. | GPU-lastige Shared-Buffer auf RTX | Dirty-Tracking kostet Schreibfehler im DBT; Zurückkopieren verlängert die Latenz. |
| **Managed** | explizit über `didModifyRange`/`synchronizeResource` | – | genau Metals Semantik |
| **Private** | nur VRAM | alles GPU-Interne | – |

**Platzierungs-Policy** [Entscheidung, Startwerte, später durch Messungen ersetzt (CLAUDE.md §3.8)]:
Iris Xe: immer (a). RTX: Shared startet in (a); ein Buffer wandert nach (c), wenn er in N aufeinanderfolgenden
Submissions von der GPU gelesen, aber selten von der CPU beschrieben wird; `WriteCombined` → (b), falls ReBAR
(`Capabilities::shared_buffers_direct`). Migration ist für den Gast unsichtbar, weil `gpuAddress` (BDA) sich dabei
ändern würde → **Migration nur für Buffer, deren `gpuAddress` nie abgefragt wurde**, sonst bleibt der Buffer, wo er
ist. Alternative: BDA-stabile Platzierung über `bufferDeviceAddressCaptureReplay` [Offen].

### 6.3 Texturen

Texturen haben in Metal keinen direkten CPU-Zeiger (Ausnahme: Buffer-Texturen, IOSurfaces). Shared-Texturen liegen
deshalb immer in VRAM bzw. im Iris-Gerätespeicher; `replaceRegion`/`getBytes` sind explizite Kopien. IOSurfaces:
Lock/Unlock sind die Synchronisationspunkte (Download bei Lock mit Lesezugriff, Upload nach Unlock mit Schreibzugriff),
Details in WP-GPU-15.

## 7. Hazard-Tracking: automatisch (Metal) vs. explizite Barrieren (Vulkan)

Das ist der zentrale Punkt für Korrektheit **und** Leistung. Zu wenige Barrieren = falsche Bilder (selten
reproduzierbar), zu viele = halbe Leistung.

### 7.1 Metal-Semantik, die erhalten werden muss [öffentlich, Details Annahme]

* Für **getrackte** Ressourcen garantiert Metal zwischen Encodern und zwischen Command Buffern **derselben Queue**,
  dass Schreib/Lese-Abhängigkeiten eingehalten werden. Zwischen Queues braucht es Events.
* In einem Compute-Encoder mit `MTLDispatchTypeSerial` gelten Dispatches als nacheinander ausgeführt (Abhängigkeiten
  über getrackte Ressourcen werden eingehalten); mit `Concurrent` nur über `memoryBarrier`.
* In einem Render-Encoder werden Abhängigkeiten zwischen Draws über Shader-Speicherzugriffe **nicht** automatisch
  aufgelöst (nur Attachment-Rasterreihenfolge); dafür gibt es `memoryBarrierWithScope:afterStages:beforeStages:`.
* **Untracked** (Default für Heap-Ressourcen): die App synchronisiert mit `MTLFence`, Events, Barrieren.
* **Metal 4**: kein automatisches Tracking, nur explizite Barrieren → direkte Übersetzung.

### 7.2 Entwurf: zentraler `HazardTracker` im Kern [Entscheidung]

Barrieren entstehen **an genau einer Stelle** (nicht verstreut in Backends):

1. **Zugriffsmenge pro Encoder (bzw. Dispatch bei Serial-Compute).** Quellen: direkt gebundene Ressourcen
   (`setBuffer`/`setTexture`) mit Lese/Schreib-Info aus der Shader-Reflexion (air2spirv liefert pro Binding
   `read`/`write`/`read_write`), Attachments des Render-Passes (Load/Store-Actions), `useResource:usage:`,
   `useHeap:` (Heap = Menge seiner getrackten Ressourcen), Blit-Quellen/-Ziele. Jeder Eintrag:
   `(Ressource, Teilbereich, Stages, Access, Ziel-Layout)`. Teilbereich: Buffer als Bytebereich (grob: ganzer
   Buffer, fein ab einer Schwelle), Texturen pro Mip/Layer-Bereich.
2. **Zustand pro Ressource** (am Ressourcenobjekt gespeichert, Stempel mit Command-Buffer-ID statt Hash-Map):
   `letzter Schreiber (Stages, Access)`, `Leser seit letztem Schreiben (Stage-Vereinigung)`, `aktuelles Layout`.
3. **Vergleich und Sammeln.** RAW, WAW, WAR oder Layout-Wechsel erzeugen eine Barriere; Read-after-Read im
   gleichen Layout nicht. Alle Barrieren vor einem Encoder werden in **ein** `VkDependencyInfo` gebündelt (ein
   `vkCmdPipelineBarrier2` pro Encoder-Grenze).
4. **Command-Buffer-Grenzen.** Metal erlaubt paralleles Encoden mehrerer Command Buffer; die Reihenfolge steht erst
   bei `enqueue/commit` fest. Daher zeichnet jeder Command Buffer mit **unbekanntem Anfangszustand** auf und merkt
   sich pro Ressource den **ersten Zugriff** und den **Endzustand**. Der Submission-Thread der Queue löst bei der
   Übergabe auf: Queue-Zustand × erste Zugriffe → kleiner **Prolog-Command-Buffer** mit den nötigen Barrieren und
   Layout-Übergängen, dann der eigentliche Command Buffer, dann Queue-Zustand := Endzustände. (Bewährtes Muster aus
   DXVK/vkd3d-proton.)
5. **Queue-übergreifend:** keine automatischen Barrieren (wie Metal); Events → Semaphoren. Für Ressourcen mit
   exklusivem Queue-Familien-Besitz nutzt Ritsu `VK_SHARING_MODE_CONCURRENT` bzw. eine einzige Queue-Familie, um
   Ownership-Transfers zu vermeiden [Entscheidung].
6. **Sicherheitsnetz:** Debug-Schalter `RITSU_GPU_BARRIERS=full` setzt an jeder Encoder-Grenze eine Voll-Barriere
   (`ALL_COMMANDS → ALL_COMMANDS`). Unterscheidet sich das Bild, ist der Tracker schuld – schnelle Fehlersuche.

### 7.3 Kosten

O(Anzahl berührter Ressourcen pro Encoder), keine globale Suche. Bindless-Zugriffe ohne `useResource` sind in Metal
undefiniertes Verhalten; Ritsu verhält sich dort wie für untracked (keine Barriere) und meldet es im Debug-Modus.

### 7.4 Image-Layouts

Policy pro Backend: NVIDIA → `GENERAL` für alles außer Attachments/Präsentation (kein Nachteil [Annahme]); Intel →
präzise Layouts, weil `GENERAL` die CCS-Kompression abschalten kann [Annahme]. Mit `VK_KHR_unified_image_layouts`
überall `GENERAL`.

### 7.5 Untracked-Ressourcen und `MTLFence`

Siehe §4.7. Fences tragen keine Ressourcenliste → globale Speicherbarrieren zwischen den Stage-Masken. Auflösung über
Command-Buffer-Grenzen wie §7.2 Punkt 4.

### 7.6 Barrieren innerhalb eines Render-Encoders

Vulkan erlaubt Barrieren innerhalb von dynamic rendering nur eingeschränkt (mit `dynamic_rendering_local_read` für
framebuffer-lokale Abhängigkeiten). Ein Metal-`memoryBarrier` im Render-Encoder, der darüber hinausgeht (z. B.
Storage-Buffer zwischen Fragment- und Vertex-Stufe), führt zu einem **Pass-Split** (Rendering beenden, Barriere,
Rendering mit `LOAD` fortsetzen). Korrekt, aber teuer → Zähler in der Leistungsstatistik. [Offen] G-11: genaue
Erlaubnisregeln der Erweiterung prüfen.

### 7.7 Tests

Golden-Tests: synthetische Encoder-Folgen → erwartete Barrierenliste (läuft gegen den Tracker ohne GPU). Auf
lavapipe zusätzlich Synchronization-Validation (`VK_LAYER_KHRONOS_validation`, syncval) – jede Meldung ist ein
Testfehler.

## 8. Synchronisation, Threading, Speicherverwaltung, Lebensdauer

### 8.1 Gast ↔ Host

* Jede Gast-Queue → Host-Queue mit eigener Timeline-Semaphore `T_q`. `commit` → Submission-Token `v`.
  Fertig ⇔ `T_q ≥ v` (`GpuBackend::completed`).
* **Completion-Pfad:** Ein Completion-Thread wartet mit `vkWaitSemaphores` (any) auf alle Queues, schreibt den
  erreichten Wert in eine **Completion-Seite** im Gastspeicher (pro Queue ein `u64`, vom Gast ohne Trap lesbar) und
  löst einen (zusammengefassten) Interrupt aus. Der Gast-Treiber führt Completion-Handler aus und weckt
  `waitUntilCompleted`.
* **Warten im Gast:** Der Gast soll nie aktiv auf GPU-Arbeit spinnen (jeder Spin kostet emulierte CPU-Zeit):
  zuerst Completion-Seite prüfen, dann schlafen bis zum Interrupt.
* **Fehler:** Ein ungültiger Command Buffer bekommt Status `Error` mit Fehlercode, die Sitzung lebt weiter
  (implementiert, §9.6). Protokollverletzungen (Framing, Version, nicht ausgehandelte Opcodes, doppeltes HELLO)
  → Sitzung wird zurückgesetzt (`SessionReset`, implementiert); `VK_ERROR_DEVICE_LOST` soll denselben Weg nehmen
  (WP-GPU-08). Der Gast sieht einen GPU-Reset (wie ein echter Treiber nach einem Hang).

### 8.2 Threads [Entscheidung]

| Thread | Aufgabe | Kernwahl |
|---|---|---|
| Frontend-Thread pro Gast-Ring | Dekodieren, Validieren, Encoder-Puffer füllen | P-Kerne |
| Submission-Thread pro `VkQueue` | Prolog-Barrieren (§7.2), Wait-before-signal-Halt, `vkQueueSubmit2` | P-Kerne |
| Completion-Thread | Timeline-Warten, Completion-Seiten, Interrupts | beliebig |
| Compile-Pool | air2spirv, Pipeline-Erstellung (§11) | E-Kerne |
| Upload/Transkodier-Pool | ASTC, Dirty-Uploads, IOSurface-Konvertierung | E-Kerne |

vCPU-Threads blockieren nie auf GPU-Arbeit: Sie schreiben in den Ring und klingeln (Doorbell).

### 8.3 Speicherverwaltung

* Allocator im Stil von VMA in Rust: Pools pro Speichertyp, Blöcke 64–256 MiB, Suballokation (TLSF), dedizierte
  Allokationen für große Images (> 32 MiB), importierte und exportierbare Speicher. Externe Crate (z. B.
  `gpu-allocator`) oder eigen [Offen] G-12 – kommt mit `ash` in WP-GPU-04.
* **Budget:** `VK_EXT_memory_budget`. Überschreitung → die Gast-Allokation scheitert (`newBuffer…` liefert `nil`),
  nie stilles Überbuchen (im `NullBackend` als `OutOfDeviceMemory` umgesetzt). Auslagern von Private-Ressourcen in
  Host-Speicher bei Druck [Offen].
* `recommendedMaxWorkingSetSize` = Budget minus Reserve für Ritsus eigene Objekte (Deskriptor-Heaps, Staging,
  Präsentation).

### 8.4 Lebensdauer und Handles

* Jedes Host-Objekt liegt in einer `HandleTable<T>` (generationssicher; Stale-Handles werden erkannt, Slots mit
  erschöpfter Generation werden stillgelegt, Tabellengröße begrenzt).
* Frontends übersetzen **Gast-IDs** (pro Sitzung) in Handles. Zerstört der Gast ein Objekt, ist der Name sofort frei;
  das Objekt selbst lebt weiter, solange (1) ein noch nicht übergebener Command Buffer es referenziert (Skelett:
  `GuestSession` verzögert bis `Commit`) und (2) eine laufende Submission es nutzt (`last_use = (Queue, Token)`;
  Freigabe, wenn `T_q ≥ last_use` – Aufgabe der Vulkan-Backends).
* **Validierung jedes Gast-Werts** (Pflicht vor Vulkan): Handle-Lookup mit Typ; Offsets/Größen mit `checked_*`
  gegen Ressourcengrenzen; Enums über `from_raw` (unbekannt → Fehler, nie Default); Deskriptoren über
  `BufferDesc/TextureDesc::validate`; Encoder-Verschachtelung; Zählerobergrenzen (Attachments, Threads pro
  Threadgroup, Pending-Befehle, Objekte pro Sitzung).

### 8.5 Sicherheit (Q-SEC-1)

* Gast-Shader mit BDA können **jeden** Speicher des `VkDevice` adressieren, nicht nur eigene Buffer. Auf echter
  Apple-Hardware hätte jeder Gast-Prozess einen eigenen GPU-Adressraum. Folgen: (1) Gast-Prozesse können sich
  gegenseitig GPU-Speicher lesen – akzeptiert, weil alles im selben Gast liegt [Offen] G-13; (2) Host-interne
  GPU-Daten (Deskriptor-Heaps, Staging) sind erreichbar → Korruption führt höchstens zu `DEVICE_LOST` (DoS des
  eigenen Gasts), nicht zu Host-Codeausführung. Keine Host-Geheimnisse in GPU-Speicher legen.
* TOCTOU: Der Ring liegt im Gastspeicher. Der Frontend-Thread **kopiert** einen Batch in Host-Speicher, bevor er
  dekodiert (§9.4); validierte Werte werden nie ein zweites Mal aus Gastspeicher gelesen.
* Shader-Eingaben (AIR) sind ebenso feindlich: air2spirv läuft mit Grenzen (Größe, Zeit) und wird gefuzzt
  (shader-compiler.md).

## 9. Frontend G: Protokoll `ritsu-gpu-proto`

### 9.1 Transport [Entscheidung, vorläufig – ADR-0005 ist Proposed]

* virtio-PCI-Gerät. Zwei Möglichkeiten: eigener virtio-Gerätetyp oder **virtio-gpu mit eigenem Context-Type** (wie
  Venus/gfxstream/DRM-native-context in Linux-Gästen). platform.md reserviert ID 16 (virtio-gpu) bisher nur. [Offen]
  G-14.
* **Shared Memory Regions** (virtio 1.2 „shared memory“) für host-sichtbare Bereiche (Apertur, §6.2 b).
* Befehlsringe: ein Single-Producer/Single-Consumer-Ring pro Gast-Queue im Gastspeicher, Kopf mit `head`/`tail`,
  Doorbell per virtqueue-Notify. Ein Batch zwischen zwei Doorbells ist ein **Stream** im Sinne von §9.2. Records
  liegen nie über dem Ringende; davor füllt ein `Nop`-Record auf (`Nop` erlaubt beliebige Payload, auch im
  HELLO-Strom, §9.3).
* Rückkanal: Completion-Seiten (§8.1) + Antwort-Ring für Handshake, Fehler, Command-Buffer-Status und später
  Abfragen. Format des Antwort-Rings: §9.6 (implementiert, Transport noch offen G-14).

### 9.2 Format (implementiert, Version 1)

```
Header (16 B):  magic u32 = "RTSG" | version u16 | flags u16 (=0) | record_count u32 | reserved u32 (=0)
Record:         opcode u16 | flags u16 (=0) | payload_len u32 | payload | Null-Padding auf 8 B
```

Little-Endian, jeder Record ein Vielfaches von 8 Byte. Der Header akzeptiert Versionen im Bereich
`MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION` (derzeit 1..=1); die Sitzung verlangt danach genau die ausgehandelte.
Opcodes (fest, nie wiederverwendet): `0x00xx` Steuerung (`Nop` 0x0001, `Hello` 0x0002),
`0x01xx` Ressourcen, `0x02xx` Render-Encoder, `0x03xx` Compute-Encoder, `0x04xx` Synchronisation, `0x05xx`
Submission. Jeder Opcode hat eine Längenregel (`Exact`/`AtLeast`/`OneOf`). Objekt-ID 0 ist reserviert (null).
Enum-Felder werden roh (Metal-SDK-Werte) übertragen und erst im Host-Modell validiert.

Längenregeln: `Exact(n)`, `AtLeast(n)` (variabler Rest, Zählfelder im Kopf müssen den Rest exakt erklären) und
`OneOf(a, b)` (Basis-Layout oder Basis + optionale Erweiterung, z. B. `CreateTexture` mit Swizzle). Variable Teile
(Attachment-Listen, Vertex-Attribute/-Layouts) dekodiert `RecordList` zero-copy: Einträge fester Größe, jeder
vollständig geprüft, indizierte Einträge streng aufsteigend und unter ihrer Grenze; ein dekodiertes `Command` bleibt
dadurch ≤ 256 B (Test `decoded_commands_stay_small`), der Speicherbedarf wächst nie mit Gast-Zählern.

#### 9.2.1 Befehle (v1, Stand WP-GPU-24)

Alle IDs sind Gast-IDs (0 = reserviert; wo Metal `nil` erlaubt, bedeutet 0 `nil`). Enum-Felder tragen Metal-SDK-Rohwerte
(validiert im Host-Modell); Protokoll-eigene Felder (Flags, Attachment-Art, Listenstruktur) prüft schon der Decoder.

| Opcode | Name | Klasse | Länge | Payload (LE) |
|---|---|---|---|---|
| 0x0001 | `Nop` | Control | ≥ 0 | beliebig |
| 0x0002 | `Hello` | Control | 16 | §9.3 (eingefroren) |
| 0x0100 | `CreateBuffer` | Resource | 16 | `id u32, storage u8, cpu_cache u8, hazard u8, res u8, length u64` |
| 0x0101 | `DestroyBuffer` | Resource | 4 | `id u32` |
| 0x0102 | `WriteBuffer` | Resource | ≥ 16 | `id u32, res u32, offset u64, data` |
| 0x0103 | `CopyBuffer` | Encoder | 32 | `src u32, dst u32, src_off u64, dst_off u64, size u64` |
| 0x0110 | `CreateTexture` | Resource | 32 oder 40 | `id u32, format u16, usage u16, type u8, storage u8, samples u8, res u8, width, height, depth, array_length, mip_levels (u32)`; Erweiterung: `swizzle [u8;4] (R,G,B,A), res u32` – der Encoder sendet sie nur bei Nicht-Identität |
| 0x0111 | `DestroyTexture` | Resource | 4 | `id u32` (Texturen und Views) |
| 0x0112 | `CreateTextureView` | Resource | 32 | `id u32, parent u32, format u16, type u8, res u8, swizzle [u8;4], level_start, level_count, slice_start, slice_count (u32)` |
| 0x0120 | `CreateSampler` | Resource | 32 | `id u32, min u8, mag u8, mip u8, res u8, s u8, t u8, r u8, border u8, reduction u8, compare u8, flags u8 (1 normalized, 2 lodAverage, 4 argumentBuffers), res u8, max_anisotropy u32, lod_min f32, lod_max f32, lod_bias f32` |
| 0x0121 | `DestroySampler` | Resource | 4 | `id u32` |
| 0x0130 | `CreateDepthStencilState` | Resource | 40 | `id u32, depth_compare u8, flags u8 (1 depthWrite, 2 front, 4 back), res u16`, je Seite 16 B `compare, sfail, dfail, pass (u8), read_mask u32, write_mask u32, res u32` (fehlende Seite = 16 Null-Bytes) |
| 0x0131 | `DestroyDepthStencilState` | Resource | 4 | `id u32` |
| 0x0140 | `CreateRenderPipelineState` | Resource | ≥ 32 | Kopf `id u32, vertex_fn u32, fragment_fn u32, depth_fmt u16, stencil_fmt u16, samples u8, topology u8, flags u8 (1 alphaToCoverage, 2 alphaToOne, 4 rasterization, 8 vertexDescriptor), color_count u8 (≤ 8), amplification u32, attr_count u8, layout_count u8 (≤ 31), res 6 B`; dann `color_count` × 16 B (`format u16, blend u8 (Bit 0 = blendingEnabled), write_mask u8, src_rgb u8, dst_rgb u8, rgb_op u8, src_alpha u8, dst_alpha u8, alpha_op u8, res 6 B` – genau diese Reihenfolge; Faktoren `MTLBlendFactor`, Operationen `MTLBlendOperation`; dicht ab Slot 0), `attr_count` × 16 B (`index u8, res u8, format u16, buffer_index u32, offset u64`), `layout_count` × 24 B (`index u8, step_fn u8, res 6 B, stride u64 (u64::MAX = dynamisch), step_rate u64`; fehlende Layouts = SDK-Default) |
| 0x0141 | `DestroyRenderPipelineState` | Resource | 4 | `id u32` |
| 0x0200 | `BeginRenderPass` | Encoder | ≥ 16 | Kompaktform: `width, height, count (u32), res u32`, `count` × 24 B (`tex u32, load u8, store u8, res u16, clear f32×4`); Farbe dicht, Level/Slice 0, keine Resolve-Textur |
| 0x0201 | `EndRenderPass` | Encoder | 0 | – |
| 0x0202 | `Draw` | Encoder | 20 | `primitive u8, res 3 B, vertex_start, vertex_count, instance_count, base_instance (u32)` |
| 0x0203 | `BeginRenderPass2` | Encoder | ≥ 24 | Kopf `width, height, array_length, default_samples (u32), color_count u8, flags u8 (1 Tiefe, 2 Stencil), res 6 B`; Attachment-Kern 40 B `tex u32, load u8, store u8, extra u8, res u8, store_options u32, level u32, slice u32, depth_plane u32, resolve_tex u32 (0 = keine, dann Level/Slice/Ebene = 0), resolve_level, resolve_slice, resolve_depth_plane (u32)`; Farbe 72 B = Kern (extra = Slot, streng aufsteigend, < 8) + `clear f64×4`; Tiefe 48 B = Kern (extra = Depth-Resolve-Filter) + `clear_depth f64`; Stencil 48 B = Kern (extra = Stencil-Resolve-Filter) + `clear_stencil u32, res u32` |
| 0x0204 | `SetStoreAction` | Encoder | 8 | `kind u8 (0 Farbe, 1 Tiefe, 2 Stencil), index u8 (Farbe < 8, sonst 0), store_action u8, res 5 B` |
| 0x0210 | `SetRenderPipelineState` | Encoder | 4 | `id u32` |
| 0x0211 | `SetDepthStencilState` | Encoder | 4 | `id u32` (0 = `nil`, Default-Zustand) |
| 0x0212 | `SetVertexBuffer` | Encoder | 24 | `id u32 (0 = nil), index u32, offset u64, attribute_stride u64 (u64::MAX = statisch)` |
| 0x0213 / 0x0214 | `SetVertexTexture` / `SetFragmentTexture` | Encoder | 8 | `id u32 (0 = nil), index u32` |
| 0x0215 / 0x0216 | `SetVertexSamplerState` / `SetFragmentSamplerState` | Encoder | 8 | `id u32 (0 = nil), index u32` |
| 0x0300–0x0302 | `BeginComputePass`, `EndComputePass`, `Dispatch` | Encoder | 0/0/24 | `Dispatch`: `threadgroups u32×3, threads_per_threadgroup u32×3` |
| 0x0400 / 0x0401 | `SignalEvent` / `WaitEvent` | Sync | 16 | `id u32, res u32, value u64`; nur mit `SHARED_EVENTS` |
| 0x0500 | `Commit` | Submission | 8 | `submission_id u64` |

**Vorwärtskompatibilität der neuen Befehle** [Entscheidung]: Sie gehören zur Basis von v1 (kein Feature-Bit), weil
v1 noch nicht ausgeliefert ist (ADR-0005 `Proposed`, Änderungen bis M4 erlaubt); alte Ströme bleiben
**dekodierbar** (`CreateTexture` mit 32 B, `BeginRenderPass` als Kompaktform), aber nicht jede alte Befehlsfolge
bleibt semantisch gültig: Ein `Draw` ohne vorheriges `SetRenderPipelineState` ist seit WP-GPU-24 ein Gast-Fehler
(`InvalidCommand`, Command Buffer vergiftet). Die Fuzz-Seeds `fuzz_gpu_host_session/render_pass` (zeichnet ohne
Pipeline) und `fuzz_gpu_proto_decode/all_opcodes` (deckt die neuen Opcodes nicht ab) bleiben gültige Eingaben,
decken aber weniger ab; ihre Erneuerung ist Folge-WP (WP-QA-10). **Shader-Funktionen** in `CreateRenderPipelineState`
sind Platzhalter: `vertex_fn`/`fragment_fn` sind für Gast-IDs von Funktionsobjekten reserviert. Bis WP-SC-09/12 die
Bibliotheks-/Funktions-Opcodes (vorgesehen: `CreateLibrary`/`CreateFunction` im Bereich `0x015x`, Auflösung der IDs
im Host, Pflicht einer Vertex-Funktion) ergänzen, akzeptiert der Host nur 0; jeder andere Wert → `Unimplemented`.
Das Layout ändert sich dadurch nicht. Pipelines ohne Funktionen kann nur der `NullBackend` „ausführen“ (er führt nichts
aus); ein Vulkan-Backend muss Draws mit ihnen als `Unimplemented` ablehnen, statt ohne Shader zu zeichnen.

Fehler, die der Decoder erkennt (`ProtoError`): zu kurzer Header, falsche Magic, falsche Version, gesetzte
Reserve-Bits/Flags/Padding, Record-Anzahl größer als möglich, abgeschnittener Record-Kopf/Payload, Payload zu
groß (> 16 MiB), unbekannter Opcode, falsche Payload-Länge, ungültige Felder (ID 0, inkonsistente Attachment-Zahl,
unbekannte Flag-Bits, Listen nicht streng aufsteigend, Resolve-Subressource ohne Resolve-Textur),
überzählige Bytes; sitzungsbezogen (§9.3): `HelloRequired`, `HelloNotAlone`, `UnexpectedHello`, `VersionMismatch`,
`FeatureNotNegotiated`. `ProtoError::reply_code()`/`offset()`/`raw_opcode()` liefern die Felder für `SessionReset`. Kein Fall führt zu Panic oder ungeprüftem Index; Speicherbedarf wächst nur mit tatsächlich
dekodierten Records (nicht mit der angegebenen Anzahl). Tests: alle Präfixe eines gültigen Streams, alle
Einzelbit-Flips, 20 000 Pseudozufalls-Streams; pro neuem Opcode zusätzlich jede Payload-Kürzung mit passender
Längenangabe, jedes Einzelbit (akzeptierte Payloads müssen einen Fixpunkt `encode(decode(x))` ergeben) und die
Feldregeln.

### 9.3 Versionierung und Feature-Aushandlung (implementiert, WP-GPU-01)

* **Bootstrap** [Entscheidung]: Rahmenformat (Stream- und Record-Header) sowie die Payloads von `Hello`,
  `HelloAck` und `HelloReject` sind über **alle** Protokollversionen eingefroren. Der HELLO-Strom trägt immer
  Header-Version `BOOTSTRAP_VERSION` (= 1), damit jeder Host jedes HELLO lesen kann. Der Header-Parser akzeptiert
  `BOOTSTRAP_VERSION` deshalb **immer**, auch wenn sie einmal unter `MIN_PROTOCOL_VERSION` fällt
  (`version_parseable`, Compile-Zeit-Assert + Test); das gilt ebenso für den Bootstrap-Antwortstrom.
* Erster Strom einer Sitzung: genau **ein** Record `Hello` (16 B):
  `min_version u16 | max_version u16 | reserved u32 | features u64`, davor und danach beliebig viele
  `Nop`-Records (Ring-Auffüllung, §9.1). Draht-Prüfung: `min_version ≥ 1`, `max_version ≥ min_version`.
* Antworten vor dem Handshake (es gibt nichts zurückzusetzen, der Host wartet weiter auf HELLO):
  * `SessionReset` `Malformed`: Framing- oder Drahtfehler (auch ungültiges `Hello`-Feld) oder ein weiterer
    Nicht-`Nop`-Record nach dem HELLO (`HelloNotAlone`);
  * `SessionReset` `UnsupportedVersion`: Header-Version ≠ `BOOTSTRAP_VERSION`;
  * `SessionReset` `HelloRequired`: ein anderer Record als `Nop` vor dem HELLO oder gar kein HELLO;
  * `HelloReject`: gültiges HELLO ohne gemeinsame Version.
* `negotiate`: Version = höchste Version in Gast-Bereich ∩ `MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION`; leer →
  `HelloReject {host_min_version, host_max_version}` (Sitzung bleibt offen für ein neues HELLO). Features =
  Gast-Angebot ∩ Host-Angebot ∩ `Features::KNOWN`. Das Host-Angebot (`frontend_g::HOST_FEATURES`) enthält nur, was
  der Host **implementiert** (niemals still falsch); unbekannte Bits werden nie bestätigt.
* Antwort `HelloAck`: gewählte Version, aktivierte Features, **Fähigkeitsblock** (`CapabilityBlock`, 48 B:
  Familien-Bits Apple7/Mac2/Common3/Metal3/Metal4, Argument-Buffer-Tier, zehn `SupportLevel`s
  {0 = nicht gemeldet, 1 = nativ, 2 = emuliert}, Grenzen aus `ReportedFeatures::limits`). Der Gast-Treiber meldet
  Metal-Clients genau diese Menge.
* **Vorwärtskompatibilität des eingefrorenen `HelloAck`** [Entscheidung]: Der Host sendet nur Werte, die in der
  ausgehandelten Version bzw. mit den ausgehandelten Features definiert sind: Familien-Bits, `SupportLevel`s,
  `ErrorCode`s und Antwort-Opcodes (`EventSignaled` nur mit `SHARED_EVENTS`). Der Referenz-Decoder des Gasts
  ignoriert zusätzlich unbekannte Familien-Bits, damit ein neuerer Host Familien ergänzen kann, ohne ältere Gäste
  zu brechen. Andere unbekannte Werte bleiben Decoder-Fehler (sie dürften nach der Regel nie gesendet werden).
* Feature-Bits v1: Bit 0 `SHARED_EVENTS` (Opcodes `SignalEvent`/`WaitEvent`, Antwort `EventSignaled`). Der Host
  bietet es noch nicht an (WP-GPU-08). Ein Gating-Opcode ohne ausgehandeltes Feature gilt wie ein unbekannter
  Opcode → Protokollverletzung (`FeatureNotNegotiated`). Record-Flags bleiben reserviert, bis ein Feature-Bit sie
  definiert. *Hinweis:* `SignalEvent`/`WaitEvent` waren in M0 ungegatet; v1 ist noch nicht ausgeliefert
  (kein Gast-Treiber), daher ohne Versionssprung geändert.
* Nach dem Handshake: Jeder Strom trägt die ausgehandelte Version (`VersionMismatch` sonst); ein weiteres `Hello`
  ist eine Protokollverletzung (`DuplicateHello`) und setzt die Sitzung zurück. Ein Treiber-Neustart im Gast
  erhält so einen sauberen Host-Zustand und verhandelt neu.
* Inkompatible Änderungen → neue Hauptversion; der Host unterstützt mindestens die aktuelle und die vorige.

### 9.4 Validierung und Datenfluss

1. Batch aus dem Ring in einen Host-Puffer kopieren (TOCTOU).
2. `decode_hello_stream` (vor dem Handshake) bzw. `decode_session_stream` (danach): Framing, Drahtformat,
   Version, Feature-Gating. Bei Fehler: nichts ausführen, Sitzung zurücksetzen (`SessionReset`).
3. `GuestSession::execute_stream`: semantische Prüfung (Objektmodell, Handles, Limits) und Ausführung, Record für
   Record. Semantische Fehler betreffen nur den Record (`RecordError`) und, falls er zum aufgezeichneten Command
   Buffer gehört, diesen Command Buffer (§9.6). Die Ausführung geht mit dem nächsten Record weiter.

### 9.5 Zero-Copy und Uploads

* `WriteBuffer` mit Inline-Daten nur für kleine Daten (Richtwert < 4 KiB).
* Große Daten: Der Gast-Treiber legt sie in Seiten, die er per Seitenliste/Blob-ID referenziert; der Host kopiert
  von dort direkt in Staging/VRAM (oder importiert sie, §6.2 a). Für Shared-Buffer entfällt der Upload ganz.
* Shader: `metallib`-Blobs werden per Hash angemeldet (`QueryLibrary(hash)` → vorhanden/benötigt); nur bei
  Cache-Miss werden Daten übertragen.

### 9.6 Antwortkanal (Host → Gast, implementiert, WP-GPU-01)

Gleiches Rahmenformat wie §9.2, aber Magic `"RTSR"` und eigene Opcodes (`0x80xx`, fest, nie wiederverwendet).
Alle Payloads haben feste Länge. Jeder an `execute_stream` übergebene Strom erhält eine Sequenznummer
(`stream_seq`, ab 0 pro Sitzung, auch über Resets hinweg fortlaufend); Gast und Host zählen beide mit.

| Opcode | Antwort | Payload | Bedeutung |
|---|---|---|---|
| 0x8001 | `HelloAck` | 64 B | §9.3 (eingefroren) |
| 0x8002 | `HelloReject` | 8 B | keine gemeinsame Version (eingefroren) |
| 0x8003 | `SessionReset` | 24 B: `stream_seq u64, offset u64, opcode u16, code u16, reserved u32` | Strom als Ganzes abgelehnt, nichts davon ausgeführt; alle Objekte zerstört, offene Arbeit verworfen, keine weiteren Status für ältere Submissions; Gast muss neu HELLO senden |
| 0x8004 | `RecordError` | 32 B: `stream_seq u64, record_index u32, opcode u16, code u16, object_id u32, reserved u32, detail u64` | ein Record nicht ausgeführt, Strom lief weiter |
| 0x8005 | `CommandBufferStatus` | 16 B: `submission_id u64, status u8 (0 ok/1 Fehler), reserved u8, code u16, reserved u32` | genau einer pro `Commit`, in Commit-Reihenfolge |
| 0x8006 | `EventSignaled` | 16 B: `event_id u32, reserved u32, value u64` | nur mit `SHARED_EVENTS` (derzeit nie gesendet) |

`ErrorCode` (u16): 1 `Malformed`, 2 `HelloRequired`, 3 `DuplicateHello`, 4 `UnsupportedVersion`,
5 `FeatureNotNegotiated`, 6 `UnknownObject`, 7 `DuplicateObject`, 8 `InvalidArgument`, 9 `InvalidCommand`,
10 `OutOfMemory` (`detail` = angeforderte Bytes), 11 `LimitExceeded`, 12 `Unimplemented`, 13 `HostError`,
14 `TooManyErrors` (`detail` = Zahl unterdrückter Fehler).

**Regeln** [Entscheidung]:

* **Poisoned Command Buffer.** Jeder Opcode trägt eine Pflicht-Klasse (`OpcodeClass`: `Control`, `Resource`,
  `Encoder`, `Sync`, `Submission`; Spalte im Opcode-Makro, ein neuer Opcode kompiliert nicht ohne sie). Scheitert
  ein Record der Klassen `Encoder` oder `Sync` (heute `CopyBuffer`, Render-/Compute-Encoder-Befehle inkl.
  `BeginRenderPass2`, `SetStoreAction` und aller Binds, `SignalEvent`/`WaitEvent`) oder einer künftigen, dem Frontend
  unbekannten Klasse, wird der Command Buffer
  markiert. Ein Befehl, den das Frontend gar nicht ausführen kann, markiert ihn unabhängig von der Klasse (im
  Zweifel vergiften) –
  **auch über Stromgrenzen hinweg**. Folgende Encoder-Befehle werden bis zum `Commit` ohne weitere Meldung
  übersprungen; der `Commit` reicht nichts ein, meldet `CommandBufferStatus = Error(Code des ersten Fehlers)`,
  verwirft die offenen Befehle und führt verzögerte Destroys trotzdem aus. Ein Teil-Command-Buffer wird nie als
  erfolgreich ausgeführt (Review-Befund WP-M0-REV, Regressionstest
  `regression_failed_record_poisons_command_buffer_across_streams`). Ressourcenbefehle (`Create*`, `Destroy*`,
  `WriteBuffer`) sind Gerätebefehle und vergiften keinen Command Buffer. Seit WP-GPU-24 prüft das Frontend die
  Encoder-Reihenfolge schon bei der Aufzeichnung (`EncoderValidator`): ein Bind oder Draw außerhalb eines Passes ist
  ein `RecordError` `InvalidCommand` genau dieses Records (Test `bind_outside_a_render_pass_poisons`); nur ein beim
  `Commit` noch offener Encoder fällt erst dort auf (Status `Error(InvalidCommand)`).
* **Completion.** Erfolgreich eingereichte Command Buffer melden `Completed`, sobald die Backend-Timeline sie
  erreicht (`GuestSession::poll_completions`, später vom Completion-Thread angestoßen, §8.1); fehlgeschlagene
  warten in derselben Warteschlange. Die Status kommen damit **ausnahmslos** in Commit-Reihenfolge.
* **Protokollregel `MAX_OUTSTANDING_SUBMISSIONS`** (65 536, Konstante in `ritsu-gpu-proto`): So viele committete
  Command Buffer darf der Gast höchstens haben, deren Status er noch nicht erhalten hat. Der Host prüft vor der
  Ausführung eines Stroms, ob dessen `Commit`s die Grenze überschreiten würden; wenn ja, ist das eine
  Protokollverletzung (`SessionReset` `LimitExceeded`, nichts vom Strom ausgeführt). Ein Gast-Treiber blockiert
  wie ein echter Treiber vorher. So kann die Status-Warteschlange nie überlaufen und die Reihenfolge bleibt erhalten.
* **Grenzen.** Höchstens `MAX_ERRORS_PER_STREAM` (64) `RecordError`s pro Strom, danach ein `TooManyErrors`;
  höchstens `MAX_PENDING_COMMANDS` aufgezeichnete Befehle (darüber `LimitExceeded`, vergiftet den Command
  Buffer); höchstens `MAX_OBJECTS_PER_KIND` Objekte pro Art (Buffer, Texturen inkl. Views, Sampler,
  Depth/Stencil-States, Render-Pipelines), gezählt werden lebende **und** auf verzögertes Zerstören wartende Objekte
  (darüber `LimitExceeded`). Bei Texturen zählen zusätzlich Eltern-Texturen, deren Gast-Name zerstört ist, die
  aber Views noch am Leben halten (Metal: eine View hält ihre Eltern-Textur); das Frontend spiegelt dazu
  View → Eltern und gibt die Kette frei, sobald die letzte View fällt (Regressionstest
  `regression_view_chain_with_one_name_is_bounded`). Ein volles Handle-Table im Backend
  (`HandleError::Full`) erreicht den Gast ebenfalls als `LimitExceeded`. Pipelines hält das Frontend nur als `PipelineTargets` (wenige Dutzend Byte statt ≈ 1,7 KiB), damit die Grenze auch
  den Host-Speicher begrenzt. Die Antwortmenge pro Strom ist durch die Stromgröße
  beschränkt, nie durch eine Gast-Zahl.
* Fuzz-Ziel für die Gegenseite: `decode_reply_stream` (Referenzdecoder für den Gast-Treiber).

### 9.7 Noch fehlende Befehlsklassen (Folge-WPs)

Seit WP-GPU-24 ist alles, was das Objektmodell (WP-GPU-02) validiert, auf dem Draht ausdrückbar (§9.2.1). Es fehlen:
Bibliotheken/Funktionen (WP-SC-09/12, Platzhalter in `CreateRenderPipelineState`), Compute-Pipelines, Argument-Buffer,
Heaps, Fences/Events (derzeit `Unimplemented`), `setFragmentBuffer:`/`setVertexBytes:` und weiterer dynamischer
Zustand (Viewport, Scissor, Cull, Winding, Depth-Bias, Stencil-Referenz, Blend-Farbe), `setColorStoreActionOptions:`,
indizierte/indirekte Draws, Blit-Texturbefehle, Queries/Zähler, IOSurfaces, Anzeige. Erst beim Draw prüfbar und noch
offen: Sampler-Textur-Paarungen (4:2:2 braucht `clampToEdge`, `A1BGR5Unorm`/`ABGR4Unorm` nicht mit `opaqueBlack`,
Vergleichs-Sampler mit Pixelkoordinaten) – braucht die Shader-Reflexion aus WP-SC-09/12.

## 10. Frontend P: Apples Paravirtual-GPU

**Öffentlich bekannt** [öffentlich]: Virtualization.framework bietet für macOS-Gäste ein Grafikgerät
(`VZMacGraphicsDeviceConfiguration` mit Anzeigen und Auflösungen). macOS-Gäste bringen den passenden Treiber mit. Die
Host-Seite steckt im privaten Framework `ParavirtualizedGraphics`, das auf dem Host mit Metal rendert. QEMU enthält
seit Version 10 [Annahme: Versionsnummer prüfen] ein Gerät `apple-gfx` (MMIO-Variante für `vmapple`, PCI-Variante
für x86-Macs), das dieses Framework auf macOS-Hosts **benutzt** – das zeigt die äußere Form (MMIO-Fenster,
Gast-Speicherzuordnungen, Anzeigemodi, Cursor), aber nicht das Befehlsformat, das im Framework verborgen bleibt.

**Alles Weitere ist [Spec nötig]** (Spike S-GPU-1, Rolle `reverse-engineer`, nur Spezifikationen in
`docs/specs/gpu-paravirt/`): Registerlayout, Initialisierung, Speicherzuordnung, Befehls- und Antwortformat,
Ressourcen-Lebenszyklus, Anzeige/Cursor, gemeldete Metal-Familien und – **entscheidend** – die Form der Shader:
Wenn der Gast Apple-GPU-Maschinencode statt AIR (o. ä.) schickt, ist Variante P für Ritsu praktisch nicht
umsetzbar. Implementiert wird Frontend P als Geschwister von `frontend_g` gegen dasselbe Modell und denselben
`GpuBackend`.

## 11. Caches und Kompilier-Stottern

* **SPIR-V-Cache** (ADR-0007): Schlüssel = (AIR-Modul-Hash, Funktionsname, Function-Constants,
  shader-relevanter Pipeline-Zustand: Vertex-Deskriptor bei Vertex-Pulling, Attachment-Formate, Sample-Anzahl,
  Framebuffer-Fetch-Nutzung, Ziel-Fähigkeitsprofil; air2spirv-Version). Inhaltsadressiert auf Platte, pro
  macOS-Build getrennt, LRU-Grenze.
* **`VkPipelineCache` pro (Vendor, Device-ID, Treiberversion, `pipelineCacheUUID`)**, Header vor dem Laden geprüft;
  `VK_KHR_pipeline_binary`, wo vorhanden.
* **1 Metal-PSO ≈ 1 `VkPipeline`** [Annahme, gute Nachricht]: Metal backt Formate, Blending, Vertex-Deskriptor und
  Sample-Anzahl in den PSO – genau den Zustand, den Vulkan backt. Alles, was Metal pro Draw setzt, ist mit Core 1.3 +
  EDS3 dynamisch. Variantenexplosion wie bei D3D9/OpenGL-Übersetzern entsteht kaum.
* **Stotter-Strategie:** (1) `newRenderPipelineState…` (synchron) blockiert auch in Metal → Gast-Thread darf warten,
  aber mit Vorrang im Compile-Pool. (2) Mit `graphics_pipeline_library`: Teil-Bibliotheken sofort bauen, schnell
  linken, optimierten Link im Hintergrund, später austauschen. (3) Asynchrone Erstellung (`…completionHandler:`)
  direkt in den Pool. (4) **Replay-Datenbank** pro macOS-Build: Ritsu speichert alle gesehenen PSO-Deskriptoren
  und kompiliert sie beim nächsten Boot vor (WindowServer- und Core-Animation-PSOs sind dann vor dem ersten Frame
  warm). `MTLBinaryArchive`-Inhalte dienen als zusätzliche Deskriptorliste.

## 12. Präsentation auf dem G5 MF (kein MUX)

Internes Panel → Iris Xe (eDP). Externe Anschlüsse hängen beim G5 MF vermutlich an der RTX [Annahme].

| Modus | Weg |
|---|---|
| **Entwicklung** (Host mit Wayland/X) | `VK_KHR_swapchain` auf der Oberfläche; NVIDIA-PRIME-Render-Offload, der Treiber kopiert über dma-buf zur Iris. |
| **Appliance** (kein Compositor, Ziel) | RTX rendert → finaler **Present-Pass** (Formatwandlung, ggf. EDR-Tonemapping, Skalierung) in ein exportierbares, **lineares** Image im Systemspeicher → dma-buf (`external_memory_dma_buf` + `image_drm_format_modifier`) → Import in die Iris-KMS als Framebuffer (`drmModeAddFB2WithModifiers`, LINEAR) → atomarer Page-Flip mit `IN_FENCE_FD` (Sync-File aus der Vulkan-Semaphore). Keine CPU-Kopie, kein CPU-Warten. Bandbreite: 1920×1080×4 B × 144 Hz ≈ 1,2 GB/s über PCIe – vertretbar [Annahme: Panel 1920×1080 bei 144 Hz, prüfen]. |
| **Appliance, Iris rendert** (Akku) | Rendern direkt in ein scanout-fähiges Image der Iris, `VK_KHR_display`/KMS-Flip ohne Kopie. |
| Rückfall | Iris-Vulkan importiert das dma-buf und kopiert es in eine `VK_KHR_display`-Swapchain. |

**Frame-Pacing:** Page-Flip-Events mit Vblank-Zeitstempeln → virtuelle VSync-Interrupts für den Gast (CVDisplayLink).
Dreifachpuffer im Present-Pfad; VRR später. **Cursor:** KMS-Cursor-Plane der Iris (spart Latenz). **Budget
Q-PERF-4** (< 16 ms Gast-Present → Scanout): Present-Pass ≤ 1 ms, dma-buf-Übergabe ≈ 0, Warten auf Vblank ≤ 1 Frame.
Offene Punkte: Scanout-Fähigkeit linearer Fremd-Buffer auf der Iris-Display-Engine (Stride-Ausrichtung) [Annahme],
Verhalten des proprietären NVIDIA-Treibers beim Export linearer Images [Annahme], EDR/HDR auf einem SDR-Panel (G-6).

## 13. Tests

| Ebene | Was | Wo |
|---|---|---|
| Einheit | Decoder, Handles, Modell-Validierung, Fähigkeits-Policy, `NullBackend`, `GuestSession` (vorhanden) | `cargo test -p ritsu-gpu-proto -p ritsu-gpu-host` |
| Fuzzing | `decode_stream`, `decode_hello_stream`, `decode_session_stream`, `decode_reply_stream` (alle panikfrei); strukturiertes Fuzzing `GuestSession<NullBackend>` (erst HELLO senden; Invariante: jede `StreamOutcome::reply_stream()` dekodiert fehlerfrei); später Differenz „NullBackend und Vulkan-Backend lehnen dieselben Eingaben ab“ | `fuzz/` (test-engineer) |
| Hazard | Golden-Barrierenlisten (§7.7) | ohne GPU |
| Vulkan in CI | **lavapipe** (Mesa-Software-Vulkan, 1.3; Mesh/Ray Query je nach Mesa-Version [Annahme]) mit Validation-Layern inkl. Synchronization-Validation und GPU-AV; jede Meldung = Testfehler | CI |
| Bildvergleich | eigene Testszenen (Metal-Quelltext, einmalig auf einem Mac zu AIR kompiliert, ADR-0007) + **Referenzbilder von echter Apple-Hardware**; Toleranz pro Test (UNORM-Ziele ±1 LSB, Filterung/Transzendente mit PSNR-Schwelle); lavapipe-Goldens zusätzlich als Regressionsschutz | `tests/gpu/` (braucht O-2: Mac) |
| Hardware | dieselben Szenen auf RTX 4050 und Iris Xe | self-hosted Runner auf dem G5 MF (O-3), sonst „nicht ausführbar hier“ |
| Leistung | Mikro-Benchmarks (Draw-Call-Durchsatz, Barrieren pro Frame, Upload-Bandbreite, Compile-Latenz) | `bench/` |

## 14. Work Packages

★ = Hochrisiko (zuerst als Spike). Abhängigkeiten außerhalb des GPU-Pfads in Klammern.

| WP | Titel | Abhängig von | Akzeptanz |
|---|---|---|---|
| WP-GPU-01 | Protokoll v1 härten: Fuzz-Target `decode_stream` + `GuestSession<NullBackend>`, HELLO/Feature-Aushandlung, Antwortkanal | M0-GPU (test-engineer: `fuzz/`) | 24 h Fuzzing ohne Befund; Aushandlung mit Unit-Tests |
| WP-GPU-02 | Objektmodell vollständig: alle `MTLPixelFormat`-Werte gegen SDK-Header geprüft, Sampler, Depth/Stencil, Resolve, Swizzle | M0-GPU | Tabellentests Format → Blockgröße/Fähigkeit; keine `[Annahme]` mehr bei Rohwerten |
| WP-GPU-03 ★ | Vulkan-Bring-up mit `ash`: Instanz, Gerät, Fähigkeitsabfrage → `Capabilities` vom echten Gerät; lavapipe in CI; `vulkaninfo`-Dumps G5 MF unter `docs/compat/` | 02 | `missing_baseline()` leer auf lavapipe/RTX/Iris; Abweichungen zu `*_assumed()` dokumentiert und korrigiert |
| WP-GPU-04 | Speicher-Allocator, Budget, verzögerte Freigabe (Timeline), `HandleTable` im echten Backend | 03 | Allokations-Stresstest ohne Validation-Meldung; Budgetfehler statt OOM |
| WP-GPU-05 | Texturen, Formattabelle Metal→Vulkan, Uploads/Readback, ASTC-Dekoder (CPU) | 03, 04 | Bildvergleich Upload→Readback bit-genau; ASTC-Dekoder gegen Referenzdekoder bit-genau |
| WP-GPU-06 ★ | `HazardTracker` + Barrierenerzeugung + Prolog-Auflösung pro Queue | 04 | Golden-Barrierentests; syncval auf lavapipe ohne Meldung für alle Testszenen |
| WP-GPU-07 | Render-Encoder (dynamic rendering, dynamischer Zustand), PSO aus SPIR-V | 06 (shader-compiler: AIR→SPIR-V für Vertex/Fragment) | Testszenen „Clear“, „Dreieck“, „Blending“ auf lavapipe = Referenz |
| WP-GPU-08 | Queues, Submission-/Completion-Threads, Events, Shared Events, Completion-Seiten | 04 | Ordnungstests mit mehreren Queues; Wait-before-signal ohne Deadlock |
| WP-GPU-09 | Compute- und Blit-Encoder (Serial/Concurrent, große Gitter aufteilen, Mipmaps) | 06 | Compute-Testszenen bit-genau |
| WP-GPU-10 ★ | **M4 „Erstes Dreieck“**: Frontend (P oder G laut Spike) → Kern → RTX 4050 **und** Iris Xe | 05–09 (S-GPU-1/2, ADR-0005 entschieden, M3) | Gast-Test-App rendert das Dreieck; Bildvergleich gegen Apple-Referenz auf beiden GPUs |
| WP-GPU-11 ★ | Unified Memory: Import von Gastspeicher, memfd-Alias, Dirty-Tracking-Schnittstelle zu `ritsu-mem` | 04 (architect: ADR für `ritsu-mem`-Schnittstelle) | Kohärenztests CPU↔GPU auf beiden GPUs; Messung der Strategien a/b/c |
| WP-GPU-12 | Präsentation: Present-Pass, dma-buf, KMS-Flip auf Iris, Frame-Pacing, Cursor-Plane | 03 (host-os: KMS-Zugriff) | Testbild auf dem internen Panel im Appliance-Modus; Latenz gemessen (Q-PERF-4) |
| WP-GPU-13 | Argument Buffers Tier 2: Deskriptor-Heaps, `MTLResourceID`, BDA, `useResource` | 07 (shader-compiler: Tier-2-Lowering) | Bindless-Testszenen; stale IDs lesen 0 |
| WP-GPU-14 | Shader-/Pipeline-Caches, GPL, Compile-Pool, Replay-Datenbank | 07 | zweiter Start ohne Kompilierung; Stotter-Messung |
| WP-GPU-15 ★ | IOSurface-Modell, Lock/Unlock, Formatgrenzen (EDR) | 05, 11 | Prozessübergreifende Surface-Tests im Gast |
| WP-GPU-16 ★ | **M5 „Beschleunigter WindowServer“**: Desktop komponiert über Metal→Vulkan, Anzeige per §12 | 10–15 (M5 aller Subsysteme) | macOS-Desktop auf dem internen Panel, Fensterbewegung ohne Fehlbilder, Frame-Zeit gemessen |
| WP-GPU-17 | Heaps, Aliasing, Residency-Sets, `MTLFence` | 06 | Fence-/Aliasing-Testszenen; syncval sauber |
| WP-GPU-18 ★ | TBDR-Emulation: Framebuffer Fetch, ROG, Tile-Shader/Imageblocks | 07, 13 | Testszenen (deferred shading mit Imageblocks) = Apple-Referenz; dann Policy-Schalter an |
| WP-GPU-19 | Indirect Command Buffers (CPU-Replay, DGC, Readback-Rückfall) | 13 | ICB-Testszenen auf beiden GPUs |
| WP-GPU-20 | Mesh-Shader (RTX nativ, Iris emuliert) | 13 | Testszenen; Metal3-Meldung erst danach |
| WP-GPU-21 ★ | Ray Tracing (RTX nativ; Iris Compute-Emulation) | 13 | Testszenen; Leistungsmessung Iris |
| WP-GPU-22 | Metal-4-API (Allocator, Argument-Tables, Barrieren, Residency-Sets, Compiler) | 06, 13 | Metal-4-Testszenen |
| WP-GPU-23 | Zähler/Zeitstempel, MetalFX-Spike und Rückfall | 08 | `GPUStartTime/EndTime` plausibel; MetalFX-Spike-Bericht |

M7 („Metal-Breite“ in der Roadmap) = WP-GPU-13, 17–22.

## 15. Offene Fragen

| ID | Frage | Wer entscheidet | Bis |
|---|---|---|---|
| G-1 | Setzen WindowServer/Core Animation/Core Image in macOS 27 eine **Apple-GPU-Familie** voraus (Tile-Shader, Memoryless, Framebuffer Fetch) oder laufen sie mit Mac2/Metal3? Davon hängt ab, ob WP-GPU-18 vor M5 nötig ist. | Spike (Variante G: eigener Treiber meldet Mac2; beobachten) | M4 |
| G-2 | Shader-Form im Paravirt-Protokoll (AIR oder Apple-GPU-Code)? | reverse-engineer (S-GPU-1) | M4 |
| G-3 | Exakte Zuordnung Familie ⇒ Features für macOS 27 (Metal-Feature-Set-Tabellen), insbesondere Metal4. **Stand WP-GPU-02:** Die Ausgabe 2026-05-21 der Feature-Set-Tabellen kennt nur noch Apple2–Apple10 (keine Mac2-Spalte mehr) und nennt Apple7 als Untergrenze aller Metal-3/4-Geräte. Ob ein Gerät, das nur Mac2/Metal3 meldet, unter macOS 27 überhaupt akzeptiert wird, ist damit fraglich → verschärft G-1. | gpu-engineer mit öffentlicher Doku; architect | M4 |
| G-4 | Gleiche Fähigkeitsmeldung auf beiden GPUs (kleinster gemeinsamer Nenner) oder ehrlich pro GPU? | Mensch | M5 |
| G-5 | ASTC→BC7 (verlustbehaftet, 1 B/px) als Option erlaubt? Default bleibt exakt. | Mensch | WP-GPU-05 |
| G-6 | EDR/`BGRA10_XR`/HDR auf SDR-Panel: Tonemapping-Regel? | gpu-engineer + Mensch | WP-GPU-15 |
| G-7 | ~~`lodAverage` und andere Sampler-Feinheiten ohne Vulkan-Entsprechung~~ **Entschieden (WP-GPU-02):** `lodAverage` wird ignoriert (laut Apple ein Hinweis, den manche GPUs ignorieren); `reductionMode`/`lodBias` (Apple10) → `Unsupported`. | gpu-engineer | erledigt |
| G-8 | Nutzt MetalFX private Treiberpfade? | Spike | WP-GPU-23 |
| G-9 | Umfang von Metal 4 / Tensoren in macOS 27; Verhältnis zu `VK_KHR_cooperative_matrix`. | gpu-engineer + shader-compiler-engineer | M7 |
| G-10 | Schnittstelle `ritsu-mem` ↔ GPU: memfd-Backing des Gast-RAM, Alias-Mappings, Dirty-Logging pro Seite. Braucht ADR. | architect | vor WP-GPU-11 |
| G-11 | Welche Barrieren sind innerhalb von dynamic rendering mit `dynamic_rendering_local_read` erlaubt (Pass-Split vermeiden)? | gpu-engineer | WP-GPU-06 |
| G-12 | Allocator: `gpu-allocator`-Crate oder eigener? (neue externe Abhängigkeit) | architect | WP-GPU-04 |
| G-13 | Ist geteilter GPU-Adressraum für alle Gast-Prozesse akzeptabel (BDA)? Alternative: ein `VkDevice` pro Gast-Prozess (teuer). | architect + Mensch | M5 |
| G-14 | Transport für G: eigener virtio-Typ oder virtio-gpu-Context-Type? | architect + platform-engineer | ADR-0005-Entscheidung |
| G-15 | Iris-Ausgabe in Appliance-Modus: Scanout linearer dma-bufs von NVIDIA – geht das ohne Zwischenkopie? | gpu-engineer auf Hardware | WP-GPU-12 |
| G-16 | NVK statt proprietärem Treiber (freier Stack im Appliance-Image) – Feature-Lücken (Ray Tracing) akzeptabel? | Mensch | WP-GPU-03 |
