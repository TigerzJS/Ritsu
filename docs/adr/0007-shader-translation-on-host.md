# ADR-0007: Shader werden auf dem Host von AIR nach SPIR-V übersetzt

* **Status:** Accepted
* **Datum:** 2026-10-14
* **Betrifft:** `compiler/air2spirv`, `crates/ritsu-gpu-host`

## Kontext

Metal-Shader kommen als `.metallib` mit AIR (Apples LLVM-Bitcode-Dialekt mit `air.*`-Intrinsics und Metadaten). Vulkan
braucht SPIR-V. Vorhandene Werkzeuge (SPIRV-LLVM-Translator) zielen auf OpenCL-SPIR-V (Kernel-Capability) und
eignen sich nicht für Grafik-Shader (Shader-Capability).

## Entscheidung

* Eigener Übersetzer `air2spirv` in C++20 auf Basis von LLVM (Bitcode lesen, Analyse) und SPIRV-Tools (Validierung,
  Optimierung). Ausgabe ist **Vulkan-SPIR-V 1.6**.
* Er läuft **auf dem Host** in `ritsu-gpu-host` (über die C-ABI), nie im Gast.
* Jedes erzeugte Modul wird in Debug-Builds und in CI mit `spirv-val` validiert.
* Cache: SPIR-V wird nach (AIR-Hash, Pipeline-Zustand, der den Shader beeinflusst, Compiler-Version) gecacht.
  `VkPipelineCache` wird pro GPU gespeichert.
* AIR-Bitcode-Versionen von Apple können neuer sein als das gelinkte LLVM. Unbekannte Records werden **erkannt und
  gemeldet**, niemals still verworfen.

## Konsequenzen

* Testkorpus aus selbst geschriebenen Metal-Shadern, die einmalig auf einem Mac zu AIR kompiliert und als Fixtures
  eingecheckt werden (eigener Code → eigene Fixtures, keine Apple-Shader).
* Ausgabe wird mit FileCheck-artigen Tests und Bildvergleich (über `ritsu-gpu-host`) geprüft.
