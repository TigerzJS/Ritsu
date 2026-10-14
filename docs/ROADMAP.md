# Roadmap

Jeder Meilenstein endet mit einem **sichtbaren, prüfbaren Ergebnis**. Ein Meilenstein ist erst erledigt, wenn sein
Abnahmekriterium auf der Referenzmaschine (G5 MF) bzw. in CI nachgewiesen ist. ★ markiert Hochrisiko-Arbeit, die
zuerst als Spike (`/spike`) läuft.

Detaillierte Work Packages pro Subsystem stehen in den Subsystem-Dokumenten:
[dbt.md](subsystems/dbt.md) · [gpu.md](subsystems/gpu.md) · [shader-compiler.md](subsystems/shader-compiler.md) ·
[platform.md](subsystems/platform.md) · [host-os.md](subsystems/host-os.md)

## Übersicht

| M | Name | Abnahmekriterium | Hauptsächlich | Status |
|---|---|---|---|---|
| M0 | Fundament | Struktur, ADRs, CI grün, Skelette mit Tests, Multi-Agent-Prozess dokumentiert | alle | **erledigt** |
| M1 | Erster Befehl | Ein statisch gelinktes aarch64-**Linux**-ELF („hello world“, dann eine Testsuite) läuft im User-Mode über T0. Differenztests gegen arm64-CI-Runner sind grün | DBT | **in Arbeit** |
| M2 | JIT | T1 läuft und ist differenzgetestet. Erste Leistungszahlen für Q-PERF-1 (User-Mode-Benchmarks) | DBT | offen |
| M3 | Kernel spricht | vmapple-Plattform, System-Mode-DBT (EL1, MMU, Exceptions, GIC, Timer). XNU gibt Boot-Log über PL011 aus ★ | Plattform, DBT | offen |
| M4 | Erstes Dreieck | Metal-Dreieck (eigene Test-App im Gast) wird auf RTX 4050 **und** Iris Xe gerendert. ADR-0005 entschieden ★ | GPU, Shader | offen |
| M5 | Desktop | macOS 27 bootet bis zum Desktop. WindowServer komponiert über Metal → Vulkan, Anzeige auf dem internen Panel per PRIME | alle | offen |
| M6 | Benutzbar | AOT-Cache, T2-JIT, Q-PERF-Ziele „M6“ erreicht, Netzwerk, Audio, Eingabe, Appliance-Image installierbar | alle | offen |
| M7 | Metal-Breite | Argument-Buffer Tier 2, Indirect Command Buffers, Mesh Shader (RTX), Ray Tracing (RTX), Metal 4 Kernfunktionen | GPU, Shader | offen |
| M8 | Reif | Q-PERF-Ziele „M8“, Fuzzing dauerhaft in CI, Kompatibilitätsmatrix für ≥ 3 macOS-27-Builds | alle | offen |

M1/M2 und M4 sind absichtlich **voneinander unabhängig**: DBT und GPU können parallel von verschiedenen Agenten
vorangetrieben werden. Die GPU-Arbeit vor M3 läuft gegen Host-seitige Tests (lavapipe, synthetische Befehlsströme),
nicht gegen einen echten Gast.

## M0 – Fundament (erledigt)

| WP | Inhalt | Agent | Status |
|---|---|---|---|
| WP-M0-ARCH | ARCHITECTURE.md, ADR-0001…0008, CLAUDE.md, Agenten, Skills, Prozess-Doku | Orchestrator | erledigt |
| WP-M0-DBT | dbt.md + Skelett `ritsu-dbt`/`ritsu-mem` (Decoder-Teilmenge, T0, TLB) | dbt-engineer | erledigt |
| WP-M0-GPU | gpu.md + Skelett `ritsu-gpu-proto`/`ritsu-gpu-host` (Protokoll-Decoder, Handles, Caps) | gpu-engineer | erledigt |
| WP-M0-SC | shader-compiler.md + CMake-Skelett `air2spirv` mit C-ABI | shader-compiler-engineer | erledigt |
| WP-M0-PLAT | platform.md, host-os.md + Skelett `ritsu-platform`/`ritsu-virtio`/`ritsu-vmm` | platform-engineer | erledigt |
| WP-M0-QA | testing.md, Makefile, CI, Prüfskripte | test-engineer | erledigt |
| WP-M0-REV | Gesamt-Review des M0-Stands | code-reviewer | erledigt |

