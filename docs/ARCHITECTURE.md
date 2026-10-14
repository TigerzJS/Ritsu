# Ritsu – Architektur

> Status: **Entwurf v0.1** (M0). Verbindliche Entscheidungen stehen in den [ADRs](adr/). Wo dieses Dokument und ein
> ADR sich widersprechen, gilt der ADR, und dieses Dokument ist ein Bug.

## 1. Ziel in einem Satz

Ritsu soll **macOS 27 (arm64) mit voller Metal-Beschleunigung auf x86-64-PCs** laufen lassen, als Erstes auf dem
Gigabyte G5 MF (i5-12500H, RTX 4050 Laptop, Iris Xe). Dafür übersetzt Ritsu den **Gast-Prozessor** (AArch64 → x86-64)
und die **Gast-Grafik** (Metal → Vulkan).

## 2. Warum kein klassischer Hackintosh mehr geht

macOS 27 „Golden Gate“ ist das erste macOS, das **nur auf Apple Silicon** läuft. Es gibt keinen x86-64-Kernel, keine
x86-64-Systembibliotheken und kein x86-64-`dyld` mehr. OpenCore + Kexts kann deshalb nichts mehr booten: Es gibt
keinen x86-Code, den man patchen könnte (→ [ADR-0001](adr/0001-full-system-translation.md)).

Übrig bleibt **Full-System-Translation**: Wir booten den echten arm64-Kernel von macOS 27 in einer virtuellen
Apple-Plattform und führen jeden Befehl über einen Binärübersetzer (DBT) auf der x86-CPU aus. Die GPU-Befehle des
Gasts fangen wir ab und führen sie über Vulkan auf der echten GPU aus.

Für den Nutzer soll es sich trotzdem wie ein Hackintosh anfühlen. Er bootet einen Stick oder eine Partition und landet
direkt im macOS-Desktop. Dazwischen liegt ein minimales Linux-Host-System, das man nicht zu sehen bekommt
(→ [ADR-0002](adr/0002-linux-host-vulkan.md)).

## 3. Schichtenmodell

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ GAST: macOS 27 arm64e (unverändert, vom Nutzer selbst aus Apples IPSW)        │
│   Apps ─ Metal.framework ─ WindowServer ─ dyld shared cache                   │
│   XNU-Kernel (vmapple-Kernelcache)                                            │
│   Gast-GPU-Treiber  ← Variante P (Apple Paravirt) oder G (eigener) ADR-0005   │
├───────────────────────────────── Grenze: virtuelle Hardware ─────────────────┤
│ HOST-PROZESS: ritsu-vmm (Rust)                                                │
│  ┌──────────────┐ ┌──────────────────┐ ┌───────────────────────────────────┐  │
│  │ ritsu-dbt    │ │ ritsu-platform   │ │ GPU-Pfad                          │  │
│  │ AArch64→x86  │ │ vmapple-Maschine │ │ ritsu-gpu-proto  (Transport)      │  │
│  │ T0 Interp.   │ │ GICv3, Timer,    │ │ ritsu-gpu-host   (Metal→Vulkan)   │  │
│  │ T1 Basis-JIT │ │ PSCI, AES, CFG,  │ │ air2spirv (C++)  (AIR→SPIR-V)     │  │
│  │ T2 Opt.-JIT  │ │ Boot-Pfad        │ │ Pipeline-/Shader-Cache            │  │
│  │ AOT-Cache    │ ├──────────────────┤ └───────────────────────────────────┘  │
│  ├──────────────┤ │ ritsu-virtio     │                                         │
│  │ ritsu-mem    │ │ blk,net,input,   │                                         │
│  │ SoftMMU, TLB │ │ console,sound    │                                         │
│  └──────────────┘ └──────────────────┘                                         │
├──────────────────────────────────────────────────────────────────────────────┤
│ HOST-OS: minimales Linux (Appliance), Kernel-Treiber für echte Hardware       │
│   Vulkan: NVIDIA (proprietär oder NVK) │ Intel ANV │ PRIME/KMS für Anzeige    │
├──────────────────────────────────────────────────────────────────────────────┤
│ HARDWARE: i5-12500H (AVX2, 4P+8E) │ RTX 4050 Laptop (AD107) │ Iris Xe (Xe-LP) │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 3.1 Warum diese Grenzen

