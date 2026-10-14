# Ritsu

**macOS 27 mit voller Metal-Beschleunigung auf x86-64-PCs.** Erstes Zielgerät: Gigabyte G5 MF (i5-12500H,
RTX 4050 Laptop, Intel Iris Xe).

> **Status: M0 – Fundament.** Es läuft noch nichts. Dieses Repo enthält Architektur, Entscheidungen, Prozess und
> getestete Code-Skelette. Wer heute einen funktionierenden Hackintosh will: [`legacy/g5mf-sequoia/`](legacy/g5mf-sequoia/)
> (macOS 15 + NullMoth-NVIDIA-Treiber).

## Warum das ein neues Projekt ist

macOS 27 läuft **nur noch auf Apple Silicon**. Es gibt keinen x86-Kernel mehr, den OpenCore booten könnte. Der
klassische Hackintosh endet mit macOS 26. Ritsu geht deshalb einen anderen Weg:

```
macOS 27 (arm64, unverändert)
   │  CPU-Befehle            │  Metal-Befehle
   ▼                         ▼
ritsu-dbt                ritsu-gpu-host + air2spirv
AArch64 → x86-64         Metal → Vulkan, AIR → SPIR-V
   │                         │
   ▼                         ▼
i5-12500H               RTX 4050 / Iris Xe
        minimales Linux als unsichtbarer Host
```

Ein Nebeneffekt: Alles, was beim alten Hackintosh auf diesem Laptop nicht ging (WLAN ohne HeliPort, beschleunigter
interner Bildschirm an der Iris Xe, Helligkeit, Ruhezustand), regeln Linux-Treiber auf dem Host.

Das Projekt ist riesig und vielleicht unmöglich. Die größten Risiken sind Leistung (Full-System-Emulation ohne
Hardware-Hilfe), undokumentierte Apple-Schnittstellen und Apples Lizenz. Alle Risiken stehen offen in
[ARCHITECTURE.md §7](docs/ARCHITECTURE.md#7-risiken-top-6).

## Einstieg

| Für | Lies |
|---|---|
| Überblick | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| Entscheidungen | [docs/adr/](docs/adr/README.md) |
| Plan | [docs/ROADMAP.md](docs/ROADMAP.md), [offene Fragen](docs/OPEN-QUESTIONS.md) |
| Mitarbeit mit Claude Code | [CLAUDE.md](CLAUDE.md), [Orchestrierung](docs/process/orchestration.md) |
| Subsysteme | [DBT](docs/subsystems/dbt.md) · [GPU](docs/subsystems/gpu.md) · [Shader-Compiler](docs/subsystems/shader-compiler.md) · [Plattform](docs/subsystems/platform.md) · [Host-OS](docs/subsystems/host-os.md) |

## Bauen

```bash
make check     # Format, Lints, alle Tests (Rust + C++), Architekturregeln, Doku-Links
make help      # alle Ziele
```

Voraussetzungen: Rust (stable, siehe `rust-toolchain.toml`), CMake ≥ 3.25, Ninja, GCC oder Clang mit C++20, Python 3.

## Rechtliches

Ritsu enthält und verteilt **keine** Apple-Software. Apples Lizenz erlaubt macOS nur auf Apple-Hardware. Die Nutzung
erfolgt auf eigenes Risiko, wie beim klassischen Hackintosh. Reverse Engineering erfolgt ausschließlich nach dem
[Clean-Room-Verfahren](docs/process/clean-room.md). Die Projektlizenz ist noch nicht festgelegt
([O-1](docs/OPEN-QUESTIONS.md)).