## Architektur-Folgearbeiten (aus M0-Berichten)

| WP | Inhalt | Agent | Vor |
|---|---|---|---|
| WP-ARCH-01 | `GuestMemory` für gleichzeitigen Zugriff mehrerer vCPU- und Geräte-Threads neu schneiden (`&self` + innere Veränderlichkeit, Speicherordnung für virtio-Ringe, Host-`mmap`-Backend). Neuer ADR, ersetzt die M0-Schnittstelle | architect | M1 |
| WP-ARCH-03 | ADR „Gast-CPU-Feature-Baseline“: macOS wird vermutlich für eine feste CPU-Baseline (M1) kompiliert und nutzt deren Befehle ohne Blick in die ID-Register. Das widerspricht der Konsequenz „minimaler Feature-Satz“ aus ADR-0004 → neuer ADR mit ISA-Zensus (WP-DBT-19) als Grundlage | architect + dbt-engineer | M3 |
| WP-ARCH-04 | ADR „Shader-Compiler-Isolation“: LLVM ist nicht gegen feindliches Bitcode gehärtet, und Fatal Errors lassen sich nicht abfangen → `air2spirv` in einem sandboxed Worker-Prozess statt im Host-Prozess (Q-SEC-1) | architect | M4 |
| WP-ARCH-05 | ADR „Bindungsmodell“: Argument-Tabelle + Bindless-Heaps vs. Descriptor-Sets, gemeinsam für gpu.md und shader-compiler.md | architect + gpu-engineer + shader-compiler-engineer | WP-SC-12 |
| WP-MEM-01 | `GuestMemory` v2 nach ADR-0009 (nach Freigabe O-13) | dbt-engineer | M3 |
| WP-MEM-02 ★ | `GuestRam` (memfd + `host_map`, unsafe-Kern) nach ADR-0009 | dbt-engineer | M3 |
| WP-PLAT-21 | virtio-Speicherordnung nach ADR-0009 | platform-engineer | M3 |
| WP-ARCH-06 | Normative Prüfung offener Architekturregeln gegen Arm DDI 0487 | architect | M3 |
| WP-ARCH-02 | Schnittstelle DBT ↔ Plattform: Systemregister-Hooks (`MRS`/`MSR` für GIC-ICC), `HVC`/`SMC` (PSCI), `WFI`/`WFE`-Parken, IRQ-Pending-Flag und vCPU-Kick | architect | M3 |

## Spikes (vor den abhängigen Meilensteinen)

| Spike | Frage | Entscheidet | Agent | Vor |
|---|---|---|---|---|
| S-GPU-1 ★ | Lässt sich die Host-Seite des Apple-Paravirt-GPU-Protokolls beschreiben (Dreieck-Niveau)? Welche Metal-Familien meldet der Gast? | ADR-0005 | reverse-engineer | M4 |
| S-GPU-2 ★ | Lässt sich im macOS-27-Gast ein eigenes Metal-Device-Bundle laden? | ADR-0005 | gpu-engineer (Prototyp, macOS-SDK nötig) | M4 |
| S-BOOT-1 ★ | Kann ein eigener Loader den vmapple-Kernelcache ohne Apples `AVPBooter` starten? | ADR-0004-Folge | reverse-engineer + platform-engineer | M3 |
| S-BOOT-2 ★ | Kann eine macOS-27-IPSW ohne Mac in ein VM-Disk-Image installiert werden (Restore, Personalisierung)? | Installationsweg | reverse-engineer | M5 |
| S-PERF-1 | Wie schnell ist ein naiver T1 auf dem i5-12500H, gemessen an User-Mode-Benchmarks? Reicht Cranelift für T2? | ADR-0006-Überprüfung | dbt-engineer | M2 |
| S-AIR-1 | Welche LLVM-Version liest AIR aus dem macOS-27-SDK fehlerfrei? | air2spirv-LLVM-Pin | shader-compiler-engineer | M4 |
