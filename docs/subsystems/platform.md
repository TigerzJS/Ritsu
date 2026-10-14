# Plattform – vmapple-Maschinenmodell, virtio, Boot, vmm

> Status: **Entwurf v0.1** (WP-M0-PLAT). Zuständig: `platform-engineer`. Gilt zusammen mit
> [ADR-0004](../adr/0004-guest-platform-vmapple.md) (Plattform), [ADR-0002](../adr/0002-linux-host-vulkan.md) (Host)
> und [ADR-0008](../adr/0008-clean-room.md) (Clean Room). Bei Widerspruch gilt der ADR.
> Crates: `ritsu-platform`, `ritsu-virtio`, `ritsu-vmm`. Das Host-Betriebssystem steht in [host-os.md](host-os.md).

## 0. Quellenlage und Kennzeichnung

Über die Apple-VM-Plattform („vmapple“, das Gerätemodell von Virtualization.framework) ist **wenig öffentlich
dokumentiert**. Dieses Dokument trennt deshalb strikt:

| Marke | Bedeutung |
|---|---|
| **[öffentlich]** | Steht in öffentlicher Dokumentation: ARM-Handbücher (GICv3 IHI 0069, PL011 DDI 0183, Generic Timer, PSCI DEN 0022), virtio-1.2-Spezifikation, die QEMU-Doku zur Maschine `vmapple` ([docs/system/arm/vmapple.rst](https://www.qemu.org/docs/master/system/arm/vmapple.html)), öffentliche Apple-API-Dokumentation. |
| **[Annahme]** | Plausibel, aber **nicht belegt**. Darauf darf Code nur aufbauen, wenn er die Annahme in einer Konstante/Doku-Kommentar trägt und leicht änderbar ist. |
| **[Spec nötig]** | Muss als Clean-Room-Spezifikation (`docs/specs/platform/…`, ADR-0008) entstehen, bevor implementiert wird. Der Implementierer erfindet keine Adressen und Registerlayouts. |

Was die QEMU-Doku **tatsächlich** sagt (Stand der Abfrage), und worauf wir uns damit stützen dürfen:

* `vmapple` bildet das Gerätemodell nach, das Virtualization.framework Apple-Silicon-Gästen zeigt; QEMU nutzt dafür
  keinen Apple-Code aus dem Framework. QEMU unterstützt in der dokumentierten Fassung nur macOS-12-Gäste.
* Das Boot-ROM ist Apples `AVPBooter.vmapple2.bin` aus den Ressourcen von Virtualization.framework (`-bios`).
* Es gibt zwei Speicherabbilder, **aux** und **root**. Beide werden doppelt angebunden: als `pflash`-Laufwerk **und**
  als `vmapple-virtio-blk-pci` mit `variant=aux` bzw. `variant=root`. Beim aux-Image wird der erste 0x4000-Byte-Block
  (Metadaten) abgeschnitten.
* Netzwerk ist ein normales `virtio-net-pci`. Die Maschine bekommt eine **UUID** (`-M vmapple,uuid=…`) als Maschinen-ID.
* **Alle** dort genannten virtio-Geräte sind **PCI**-Geräte.

Alles darüber hinaus (Speicherkarte, IRQ-Nummern, Apple-Geräte-Register) ist **[Spec nötig]**. Ich habe Registerwerte
nicht aus dem Gedächtnis „ergänzt“.

## 1. Stand der Implementierung (WP-M0-PLAT)

| Baustein | Pfad | Inhalt |
|---|---|---|
| MMIO-Abstraktion | `ritsu-platform::mmio` | `MmioDevice`-Trait, `AccessSize`, `MmioBus` (BTreeMap, Überlappung/Nulllänge/Überlauf werden abgelehnt, ungemappt/fehlausgerichtet/regionsüberschreitend → Fehler) |
| UART | `ritsu-platform::pl011` | PL011 nach ARM DDI 0183: `UARTDR`→`ByteSink`, `UARTFR`, ID-Register, Konfigurationsregister als Speicher |
| Konfiguration | `ritsu-platform::machine` | `MachineConfig` + Validierung (RAM 2–64 GiB, 16-KiB-Vielfaches; 1–8 vCPUs; **Grenzen sind [Annahme]**) |
| Virtqueue | `ritsu-virtio::queue` | Split-Virtqueue (virtio 1.2 §2.7) mit validierendem Deskriptorketten-Walker, `push_used`, „broken“-Zustand |
| Geräte-Trait | `ritsu-virtio::device` | `DeviceId` (Spec-IDs), `VirtioDevice`-Skizze, `DeviceError` |
| CLI | `ritsu-vmm` | std-only Parser → `MachineConfig`; Start meldet „Boot nicht implementiert“, Exit-Code 2 |

Alles andere in diesem Dokument ist Entwurf.

## 2. Maschinenmodell im Überblick

```
                   ┌──────────────────────────── ritsu-vmm (Host-Prozess) ───────────────────────────┐
 vCPU-Thread 0..N  │  ritsu-dbt (T0/T1/T2)  ──MMIO/IRQ-Check──►  ritsu-platform                         │
 (P-Kerne)         │        │ GuestMemory (ritsu-mem)             ├─ MmioBus ─► PL011, PL031, pvpanic,   │
                   │        ▼                                     │             Apple-Geräte (cfg, aes…) │
                   │   Gast-RAM  ◄───────── DMA ──────────────────├─ GICv3 (GICD, GICR[n], ICC_*-Hook)  │
                   │                                              ├─ Generic Timer, PSCI                 │
 Geräte-/IO-Thread │  ritsu-virtio: PCIe-Host (ECAM) + virtio-pci ├─ PCIe-Host-Bridge ► virtio-blk/net/…  │
 (E-Kerne)         │  Host-Backends: Datei/Block, tap/NAT, evdev, PipeWire                               │
                   │  GPU-Pfad (ritsu-gpu-host) – eigenes Dokument gpu.md                                │
                   └─────────────────────────────────────────────────────────────────────────────────────┘
```

Der Gast läuft in EL1 (Kernel) und EL0. **[Annahme]:** Es gibt keine Gast-sichtbaren EL2/EL3. Das ist die
Voraussetzung dafür, dass der DBT weder Virtualisierungserweiterungen noch Secure Monitor emulieren muss. Ob
macOS auf `vmapple` EL2 erwartet (z. B. für eigene Hypervisor-Nutzung), ist **[Spec nötig]** und Teil von S-BOOT-1.

## 3. Geräteliste

| Gerät | Zweck | Öffentlich bekannt? | Status | Ziel-Modul | WP |
|---|---|---|---|---|---|
| ARM GICv3 (Distributor + Redistributoren) | Interrupt-Controller | [öffentlich] Architektur; [Spec nötig] Adressen, ITS ja/nein | offen | `platform::gic` | PLAT-06 |
| ARM Generic Timer (CNTV/CNTP, CNTFRQ) | Zeit, Timer-IRQs | [öffentlich] Architektur; Frequenz [Annahme] 24 MHz | offen | `platform::timer` | PLAT-07 |
| PSCI | SMP-Start, Reset, Power-off | [öffentlich] PSCI-Spec; Conduit (HVC/SMC) [Spec nötig] | offen | `platform::psci` | PLAT-07 |
| PL011 UART | Boot-Konsole | [öffentlich] TRM; Adresse/IRQ [Spec nötig] | **Modell fertig** (Senden) | `platform::pl011` | PLAT-01 |
| PL031 RTC | Echtzeituhr | [öffentlich] TRM; Adresse/IRQ [Spec nötig] | offen | `platform::pl031` | PLAT-08 |
| pvpanic | Gast meldet Absturz | [öffentlich] (QEMU-Schnittstelle); ob Apple dasselbe nutzt [Spec nötig] | offen | `platform::pvpanic` | PLAT-08 |
| PCIe-Host-Bridge (ECAM) | trägt alle virtio-Geräte | [öffentlich] indirekt (alle virtio-Geräte sind PCI); Layout [Spec nötig] | offen | `platform::pcie` | PLAT-09 |
| virtio-blk (`root`, `aux`) | System- und Hilfsspeicher; Apple-Variante `vmapple-virtio-blk` | [öffentlich] Existenz+Varianten; Unterschied zu Standard-virtio-blk [Spec nötig] | Queue fertig, Gerät offen | `virtio::blk` | PLAT-10 |
| virtio-net | Netzwerk | [öffentlich] | offen | `virtio::net` | PLAT-11 |
| Apple-AES-Engine | Hardware-AES für Gast-Kryptografie (Boot, Keybag) | Existenz [öffentlich]; Register [Spec nötig] | offen | `platform::apple::aes` | PLAT-12 ★ |
| Apple-Konfigurationsbereich („cfg“) | Maschinenparameter (UUID, Kernzahl, RAM …) | Existenz [öffentlich]; Layout [Spec nötig] | offen | `platform::apple::cfg` | PLAT-12 ★ |
| Boot-Disk-Schnittstelle „bdif“ | Zugriff des Boot-ROMs auf aux/root (vgl. die doppelte Anbindung als pflash+virtio) | Existenz [öffentlich]; Protokoll [Spec nötig] | offen | `platform::apple::bdif` | PLAT-12 ★ |
| Paravirtuelle GPU | Metal-Beschleunigung | [Spec nötig]; Entscheidung ADR-0005 (Proposed) | → `gpu.md` | `ritsu-gpu-*` | – |
| Tastatur/Zeiger/Trackpad | Eingabe | siehe §8.3 | offen | `virtio::input` / `platform::usb` | PLAT-13 |
| virtio-sound | Audio | [Annahme] Treiber im Gast, siehe §8.5 | offen | `virtio::sound` | PLAT-14 |
| virtio-entropy, virtio-console | Zufall, serielle Zusatzkonsole | [öffentlich] Geräte; Gast-Treiber [Annahme] | offen | `virtio::rng`, `virtio::console` | PLAT-14 |
| Weitere (Memory-Balloon, vsock, virtiofs, USB-Massenspeicher) | optional | – | später | – | – |

## 4. Speicherkarte

Die Speicherkarte ist **nicht** bekannt. Sie wird so festgelegt:

| Bereich | Basis | Länge | Quelle |
|---|---|---|---|
| Boot-ROM / Flash (pflash?) | TBD | TBD | [Spec nötig] |
| GICv3 Distributor (GICD) | TBD | 64 KiB | Größe [öffentlich] (IHI 0069), Basis [Spec nötig] |
| GICv3 Redistributoren | TBD | 128 KiB je CPU (RD_base + SGI_base) | Größe [öffentlich], Basis [Spec nötig] |
| PL011 | TBD | 4 KiB | [Spec nötig] |
| PL031 | TBD | 4 KiB | [Spec nötig] |
| Apple cfg / AES / bdif | TBD | TBD | [Spec nötig] |
| PCIe ECAM / MMIO-Fenster / IO-Fenster | TBD | TBD | [Spec nötig] |
| RAM | TBD | `MachineConfig::ram_bytes` | Basis [Spec nötig] |

**Wie sie festgelegt wird (Clean Room):**

1. `reverse-engineer` erstellt `docs/specs/platform/memory-map.md` (Kopf nach ADR-0008: Quelle/Methode, macOS-Build,
   Rolle). Bevorzugte Quellen: öffentliche Dokumentation; **Black-Box-Beobachtung** (siehe MMIO-Trace unten);
   Device-Tree, den der Gast selbst ausgibt bzw. den Boot-ROM/Kernel verwendet, soweit er beim Boot sichtbar wird.
2. `platform-engineer` implementiert `MachineMap` (eine Konstanten-Tabelle, ein Ort, ein Test, der Überlappungen
   über `MmioBus::register` prüft) **erst nach** der Spezifikation.
3. **MMIO-Trace-Modus** (WP-PLAT-05): `MmioBus` kann bei Zugriffen auf *unmapped* Adressen statt eines Fehlers
   protokollieren und RAZ/WI liefern. So lässt sich beobachten, welche Adressen ein echter Gast-Boot anfasst, ohne dass
   jemand Apple-Code liest. Der Modus ist ausdrücklich ein Diagnose-Schalter (nie Standard: „niemals still falsch“).

## 5. Interrupt-Architektur

### 5.1 GICv3

[öffentlich] (ARM IHI 0069): Der Gast-Kernel erwartet GICv3. Bestandteile:

* **Distributor (GICD)**: ein Registerblock; verwaltet SPIs (INTID 32–1019): Enable, Priorität, Ziel (Affinity-Routing,
  `GICD_IROUTER`), Gruppe, Pending/Active.
* **Redistributor (GICR)**: einer pro vCPU, je zwei 64-KiB-Frames (`RD_base`, `SGI_base`); verwaltet SGIs (0–15) und
  PPIs (16–31) dieser CPU; `GICR_WAKER` (Prozessor-schlafend-Bit), `GICR_TYPER` (Affinität, „Last“-Bit).
* **CPU-Interface**: wird bei GICv3 **nicht per MMIO**, sondern über Systemregister (`ICC_IAR1_EL1`, `ICC_EOIR1_EL1`,
  `ICC_PMR_EL1`, `ICC_SGI1R_EL1`, `ICC_SRE_EL1` …) bedient. Diese Zugriffe fängt der DBT als `MRS`/`MSR`-Hook ab und ruft
  die GIC-Implementierung. Das ist eine **Schnittstelle zu `ritsu-dbt`** (offen, siehe §14).
* **ITS/LPIs (MSI)**: **[Spec nötig]**, ob `vmapple` einen ITS besitzt. Davon hängt ab, ob virtio-pci-Geräte MSI-X über
  LPIs melden oder klassische INTx-Leitungen auf SPIs nutzen. Das ist die wichtigste IRQ-Frage für PLAT-06/09. Bis
  geklärt, wird INTx (SPI) implementiert; ein ITS nur bei Nachweis.
* Standard-Aufteilung (**[öffentlich]**, ARM BSA-Konvention; Nutzung durch macOS **[Annahme]**): PPI 27 = virtueller
  Timer, PPI 30 = nicht-sicherer EL1-Physical-Timer, PPI 26 = Hypervisor-Timer (nur falls EL2 sichtbar), PPI 25 =
  Maintenance. **SPIs** gehören den Geräten (UART, RTC, PCIe-INTx, Apple-Geräte); die Nummern sind **[Spec nötig]**.

### 5.2 Wie ein Interrupt in den DBT kommt

Ziel: Geräte-Threads und Timer lösen Interrupts aus, ohne den schnellen Code-Pfad zu verlangsamen.

```
Gerät/Timer ──► Gic::set_spi_level(n, true)
                  │  berechnet pro Ziel-CPU die höchste anstehende Priorität
                  ▼  (Maske ICC_PMR, laufende Priorität, Gruppen-Enable)
          VcpuShared { irq_line: AtomicBool, fiq_line: AtomicBool, exit_request: AtomicBool, wake: Futex }
                  │  store(Release) + wake, falls die vCPU in WFI parkt
                  ▼
 DBT-Code:  am Block-Ende / Rücksprung   cmp byte [state+EXIT_REQ], 0 ; jne → Dispatcher
 Dispatcher: liest irq_line; ist PSTATE.I == 0 → Exception „IRQ“ nehmen (VBAR_EL1 + Offset, SPSR/ELR setzen)
```

* **Pending-IRQ-Flag an Blockgrenzen:** Interrupts dürfen architekturell an jeder Befehlsgrenze genommen werden; eine
  Blockgrenze ist eine Befehlsgrenze. Latenzschranke = maximale Blocklänge. Deshalb: (a) Blöcke haben eine feste
  Obergrenze an Befehlen, (b) **jeder Rücksprung** (Schleife) prüft das Flag, (c) Blockverkettung (Chaining) prüft es
  ebenfalls, (d) Befehle, die PSTATE.I/F ändern (`MSR DAIFClr`, `ERET`), beenden den Block, damit ein soeben
  freigegebener Interrupt sofort wirkt.
* **Kick:** Ein anderer Thread (Gerät, Timer, IPI/SGI) setzt `exit_request` und weckt den vCPU-Thread, falls er in WFI/WFE
  schläft (Futex/`parking`). Der Kick ist `Release`, das Lesen im DBT `Acquire`/`Relaxed` plus Barriere im Dispatcher.
* **WFI:** Der Dispatcher prüft Interrupt- und Timerzustand; ohne anstehende Ereignisse parkt der Thread bis zum
  nächsten Timer-Deadline oder Kick. Das spart Host-CPU im Leerlauf (wichtig für Akkulaufzeit).
* **Level vs. Edge:** Geräte melden Pegel; das GIC speichert Pending für Edge-Quellen, hält Pegel für Level-Quellen
  (`GICD_ICFGR`). Interne Schnittstelle: `IrqLine::set(bool)` pro Quelle.
* **Gast-Fehler:** GIC-Registerzugriffe mit falscher Größe/Offset → `MmioError`, nie Panic. Ein Gast, der
  unsinnige Prioritäten setzt, kann nur sich selbst stören.

## 6. Generic Timer und Host-Uhr

* [öffentlich] ARM Generic Timer: `CNTFRQ_EL0`, `CNTVCT_EL0`, `CNTV_CTL/CVAL/TVAL_EL0`, `CNTP_*`, `CNTKCTL_EL1`.
* **Frequenz:** **[Annahme]** 24 MHz (so meldet Apple-Silicon-Hardware und damit auch Virtualization.framework). Die Frequenz
  ist eine Konstante `TIMER_FREQ_HZ` in einem Ort, sie wird auch in cfg/Device-Tree an den Gast gemeldet (**[Spec nötig]**).
* **Host-Uhr:** `CNTVCT = (host_ns_seit_Start * freq / 1e9) - CNTVOFF`. Quelle `CLOCK_MONOTONIC_RAW` über vDSO. Das ist
  monoton, unabhängig von NTP-Anpassungen und billig (kein Syscall). Alternative `RDTSC` mit Kalibrierung nur bei
  `invariant TSC` (spart ~20 ns je Lesen; erst nach Messung, `bench/`).
* **Timer-Auslösung:** Ein Timer-Thread (oder `timerfd` + `epoll`) hält die nächsten `CVAL`-Deadlines aller vCPUs
  (Min-Heap). Bei Ablauf: PPI am Redistributor der Ziel-vCPU setzen (§5.2). vCPUs in WFI nutzen denselben Deadline als
  Park-Timeout.
* **Zeitsprünge:** Pausiert die VM (Snapshot, Host-Suspend), wird `CNTVOFF` angepasst, damit der Gast keine Sprünge sieht
  (macOS reagiert empfindlich auf Zeitsprünge). **RTC (PL031)** liefert die Wanduhr aus `CLOCK_REALTIME` + Offset.
* **Bekanntes Risiko:** Ein Gast, der auf `CNTVCT` in engen Schleifen pollt, belastet den DBT-Slow-Path nicht, wenn
  `MRS CNTVCT_EL0` inline übersetzt wird (T1/T2 Intrinsic). Sonst Timer-Latenz sichtbar. Messen.

## 7. PSCI und SMP-Hochlauf

* [öffentlich] PSCI (ARM DEN 0022): `PSCI_VERSION`, `CPU_ON`, `CPU_OFF`, `AFFINITY_INFO`, `SYSTEM_OFF`, `SYSTEM_RESET`,
  evtl. `CPU_SUSPEND`, `SYSTEM_SUSPEND`.
* **[Spec nötig]:** Ob macOS auf `vmapple` PSCI überhaupt benutzt (Apple-SoCs starten Zweit-CPUs über eigene
  Mechanismen), über **HVC oder SMC**, und welche Funktions-IDs/Parameter. Das ist Teil von S-BOOT-1 / PLAT-07.
* Entwurf: `Psci` ist ein Handler, den der DBT bei `HVC`/`SMC` aufruft (`fn call(&mut self, vcpu, regs) -> PsciResult`).
  `CPU_ON(target_mpidr, entry, context_id)` validiert `target_mpidr` gegen die Topologie (unbekannt → `INVALID_PARAMETERS`,
  bereits an → `ALREADY_ON`), setzt den Zustand der Ziel-vCPU (PC=entry, X0=context_id, EL1, MMU aus) und startet bzw.
  weckt ihren Thread. Alle vCPUs außer 0 starten im Zustand „off“ (Thread parkt).
* **MPIDR** der vCPUs (Aff0/Aff1) und die Topologie kommen aus `MachineConfig::cpus` (Format **[Spec nötig]**).

## 8. MMIO-Bus und Geräte-Dispatch

### 8.1 Pfad

```
Gast-Load/Store ─► DBT: SoftMMU-Translate (Stage-1) ─► GPA
      │ GPA im RAM?  ja ─► direkter Host-Zugriff (schnell)
      └ nein (TLB-Eintrag trägt Flag MMIO) ─► Slow Path:
            ritsu_platform::MmioBus::{read,write}(gpa, AccessSize, value)
               └► BTreeMap-Suche (Basis ≤ gpa) ─► Gerät.read/write(offset, size, value)
            Ergebnis ─► Zielregister  │  Fehler ─► Gast-Exception (externer Abort)
```

* **Implementiert:** `MmioBus` mit `register(base, len, device)`; lehnt Länge 0, Überlauf (`base+len`) und Überlappung ab
  (Vorgänger- und Nachfolger-Prüfung). Zugriffe werden gemaskt (`AccessSize::mask`), **natürlich ausgerichtet**
  verlangt (ARM-Device-Speicher: fehlausgerichtet → Alignment-Fault) und dürfen die Region nicht überschreiten.
* **Nicht Aufgabe des Busses:** Zerlegen von `LDP/STP`, SIMD-Zugriffen und Zugriffen über Seitengrenzen in 1/2/4/8-Byte-Zugriffe
  macht der DBT; Memory-Attribute (nGnRnE …) und Ordnung ebenso.
* **Fehlerpolitik:** `MmioError` → der Aufrufer entscheidet: *Standard* = synchroner externer Abort (`DFSC 0x10`) in den Gast;
  für dokumentierte Lücken (z. B. Reserved-Register) RAZ/WI **im Gerät**, nicht im Bus. Nie ein Panic.
* **Nebenläufigkeit:** Phase 1: `Mutex<MmioBus>` (ein Lock, einfach, korrekt). Phase 2: Bus liefert `Arc<Mutex<dyn MmioDevice>>`
  und sperrt nur das Gerät; die Karte wird beim Start gebaut und danach nur gelesen (lock-frei, `Arc<[Region]>`).
  Der Trait ist deshalb schon `Send`.
* **DMA:** Geräte greifen über `GuestMemory` (eingefroren) auf RAM zu. **Offene Architekturfrage:** `write(&mut self)` zwingt
  zu exklusivem Zugriff; für parallele Gerätethreads braucht es eine `&self`-Variante mit innerer Veränderlichkeit
  (siehe §14, Frage 3).

## 9. virtio

### 9.1 Transport: PCI statt MMIO (Begründung)

Die QEMU-Doku zu `vmapple` bindet **jedes** virtio-Gerät als PCI-Gerät an (`vmapple-virtio-blk-pci`, `virtio-net-pci`). Daraus
folgt **[Annahme, plausibel]:** Der macOS-Gast findet virtio-Geräte über einen PCIe-Host (ECAM) und verwendet
den virtio-pci-Treiber. Deshalb:

* **Primär: virtio-pci (modern, virtio 1.x)** mit PCIe-Host-Bridge → PLAT-09. Das ist auf dem kritischen Pfad
  (ohne Plattenzugriff kein Boot).
* **virtio-mmio** (virtio 1.2 §4.2) bleibt als einfacher Testtransport und als Rückfall, falls die Spec etwas anderes zeigt.
  Ein Gerät (`VirtioDevice`) kennt den Transport nicht – das ist der Zweck der Trait-Trennung.
* Unklar: MSI-X/ITS vs. INTx (§5.1) und die Eigenheiten der `vmapple-virtio-blk`-Variante (andere PCI-IDs? zusätzliche
  Feature-Bits? Config-Space?) – **[Spec nötig]**.

### 9.2 Virtqueue (implementiert)

`ritsu_virtio::queue::Queue` implementiert das Split-Ring-Format aus virtio 1.2 §2.7 und ist als Angriffsfläche gebaut:

* Größe Potenz von 2, ≤ 32768; Ausrichtung 16 (Descriptor), 2 (Avail), 4 (Used); Adressüberlauf wird abgelehnt.
* Kettenlauf: Kopf-/`next`-Index ≥ Größe → Fehler; mehr Deskriptoren als Queue-Einträge → `ChainLoop`
  (jeder Deskriptor kommt höchstens einmal in einer Kette vor); `INDIRECT` → `Unsupported` (kommt später);
  lesbar-nach-schreibbar → Fehler (§2.7.4.2); `addr+len`-Überlauf → Fehler.
* `avail.idx` darf höchstens `size` neue Einträge vorspringen (sonst `AvailOverrun`).
* Nach dem ersten Verstoß ist die Queue **kaputt** (`DEVICE_NEEDS_RESET`, §2.1.2) und liefert nur noch `Broken`. So kann ein
  feindlicher Gast kein Gerät in eine Endlosschleife treiben.
* Nicht implementiert: `EVENT_IDX`, Packed Ring, Indirect. Speicherordnung (Acquire/Release auf `avail.idx`/`used.idx`)
  ist Pflicht des Speicher-Backends (Moduldoku).

### 9.3 Geräte

| Gerät | ID (§5) | Gast-Erwartung | Host-Backend | Hinweise |
|---|---|---|---|---|
| **blk** (`root`, `aux`) | 2 | [öffentlich] `vmapple-virtio-blk` mit `variant=root|aux` | Datei (raw, sparse), später Block-Gerät/Partition | `aux` = NVRAM/Boot-Policy-Zustand, siehe §10.4. Ob TRIM/Flush/Mehrfach-Queues: [Spec nötig]. `O_DIRECT` vs. Page-Cache messen. Flush muss `fdatasync` sein, sonst geht Gast-Dateisystem-Konsistenz verloren. |
| **net** | 1 | [öffentlich] `virtio-net-pci` | Phase 1: `tap` + Host-NAT (nftables, vom Appliance-Image eingerichtet) oder Bridge; Phase 2: Nutzermodus-NAT im Prozess | Wegen „keine neuen Crates“ gibt es keinen fertigen User-Mode-Stack (kein smoltcp/slirp); ein eigener NAT-Stack ist ein eigenes WP (Offene Frage 6). Offloads (checksum/TSO) zuerst aus; Gast-MAC deterministisch aus Maschinen-UUID. |
| **input** | 18 | **[Annahme]: macOS bringt keinen virtio-input-Treiber mit.** Virtualization.framework bietet Tastatur/Zeiger über USB-HID bzw. Apples eigene Geräte | – | Siehe §9.4. |
| **console** | 3 | [Annahme] kaum genutzt; die PL011 ist die Boot-Konsole | pty/Datei | niedrige Priorität |
| **sound** | 25 | [Annahme]: Virtualization.framework hat eine öffentliche virtio-Sound-API (`VZVirtioSoundDevice…`), also existiert ein Gast-Treiber | PipeWire (Host) | Format-/Sample-Raten-Aushandlung beachten; Latenz-Puffer; Mikrofon später |
| **rng/entropy** | 4 | [Annahme] vorhanden (`VZVirtioEntropyDevice…` ist öffentliche API) | `getrandom(2)` | trivial; erste Probe für Queue+Transport (PLAT-10 Vorstufe) |
| gpu | 16 | **nicht** Apples Paravirt-GPU (anderes, undokumentiertes Protokoll) | – | nur ID reserviert; ADR-0005 |

### 9.4 Eingabe

Das ist ein echtes Risiko, weil die naheliegende Wahl (virtio-input) im Gast **vermutlich nicht funktioniert**:

* **Option A – USB-HID über XHCI:** [öffentlich] Virtualization.framework stellt Tastatur und Zeigegerät als USB-HID-Geräte
  bereit; macOS hat dafür Treiber. Der Preis ist ein XHCI-Controller-Modell (groß, aber öffentlich spezifiziert, Intel xHCI 1.2).
* **Option B – Apples eigene Geräte** (Mac-Tastatur, Mac-Trackpad mit Multitouch seit macOS 13 im Framework):
  **[Spec nötig]**; nur sie liefern echte Multitouch-Gesten des Touchpads. Ohne sie gibt es nur Zeiger + Scrollrad.
* **Option C – virtio-input**: nur, wenn Spike S-INPUT-1 einen Gast-Treiber findet.

Empfehlung: **A zuerst** (erreicht „Maus und Tastatur gehen“), B als spätere Komfortstufe, C nur bei Nachweis. Das Host-Ende ist in
allen Fällen dasselbe: libinput/evdev → normalisierte Ereignisse → Gerätemodell (Tastatur-Keycodes HID-Usage-basiert, absolute
Zeigerkoordinaten vs. relativ; Pointer-Capture und Fokuswechsel im Kiosk-Compositor, siehe `host-os.md`).
Touchpad des G5 MF (I2C-HID, Linux `hid-multitouch`) liefert Mehrfinger-Kontakte; in Option A/C gehen sie als Zeiger+Scroll
(libinput-Gestenübersetzung), nur in Option B als echte Touch-Daten.

## 10. Boot-Fluss

### 10.1 Zwei Wege

```
Weg 1 (Standard-Hypothese, QEMU-nah):
  Nutzer-Boot-ROM (AVPBooter.vmapple2.bin, nie ausgeliefert) ──► lädt iBoot/Kernelcache aus root/aux ──► XNU
Weg 2 (eigener Lader, Spike S-BOOT-1):
  ritsu-loader ──► liest Kernelcache aus root, baut Boot-Argumente/Device-Tree selbst, springt in XNU (EL1)
```

* **Weg 1:** Das ROM ist Apple-Code, der Nutzer liefert ihn (`--boot-rom`), Ritsu verteilt ihn nie und hält ihn nicht im
  Repo (ADR-0004/0008). Das ROM spricht mit `bdif`, `cfg`, AES und liest aux/root; alle drei sind **[Spec nötig]**.
  Der ROM-Code läuft wie jeder andere Gast-Code durch den DBT.
* **Weg 2:** Eigenes Laden macht uns unabhängig vom ROM, ersetzt aber seine Sicherheitsprüfungen (Signaturkette,
  Boot-Policy aus aux, Kernelcache-Personalisierung). Ob der VM-Kernelcache ohne diese Prüfungen startet, ist offen.

### 10.2 Spike S-BOOT-1 (★)

Frage: Kann ein eigener Lader den vmapple-Kernelcache starten, oder braucht es das Boot-ROM?
Methode: (1) Weg 1 zuerst zum Laufen bringen (DBT + PL011-Trace); (2) beobachten, welche Geräte/Register das ROM vor dem
Kernel anfasst (MMIO-Trace, §4); (3) daraus Spec erzeugen; (4) Minimal-Lader gegen diese Spec bauen.
Ergebnis: Bericht `docs/research/s-boot-1.md` + Spec `docs/specs/platform/boot-handoff.md`. Abbruchkriterium: Wenn das ROM für
Boot-Policy zwingend ist, bleibt Weg 1 der einzige Weg (Nutzer liefert ROM: schon das Ausgangsszenario).

### 10.3 Installation aus der IPSW (Spike S-BOOT-2, ★)

Apple installiert in `Virtualization.framework` über `VZMacOSInstaller` auf einem **Mac**: Restore einer IPSW in ein Disk-Image,
mit Personalisierung (Apple-Signaturserver, „TSS“) pro Maschinen-ID.

* **Frage S-BOOT-2:** Lässt sich der Restore Host-seitig ohne Mac nachbauen (IPSW entpacken, Disk-Image schreiben, Personalisierung)? Hauptproblem:
  Die Personalisierung ist eine Apple-Server-Interaktion (Signaturticket an die Maschinen-ID gebunden). **[Spec nötig]**, ob und wie das ohne Mac geht,
  und ob das rechtlich/technisch tragbar ist (**Rechtsfrage → Mensch, nicht der Agent**).
* **Rückfall (Phase 1, entspricht dem Vorgehen der QEMU-Doku):** Der Nutzer erzeugt Disk-Image **und** aux-Image einmalig auf einem eigenen Mac
  (mit Virtualization.framework, z. B. über eine öffentliche CLI), kopiert beides samt Boot-ROM und Maschinen-UUID. Ritsu bietet dafür ein
  Import-Werkzeug (Prüfsummen, Format-Sniffing, aux-Header abschneiden). Die erste Version von Ritsu **setzt diesen Rückfall voraus**.
* Auch der Rückfall hat eine Hürde: Das Image gehört zu einer Maschinen-ID; Ritsu muss exakt dieselbe UUID und denselben cfg-Inhalt melden (§10.5).

### 10.4 Aux-Speicher und NVRAM

* Das **aux-Image** enthält Boot-Policy/NVRAM-artigen Zustand (laut QEMU-Doku „Metadaten“, der erste 0x4000-Byte-Block wird dort verworfen).
  Das Format ist **[Spec nötig]**. Ritsu behandelt es zunächst als undurchsichtige Blockdatei, die der Gast schreibt (kopiert beim Start
  als Arbeitskopie, Original bleibt für Rollback).
* Persistenz: Schreibzugriffe gehen in die Datei; Flush bei Gast-Flush und beim Beenden. Sicherung: vor jedem Boot optional Reflink-Kopie.
* Klärung nötig: Ist die `pflash`-Doppelanbindung nur QEMU-Praxis oder Teil des Apple-Modells, d. h. sieht das ROM Flash und der Kernel
  virtio-blk? (**[Spec nötig]**, Teil von `bdif`.)

### 10.5 Maschinen-Identität

* Maschinen-UUID als Kommandozeilenoption (`--uuid`, WP-PLAT-05; heute nicht implementiert). **[Öffentlich]:** QEMU übergibt sie mit `uuid=`.
* ECID/Seriennummer/weitere IDs: **[Spec nötig]**. Wir erfinden keine Apple-Seriennummern; Ziel ist keine iCloud-Spoofing-Funktion
  (Nicht-Ziel in ARCHITECTURE.md §8).

## 11. vCPU-Threading

* **Ein Host-Thread pro vCPU**, fest an einen **P-Kern** gepinnt (physischer Kern, ohne SMT-Geschwister; der i5-12500H hat 4 P-Kerne → Standard
  `DEFAULT_CPUS = 4`). Siehe `host-os.md` zur Topologie-Erkennung.
* Weitere Threads: Timer-Thread, I/O-Thread(s) für Block/Netz (E-Kerne), Eingabe-Thread, JIT-Compiler-Threads (E-Kerne),
  GPU-Thread(s) (`ritsu-gpu-host`).
* **Gemeinsamer Zustand:** GIC und `MmioBus` hinter Locks (§8.1); Gast-RAM über `GuestMemory`. Speicherordnung des Gastes (schwach) vs. x86-TSO
  ist Sache des DBT (ARCHITECTURE.md §5.1).
* **Anhalten/Stop-the-world** (Snapshot, Reset, Debugger): `exit_request` bei allen vCPUs, Barrier bis alle im Dispatcher stehen.
* **Fehlerbehandlung:** Panic in einem vCPU-Thread beendet kontrolliert die VM (kein stilles Weiterlaufen mit halber Maschine); Gast-Fehler sind
  keine Panics.

## 12. Snapshot / Save-State (später)

Nicht Teil von M0–M2. Entwurf: Jedes Gerät implementiert später `save(&self, w) / restore(&mut self, r)` mit **Versionsnummer** und eigenem Block.
vCPU-Zustand (Register, Systemregister, PAC-Schlüssel), GIC, Timer, Geräte, RAM (Seiten-Delta, komprimiert), Disk-Overlay (copy-on-write).
Voraussetzung: alle Geräte sind deterministisch serialisierbar – darum kein versteckter Zustand in Host-Handles; Host-Backends werden beim Restore neu geöffnet.
Der Übersetzungs-Cache gehört **nicht** in den Snapshot.

## 13. Sicherheit

* **Alle Gast-Eingaben sind feindlich** (CLAUDE.md §3): MMIO-Offsets/-Größen/-Werte, DMA-Deskriptoren, Config-Space, Block-Anfragen (Sektor + Länge),
  Netzwerkframes (Länge ≤ MTU+Header, keine Weitergabe unvalidierter Offloads), Eingabeereignisse.
* **Keine `panic!`/`unwrap`/ungeprüfte Indizes** auf Gast-Daten; Fehler → `ritsu_core::Error::Guest` oder Gast-Exception (Konvertierungen
  `MmioError`/`QueueError`/`DeviceError` → `Error` existieren).
* **Ressourcenschranken:** Pro Anfrage Maximalgröße (Block-Lesen: Länge begrenzt, Sektorbereich gegen Imagegröße geprüft), Kettenlänge ≤ Queue-Größe,
  Warteschlangenlängen begrenzt; ein Gast darf Host-RAM nicht beliebig anfordern.
* **Dateizugriff:** Disk-Images öffnet der vmm selbst; der Gast liefert nie Pfade. Prozess läuft mit minimalen Rechten (seccomp-Allowlist, kein `exec`),
  sobald die Syscall-Menge stabil ist (PLAT-15).
* **Fuzzing (Q-SEC-1):** Ziele für `fuzz/` (Eigentum `test-engineer`): `MmioBus`+PL011 (zufällige Zugriffsfolgen), Queue-Walker (zufällig gefüllte Ringe), später
  virtio-blk/net-Request-Parser, GIC-Registerfolgen. Heute decken Unit-Tests Schleifen, Bereichsfehler und Grenzfälle ab.
* **Geheimnisse:** Boot-ROM, aux (enthält ggf. maschinengebundene Daten) und Images haben Dateirechte 0600 und liegen auf der Datenpartition.

## 14. Work Packages

★ = Hochrisiko (zuerst als Spike). „Dep“ = Voraussetzung. Abnahme = überprüfbares Kriterium.

| WP | Titel | Dep | Risiko | Abnahme |
|---|---|---|---|---|
| **PLAT-01** | MMIO-Bus, PL011, `MachineConfig` | – | niedrig | **erledigt in WP-M0-PLAT**; Tests grün |
| **PLAT-02** | Split-Virtqueue + `VirtioDevice`-Skizze | – | niedrig | **erledigt in WP-M0-PLAT**; Tests (Schleifen, Bereichsfehler) grün |
| **PLAT-03** | **Spike S-BOOT-1:** eigener Lader vs. Boot-ROM | PLAT-05, PLAT-06, PLAT-07, DBT-Interpreter | ★★★ | Bericht + `boot-handoff`-Spec; Entscheidung ADR-Vorschlag |
| **PLAT-04** | **Spike S-BOOT-2:** IPSW-Restore ohne Mac | – (parallel, nur Doku/Recherche) | ★★★ | Bericht: machbar/nicht machbar + Rechtshinweis; sonst Import-Rückfall bestätigt |
| **PLAT-05** | Speicherkarte (Spec → `MachineMap`), MMIO-Trace-Modus, `--uuid` | Spec `memory-map` (reverse-engineer) | ★★ | Test: alle Regionen überlappungsfrei; Trace listet Zugriffe eines synthetischen Gasts |
| **PLAT-06** | GICv3: GICD/GICR, ICC-Hook-Schnittstelle, SGI/PPI/SPI, Affinity-Routing | PLAT-05; DBT-Hook (Architect-Schnittstelle) | ★★ | Unit-Tests gegen GIC-Spec-Szenarien (Enable, Priorität, Maske, EOI, SGI); IRQ erreicht Test-vCPU |
| **PLAT-07** | Generic Timer + Host-Uhr + PSCI | PLAT-06 | ★★ | Timer-IRQ pünktlich (±1 ms) in Test; `CPU_ON` startet Zweit-vCPU im Test-Gast |
| **PLAT-08** | PL031, pvpanic, Reset/Power-off-Pfad | PLAT-05 | niedrig | Register-Tests; `SYSTEM_OFF` beendet vmm sauber |
| **PLAT-09** | PCIe-Host (ECAM, BARs, Capabilities) + virtio-pci-Transport (modern) | PLAT-05, PLAT-06 | ★★ | Enumeration eines Test-Gasts sieht Geräte; Feature-Aushandlung korrekt; Fuzz-Ziel vorhanden |
| **PLAT-10** | virtio-blk (root/aux, Apple-Variante) + Image-Backend | PLAT-02, PLAT-09; Spec Variante | ★★ | Lese-/Schreib-/Flush-Tests gegen Datei; Bereichs-/Überlauffehler; Durchsatzmessung |
| **PLAT-11** | virtio-net + tap-Backend | PLAT-09 | ★ | Loopback-Test; Ping aus Test-Gast; Länge/Offload-Validierung |
| **PLAT-12** | Apple-Geräte: cfg, AES, bdif | Specs (reverse-engineer) | ★★★ | Gast-ROM liest cfg; AES-Test-Vektoren (öffentlich: AES-FIPS-197); bdif liefert Blöcke |
| **PLAT-13** | Eingabe (Option A: XHCI + HID, Host: evdev/libinput) | PLAT-09; PLAT-19 | ★★ | Tastendruck/Mausbewegung kommt im Test-Gast als HID-Report an |
| **PLAT-14** | sound, entropy, console | PLAT-09 | ★ | Entropy liefert Bytes; Sound-Playback hörbar (manuell, am Gerät) |
| **PLAT-15** | vmm-Verdrahtung: vCPU-Threads, Pinning, Run-Loop, IRQ-Zustellung, seccomp | DBT lauffähig, PLAT-06/07 | ★★ | Mehr-vCPU-Test-Gast läuft, Shutdown-Test, Latenz-Messung |
| **PLAT-16** | Fuzz-Ziele (mit `test-engineer`) | PLAT-01/02, PLAT-09/10 | ★ | Ziele bauen, 1 h Lauf ohne Befund in CI |
| **PLAT-17** | Snapshot/Save-State | PLAT-15 | ★★ | Save→Restore→Gast läuft weiter |
| **PLAT-18** | Image-Import-Werkzeug (Rückfall S-BOOT-2) | PLAT-04 | ★ | Format-Sniffing, Prüfsumme, aux-Header; Fehlertests |
| **PLAT-19** | **Spike S-INPUT-1:** Hat macOS 27 einen virtio-input-Treiber? Welche Eingabegeräte erkennt der `vmapple`-Kernel? | PLAT-09 | ★★ | Bericht; Entscheidung A/B/C aus §9.4 |

Reihenfolge-Empfehlung: PLAT-04 (Recherche) und PLAT-05-Spec parallel zu DBT-M1; dann 05→06→07→08; danach 09→10 (damit ein Boot-ROM Platten sieht); danach 03.

## 15. Offene Fragen

1. **Speicherkarte, IRQ-Nummern, PCIe-Layout** (Spec `memory-map`) – reverse-engineer. Ohne sie bleibt alles TBD.
2. **ITS vorhanden?** MSI-X über LPI oder INTx über SPI? (§5.1) – bestimmt den GIC-Umfang.
3. **`GuestMemory::write(&mut self)`:** Für parallele Gerät-/vCPU-Threads braucht es eine Variante mit `&self` und innerer Veränderlichkeit (oder ein
   `Arc<GuestRam>` mit expliziten Atomics). Schnittstelle gehört `architect`/`dbt-engineer` (Memory-Crate ist eingefroren).
4. **DBT-Schnittstelle für ICC-Systemregister, `HVC`/`SMC`, `WFI`:** Wer definiert `trait SysRegHandler` / `trait Hypercall`? (architect)
5. **PSCI-Conduit und -Nutzung durch macOS** (HVC/SMC, ob überhaupt).
6. **Netzwerk ohne neue Crates:** eigener Mini-NAT oder tap/Bridge im Appliance? (Eine neue Abhängigkeit braucht Begründung.)
7. **Eingabe:** Gast-Treiber für virtio-input vorhanden? XHCI-Aufwand akzeptabel? Apples Trackpad-Gerät (Spec) für Gesten?
8. **Frequenz und Konfiguration der Timer** (24 MHz?) und Zeitquelle des Gastes bei Pause.
9. **Rechtlich (Mensch):** Host-seitiger IPSW-Restore und Personalisierung (S-BOOT-2); Umgang mit Maschinen-ID. Stoppen und fragen, nicht umgehen.
10. **EL2 im Gast?** Falls macOS auf vmapple EL2 erwartet (Hypervisor.framework im Gast / Nested), wäre das ein sehr großer DBT-Zusatz.
11. **Minimaler RAM** für macOS 27 (Konstante `MIN_RAM_BYTES = 2 GiB` ist geraten).
12. **Maximale vCPU-Zahl** (`MAX_CPUS = 8` geraten) und Topologie/MPIDR-Format.