| Grenze | Begründung |
|---|---|
| **Host = Linux** | Für jede Komponente des G5 MF (WLAN, BT, Touchpad, Audio, Iris Xe, RTX 4050) gibt es dort reife Treiber. Die Hälfte der alten Hackintosh-Probleme (WLAN, Touchpad, Helligkeit, interner Bildschirm an der Iris Xe) verschwindet damit. |
| **Gast-Plattform = Apple-VM-Plattform** | Apple liefert für macOS-VMs eine eigene, kleine Plattform (`vmapple`: GICv3, PL011, virtio, wenige Apple-Geräte). Ein echtes Apple-SoC (AIC, DART, ANS, SEP …) wäre um Größenordnungen aufwendiger (→ [ADR-0004](adr/0004-guest-platform-vmapple.md)). |
| **GPU-API am Host = Vulkan 1.3+** | Vulkan ist die einzige API, die auf NVIDIA und Intel unter Linux alle nötigen Features hat (Descriptor Buffer, Buffer Device Address, Mesh Shader, Ray Query, Timeline-Semaphoren). |
| **Shader-Übersetzung am Host** | Der Host läuft nativ, der Gast emuliert. Ein Compiler im Gast wäre 3–10× langsamer. Der Host kann außerdem prozessübergreifend und dauerhaft cachen (→ [ADR-0007](adr/0007-shader-translation-on-host.md)). |

## 4. Komponenten

| Komponente | Pfad | Sprache | Aufgabe | Detail-Doku |
|---|---|---|---|---|
| `ritsu-core` | `crates/ritsu-core` | Rust | Gemeinsame Typen: Adressen, Fehler, Feature-Flags. Keine Logik. | – |
| `ritsu-dbt` | `crates/ritsu-dbt` | Rust | AArch64-Decoder, IR, Interpreter (T0), Basis-JIT (T1), optimierender JIT (T2), Übersetzungs-Cache | [dbt.md](subsystems/dbt.md) |
| `ritsu-mem` | `crates/ritsu-mem` | Rust | Physischer Gastspeicher, SoftMMU (Stage-1-Walk, 16K-Granule), TLB, Code-Seiten-Schutz (SMC) | [platform.md](subsystems/platform.md) |
| `ritsu-platform` | `crates/ritsu-platform` | Rust | vmapple-Maschinenmodell: Speicherkarte, GICv3, Generic Timer, PSCI, PL011, Apple-VM-Geräte, Boot | [platform.md](subsystems/platform.md) |
| `ritsu-virtio` | `crates/ritsu-virtio` | Rust | virtio-mmio/-pci Transport + Geräte (blk, net, input, console, sound) | [platform.md](subsystems/platform.md) |
| `ritsu-gpu-proto` | `crates/ritsu-gpu-proto` | Rust | Definition und (De-)Serialisierung des Gast↔Host-GPU-Befehlsstroms | [gpu.md](subsystems/gpu.md) |
| `ritsu-gpu-host` | `crates/ritsu-gpu-host` | Rust | Metal-Semantik → Vulkan: Ressourcen, Encoder, Synchronisation, Präsentation, Backend-Abstraktion | [gpu.md](subsystems/gpu.md) |
| `air2spirv` | `compiler/air2spirv` | C++20 | Apple AIR (LLVM-Bitcode) → SPIR-V, mit C-ABI für Rust | [shader-compiler.md](subsystems/shader-compiler.md) |
| `ritsu-vmm` | `crates/ritsu-vmm` | Rust | Das Host-Programm: Konfiguration, vCPU-Threads, Verdrahtung aller Komponenten | [platform.md](subsystems/platform.md) |
| Gast-Komponenten | `guest/` | C/ObjC/C++ | Nur bei Variante G: eigener Gast-GPU-Treiber (Kext + Metal-Bundle). Braucht macOS-SDK. | [guest/README.md](../guest/README.md) |
| Host-OS-Image | `host-os/` | Shell/Config | Appliance-Linux: bootet direkt in `ritsu-vmm` im Vollbild | [host-os.md](subsystems/host-os.md) |

**Abhängigkeitsregel (wird in CI geprüft, sobald die Crates Logik enthalten):**

```
ritsu-vmm ──► ritsu-platform ──► ritsu-dbt ──► ritsu-mem ──► ritsu-core
         │                 └──► ritsu-virtio ──────────────► ritsu-core
         └──► ritsu-gpu-host ──► ritsu-gpu-proto ───────────► ritsu-core
                          └──► air2spirv (FFI, C-ABI)
```

Keine Zyklen. `ritsu-dbt` kennt keine Geräte. `ritsu-gpu-host` kennt keine CPU-Emulation. Die beiden
Hochrisiko-Teile (DBT und GPU) sind dadurch **unabhängig voneinander entwickel- und testbar**.

