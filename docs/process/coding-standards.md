# Coding-Standards

## Allgemein

* Code, Bezeichner, Kommentare, Commit-Nachrichten: **Englisch**. Projektdoku: **Deutsch**.
* Kommentare erklären das *Warum* und verweisen auf die Quelle (Arm ARM-Abschnitt, virtio-Spec-Paragraph,
  Spezifikation in `docs/specs/`).
* Keine toten Codepfade, kein auskommentierter Code. TODOs nur mit WP-ID: `// TODO(WP-DBT-07): …`.

## Rust

* Edition 2024, stable Toolchain (`rust-toolchain.toml`). Formatierung `rustfmt` (100 Spalten).
* `cargo clippy --all-targets -- -D warnings` mit den Workspace-Lints aus `Cargo.toml`.
* **Fehler:** Pro Crate ein `thiserror`-Enum, an Crate-Grenzen nach `ritsu_core::Error` konvertierbar. Kein
  `unwrap()`/`expect()` außerhalb von Tests (Ausnahme: echte Invarianten mit Kommentar). Kein `todo!()`/`unimplemented!()`.
* **unsafe:** nur in Modulen mit sprechendem Namen (`jit_mem`, `host_map`, `ffi`), jeder Block mit `// SAFETY:`. Neue
  unsafe-Module brauchen Review durch `code-reviewer` mit Schwerpunkt unsafe.
* **Typen statt Konventionen:** `GuestPhysAddr` ≠ `GuestVirtAddr`, Handles generationssicher, Größen als `u64` mit
  geprüfter Arithmetik (`checked_*`) bei Gast-Werten.
* Öffentliche Elemente haben rustdoc. `unreachable_pub` beachten: was nicht exportiert wird, ist `pub(crate)`.
* Tests im selben Modul (`#[cfg(test)]`), crateübergreifende Tests in `tests/`.

## C++ (compiler/)

* C++20, `clang-format` (Konfiguration in `compiler/.clang-format`), `-Wall -Wextra -Wpedantic -Werror`.
* Keine Exceptions über die C-ABI. RAII, `std::span`/`std::string_view`, kein rohes `new`/`delete`.
* Öffentliche C-Header kompilieren als C99. Structs mit `struct_size` für ABI-Evolution.

## Leistungskritischer Code

* Jede Optimierung kommt mit einem Benchmark in `bench/` und Vorher/Nachher-Zahlen im Commit-Text.
* Host-ISA höchstens AVX2/BMI2/FMA/AES-NI/SHA-NI mit Laufzeiterkennung. Kein AVX-512.
