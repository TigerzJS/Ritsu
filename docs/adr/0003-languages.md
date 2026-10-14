# ADR-0003: Rust für den Host, C++20 für den Shader-Compiler, C/ObjC nur im Gast

* **Status:** Accepted
* **Datum:** 2026-10-14
* **Betrifft:** alle Quellverzeichnisse

## Kontext

Der Host-Prozess verarbeitet ausschließlich nicht vertrauenswürdige Eingaben vom Gast (MMIO, DMA, GPU-Befehlsströme,
Shader). Speicherfehler dort sind Sicherheitslücken (Q-SEC-1). Gleichzeitig braucht der Shader-Compiler LLVM, um
AIR-Bitcode zu lesen, und LLVMs stabile API ist C++.

## Entscheidung

* **Rust (stable, Edition 2024)** für alle Host-Komponenten. `unsafe` nur in klar abgegrenzten Modulen (JIT-Speicher,
  FFI, Gastspeicher-Mapping), jeweils mit `// SAFETY:`-Kommentar und eigenen Tests.
* **Cranelift** als Codegenerator für den optimierenden JIT T2 (Rust-nativ, Apache-2.0, im Produktionseinsatz in
  Wasmtime).
* **C++20 + LLVM** für `compiler/air2spirv`. Die Schnittstelle zu Rust ist eine schmale C-ABI (`air2spirv.h`). Es
  gelangen keine C++-Typen nach Rust.
* **C/ObjC/C++ mit macOS-SDK** nur für Gast-Komponenten (`guest/`), falls Variante G aus ADR-0005 gewählt wird.
* Code, Bezeichner, Kommentare und Commit-Nachrichten auf **Englisch**. Projektdokumentation auf **Deutsch**.

## Konsequenzen

* Zwei Build-Systeme (Cargo, CMake), zusammengeführt über das `Makefile` im Wurzelverzeichnis und die CI.
* Für Rust gelten `cargo fmt`, `cargo clippy -D warnings`, für C++ `clang-format` und `clang-tidy`.