## 5. Die zwei Übersetzungen im Detail

### 5.1 CPU: AArch64 → x86-64 (ritsu-dbt)

Kurzfassung, Details in [dbt.md](subsystems/dbt.md) und [ADR-0006](adr/0006-dbt-tiered-design.md):

* **Mehrstufig:** T0 Interpreter (Referenz und kalter Code) → T1 Template-JIT (schnell übersetzt) → T2 optimierender
  Regionen-JIT (Cranelift-Backend, heißer Code).
* **Persistenter Übersetzungs-Cache:** Der `dyld shared cache` von macOS ist pro Build fest und mehrere GB groß.
  Übersetzungen werden pro (macOS-Build, Seiten-Hash) auf die Platte geschrieben. Ab dem zweiten Boot läuft der
  größte Teil des Systemcodes ohne JIT-Kosten (das Prinzip von Rosetta 2, hier für Full-System).
* **Speichermodell:** AArch64 ist schwächer geordnet als x86-TSO. Normale Loads/Stores lassen sich deshalb direkt
  abbilden. Ausnahme: `STLR` gefolgt von `LDAR` (RCsc) braucht auf x86 eine Store-Load-Barriere.
* **Exklusive Zugriffe** (`LDXR/STXR`) werden über `CMPXCHG` emuliert. Das ABA-Problem ist dokumentiert und
  wird getestet. Für LSE-Atomics gibt es direkte x86-Gegenstücke.
* **arm64e:** Pointer Authentication (PAC) muss emuliert werden, weil Kernel und System damit gebaut sind. Die
  PAC-Schlüssel liegen im vCPU-Zustand, und `AUT*`/`PAC*` werden als schnelle Inline-Sequenz übersetzt.
* **NEON → SSE4.2/AVX2.** Der i5-12500H hat **kein AVX-512**, deshalb darf kein Codepfad davon abhängen.
* **Hybrid-CPU:** vCPU-Threads werden bevorzugt auf P-Kerne gelegt, JIT-Compiler-Threads auf E-Kerne.

### 5.2 GPU: Metal → Vulkan (ritsu-gpu-host + air2spirv)

Kurzfassung, Details in [gpu.md](subsystems/gpu.md):

* **Zwei mögliche Gast-Seiten** (→ [ADR-0005](adr/0005-gpu-guest-path.md), noch offen, Spikes S-GPU-1/2 entscheiden):
  * **P – Apple Paravirtual GPU:** macOS 27 bringt den Gast-Treiber für Apples virtuelle GPU schon mit. Wir
    implementieren die Host-Seite des Protokolls neu (unter Clean-Room-Regeln, siehe [ADR-0008](adr/0008-clean-room.md)).
    Vorteil: kein eigener Code im Gast. Risiko: undokumentiertes Protokoll, möglicherweise eingeschränkter
    Metal-Feature-Umfang.
  * **G – Eigener Gast-Treiber:** Kext + Metal-Treiber-Bundle im Gast, die einen eigenen, von uns dokumentierten
    Befehlsstrom (`ritsu-gpu-proto`) über virtio senden. Vorteil: volle Kontrolle über die Features. Risiko: private
    Metal-Treiber-Schnittstellen und eine Code-Signatur im Gast.
* **Gemeinsam:** Beide Varianten enden im selben Host-Modell. `ritsu-gpu-host` nimmt Metal-artige Objekte entgegen
  (Buffer, Texture, RenderPass, ComputePass, ArgumentBuffer, Event) und bildet sie auf Vulkan ab.
* **Shader:** AIR aus `.metallib` → `air2spirv` → SPIR-V → Vulkan-Treiber. Dahinter liegen zwei Cache-Stufen:
  SPIR-V nach AIR-Hash und `VkPipelineCache` pro GPU.
* **Backends:** `nvidia` (RTX 4050, proprietärer Treiber oder NVK) und `intel` (Iris Xe, ANV). Jedes Backend meldet
  eine **Fähigkeitsmatrix**, und daraus ergibt sich die Metal-GPU-Familie, die dem Gast gemeldet wird. Fehlende
  Hardware-Features (Ray Tracing und Mesh Shader auf Iris Xe, ASTC) werden emuliert oder dem Gast ehrlich nicht
  gemeldet. Niemals still falsch rendern.
* **Anzeige:** Der G5 MF hat keinen MUX, der interne Bildschirm hängt an der Iris Xe. Der Host rendert auf der RTX
  4050 und gibt per PRIME über die Iris Xe aus. Unter Linux ist das Standard, unter dem alten Hackintosh war es
  unmöglich.

## 6. Qualitätsziele

| ID | Ziel | Messung | Zielwert M6 | Zielwert M8 |
|---|---|---|---|---|
| Q-PERF-1 | CPU-Leistung (Integer, Gast-Userland) | Geekbench-ähnliche Mikro-Benchmarks in `bench/`, Gast vs. native arm64-Referenzwerte, normiert auf Takt | ≥ 25 % | ≥ 50 % |
| Q-PERF-2 | Boot-Zeit bis Login-Fenster (warmer Cache) | `bench/boot` | < 120 s | < 45 s |
| Q-PERF-3 | GPU-Durchsatz | Metal-Benchmarks im Gast vs. native Vulkan-Äquivalente am Host | ≥ 50 % | ≥ 80 % |
| Q-PERF-4 | Frame-Latenz Desktop | Zeit von Gast-`present` bis Scanout | < 16 ms | < 8 ms |
| Q-COR-1 | DBT-Korrektheit | Differenztests gegen Referenz (siehe [testing.md](process/testing.md)) | 0 bekannte Abweichungen in den Basis-ISA-Tests | + NEON, PAC, LSE |
| Q-COR-2 | Rendering-Korrektheit | Bildvergleich (Toleranz pro Test) gegen Referenzbilder von echter Apple-Hardware | – | alle Pflichtszenen |
| Q-SEC-1 | Host-Isolation | Gast kann den Host-Prozess nicht kompromittieren: alle Gast-Eingaben (MMIO, virtio, GPU-Strom) werden validiert | Fuzzing aller Parser | dauerhaft im CI |

Die Zahlen sind **Ziele, keine Versprechen**. Full-System-Emulation ohne Hardware-Virtualisierung ist der schwerste
denkbare Fall für Leistung, siehe Risiko R-2.

## 7. Risiken (Top 6)

| ID | Risiko | Wirkung | Gegenmaßnahme |
|---|---|---|---|
| R-1 | **Rechtliches.** Apples macOS-Lizenz erlaubt macOS (auch in VMs) nur auf Apple-Hardware. | Projekt-Existenz | Ritsu verteilt **keine** Apple-Binärdateien. Der Nutzer liefert IPSW und Boot-ROM selbst. Reverse Engineering nur nach [ADR-0008](adr/0008-clean-room.md). Keine Rechtsberatung, das Risiko liegt beim Nutzer (wie beim alten Hackintosh). |
| R-2 | **Leistung.** Full-System-DBT ohne Hardware-Hilfe ist langsam. | Nutzbarkeit | AOT-Cache für den dyld shared cache, mehrstufiger JIT, früh messen (M1), Leistungsbudget pro Meilenstein |
| R-3 | **Paravirt-Protokoll undokumentiert** (Variante P) | GPU-Pfad blockiert | Variante G als Rückfall, Spike S-GPU-1 vor jeder größeren Investition |
| R-4 | **Boot und Installation brauchen Apple-Komponenten** (Boot-ROM `AVPBooter`, IPSW-Restore, Personalisierung) | Kein Boot ohne Mac | Spike S-BOOT-1/2. Rückfall Phase 1: Disk-Image und Boot-ROM einmalig auf einem Mac des Nutzers erzeugen (wie bei QEMU `vmapple`) |
| R-5 | **Apple ändert Interna pro Update** (Protokolle, AIR-Version, Kernelcache) | Ständiger Wartungsaufwand | Versionierte Protokoll-Adapter, Kompatibilitätsmatrix in `docs/compat/`, Tests pro unterstütztem Build |
| R-6 | **Umfang.** Projekt ist sehr groß. | Stillstand | Meilensteine mit sichtbarem Ergebnis, harte Scope-Grenzen pro Meilenstein ([ROADMAP.md](ROADMAP.md)), Multi-Agent-Prozess ([orchestration.md](process/orchestration.md)) |

## 8. Nicht-Ziele (bis auf Widerruf per ADR)

* Kein nativer x86-Hackintosh mit macOS 27. Das ist technisch unmöglich, siehe §2.
* Keine Weiterentwicklung der alten Sequoia-EFI. Sie bleibt als funktionierender Stand in `legacy/g5mf-sequoia/`.
* Kein Windows-Host in v1. Er wäre technisch möglich (Vulkan gibt es dort auch), verdoppelt aber den Aufwand für die
  Plattform-Schicht.
* Keine Weitergabe von Apple-Binärdateien, Firmware oder disassembliertem Apple-Code im Repo.
* Kein iCloud-/iMessage-Spoofing. Apple-Dienste sind nicht Teil der Ziele.
