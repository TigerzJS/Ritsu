# ADR-0010: DBT und Plattform sind über synchrone vCPU-Hooks (Systemregister, MMIO), einen Exit-basierten Run-Loop (HVC/SMC, WFI, Stopp) und atomare `VcpuSignals` gekoppelt

* **Status:** Proposed
* **Datum:** 2026-10-14
* **Entscheider:** Mensch (Freigabe), `architect` (Entwurf, WP-ARCH-02)
* **Betrifft:** `crates/ritsu-dbt` (Eigentümer aller Schnittstellentypen), `crates/ritsu-platform` (Implementierung:
  GIC, Timer, PSCI, MMIO-Bus, vCPU-Runner), `crates/ritsu-vmm` (Threads, Pinning, Verdrahtung), `tests/`, `bench/`

## Kontext

**Problem.** `ritsu-dbt` führt jeden Gast-Befehl aus, kennt aber keine Geräte (ARCHITECTURE.md §4). Mehrere
Gast-Vorgänge enden trotzdem in der Plattform: Systemregister des GICv3-CPU-Interface (`ICC_*`), die Vergleichsregister
des Generic Timer (`CNTV_*`, `CNTP_*`), Lade- und Speicherzugriffe auf Nicht-RAM (MMIO), `HVC`/`SMC` (PSCI), `WFI`
(Leerlauf) sowie Interrupts, die Geräte-, Timer- oder andere vCPU-Threads auslösen. Die Abhängigkeit darf nur
`ritsu-platform → ritsu-dbt` laufen. Alle Typen der Grenze gehören deshalb `ritsu-dbt` (oder tiefer), und die Plattform
implementiert sie.

**Ist-Zustand.** Für die Grenze gibt es noch keinen Code. Was existiert, ist ein Satz Doku-Skizzen, die sich
widersprechen:

| Thema | dbt.md | platform.md | Widerspruch |
|---|---|---|---|
| Wo liegen `CNTV_*`/`CNTP_*`? | §10.4: „Registerzustand im DBT; das Erreichen der Deadline meldet der DBT an `ritsu-platform`“ | §6: Ein Timer-Thread hält die Deadlines aller vCPUs (Min-Heap) und setzt die PPI | Wer kennt die Deadline, wer setzt die PPI? |
| Zähler | §10.4: inline aus invariantem TSC | §6: `CLOCK_MONOTONIC_RAW`, TSC erst nach Messung | Zwei Zeitquellen ergeben zwei Gast-Zeiten |
| `WFI` | §10.5: vCPU-Thread parkt im DBT, Timeout = Timer-Deadline | §5.2: „Dispatcher … parkt bis zum nächsten Timer-Deadline“ | Der DBT kennt die Deadline nur, wenn er den Timer besitzt |
| PSCI | – | §7: „`Psci` ist ein Handler, den der DBT bei `HVC`/`SMC` aufruft (`fn call(&mut self, vcpu, regs)`)“ | `CPU_ON` und `SYSTEM_RESET` greifen auf andere vCPUs und auf den Lebenszyklus zu, was ein Callback im vCPU-Thread nicht darf |
| Offene Frage | §1, §10.1: Trait `GicCpuInterface` | §15 Frage 4: „Wer definiert `trait SysRegHandler` / `trait Hypercall`?“ | – |

**Anforderungen**

| ID | Anforderung | Quelle |
|---|---|---|
| R-1 | Keine neue Crate-Kante. `ritsu-dbt` sieht nur `ritsu-mem` und `ritsu-core`. | ARCHITECTURE.md §4, `tools/check_crate_graph.py` |
| R-2 | **Präzise Exceptions:** Ein unbekanntes oder nicht freigegebenes Systemregister wird zu UNDEFINED am Befehl (nie still ignoriert). Ein MMIO-Fehler wird zum synchronen Data Abort mit korrektem `ESR_EL1`/`FAR_EL1`, und der Befehl hat dann nichts geschrieben. | dbt.md §10.1, §10.2; R-DBT-01 P1-2 |
| R-3 | **Fastpath kostenlos:** RAM-Zugriffe (Inline-TLB) und DBT-eigene Systemregister zahlen nichts für die Grenze. Die Interrupt-Prüfung kostet höchstens ein makrofusioniertes `cmp`+`jcc` pro Blockeintritt bzw. Rückwärtskante. | dbt.md §5.2, §7.3 |
| R-4 | **Interrupt-Latenz:** höchstens ein Block, solange die vCPU läuft. Parkt sie (`WFI`), verbrennt sie keine Host-CPU und wacht trotzdem schnell auf. | dbt.md §10.3, platform.md §5.2 |
| R-5 | Andere Threads (Geräte, Timer, vCPUs mit SGI, TLBI-Rundrufe) melden Ereignisse **ohne verlorene Wakeups** und ohne Data Race im Rust-Speichermodell. | ADR-0009 (Fakten zum Rust-Speichermodell) |
| R-6 | **PSCI** mit `CPU_ON` (Ziel ist eine andere vCPU), `SYSTEM_OFF`, `SYSTEM_RESET`. Der Conduit (HVC oder SMC) und ob macOS PSCI überhaupt nutzt, sind **[Spec nötig]**. Klärt die Spec das, darf sich am DBT nichts ändern. | platform.md §7 |
| R-7 | Gast-Eingaben sind feindlich: kein Panic. Was die Plattform erreicht, ist vorgeprüft (Ausrichtung, Rechte, Registerkodierung). | CLAUDE.md §3 Regel 2 |
| R-8 | **Getrennt testbar:** DBT-Tests (Mini-Kernel aus WP-DBT-07/08, Linux-User-Harness aus WP-DBT-03) laufen ohne `ritsu-platform`, Plattform-Tests ohne JIT. | ARCHITECTURE.md §4 („unabhängig entwickel- und testbar“) |
| R-9 | Alle vCPUs anhalten (Stop-the-world), Snapshot, Reset und Shutdown funktionieren mit präzisem Architekturzustand. | platform.md §11, §12 |

**Fakten mit Quelle**

* **`ICC_PMR_EL1`-Writes sind selbstsynchronisierend.** Nach einem architektonisch ausgeführten Write darf kein
  Interrupt unterhalb der neuen Priorität mehr genommen werden, und zwar ohne `ISB` und ohne Exception-Grenze.
  Kodierung: op0 = 3, op1 = 0, CRn = 4, CRm = 6, op2 = 0. Quelle: Arm System Register XML, `ICC_PMR_EL1`
  ([Spiegel](https://df.lth.se/~getz/ARM/SysReg/AArch64-icc_pmr_el1.html)).
* **`ESR_EL1.ISV` ist für unseren Gast immer 0.** Für Data Aborts, die in `ESR_EL1` gemeldet werden, ist ISV nur bei
  Stage-2-Aborts und bei FEAT_LS64-Zugriffen 1, mit FEAT_RAS nie bei synchronen External Aborts. Der Gast hat kein
  EL2 (dbt.md E-1), und LS64 wird nicht gemeldet. Mit `ISV = 0` ist `IL = 1`. `DFSC = 0b010000` ist *Synchronous
  External abort, not on translation table walk or hardware update of translation table*. Quelle: Arm DDI 0487 M.d,
  D24.2.44 `ESR_EL1` (ISV, IL, DFSC) und Pseudocode `AArch64_InstructionSyndromeValid` (J1.2.197), geprüft in
  [R-ARCH-06](../research/R-ARCH-06-ddi0487.md) §3.5. Damit ist die **Annahme** aus R-DBT-01 P1-2 („ob ISV gefüllt
  werden muss“) erledigt: ISV muss 0 sein. *(Quelle ersetzt durch R-ARCH-06.)*
* **Ohne EL2 ist `HVC` UNDEFINED, ohne EL3 ist `SMC` UNDEFINED, auf EL0 und EL1 bedingungslos.** Die Steuerbits
  `HCR_EL2.HCD`, `SCR_EL3.HCE`/`SMD` und der Trap `HCR_EL2.TSC` existieren dann nicht. Es entsteht eine Undefined
  Instruction Exception nach EL1, EC 0x00, `ELR` = Befehl. Quelle: Arm DDI 0487 M.d, C6.2.175 `HVC`, C6.2.372 `SMC`,
  D1.4.9 `R_ZVTPR`/`R_HMXQS`, Pseudocode `AArch64_CheckForSMCUndefOrTrap` (J1.2.179), geprüft in
  [R-ARCH-06](../research/R-ARCH-06-ddi0487.md) §3.1. Emulatoren (z. B. QEMU für PSCI) behandeln `HVC`/`SMC`
  trotzdem als Aufruf einer unsichtbaren Firmware-Ebene. Ritsu tut das auch (`HypercallOutcome::Handled`). Das ist
  eine **bewusste Modellabweichung**: `ID_AA64PFR0_EL1.EL2/EL3` bleiben 0, und alles, was die Plattform nicht als
  angekündigten Firmware-Aufruf erkennt, endet als `Undefined`, also genau im Architekturergebnis.
  *(Quelle ersetzt durch R-ARCH-06.)*
* **`WFI` wacht auch bei einem per `PSTATE.{A,I,F}` maskierten, anstehenden Interrupt auf, `WFE` nicht.** `WFE`
  wacht außerdem durch `SEV` eines beliebigen PE, den Event-Stream und das **Löschen des globalen Monitors** (ein
  fremder Schreibzugriff auf den per `LDXR` markierten Block). Spinlock-Code darf sich darauf verlassen und beim
  Freigeben auf `SEV` verzichten. Das Event-Register setzen zusätzlich `SEVL` und jede Exception-Rückkehr. Ein
  anstehender Interrupt macht `WFI` zum NOP, bevor die Trap-Prüfung (`SCTLR_EL1.nTWI`) greift. Quelle: Arm DDI 0487
  M.d, D1.7.2.1 `R_VRLPB`/`R_NLVGY`, D1.7.1.4 `R_KBMFC`, D1.7.1.1 `R_XRZRK`, B2.12.2.1.1, B2.12.6, Tabelle D1-22,
  Pseudocode `Hint_WFI`/`Hint_WFE`, geprüft in [R-ARCH-06](../research/R-ARCH-06-ddi0487.md) §3.2. dbt.md §10.5
  nennt für `WFE` „Interrupts“ pauschal. Richtig ist dort „nicht maskierte Interrupts“.
  *(Quelle ersetzt und ergänzt durch R-ARCH-06.)*
* **Mehrfachzugriffe dürfen teilweise schreiben und wiederholt werden.** Löst ein Befehl mit mehr als einem
  single-copy-atomaren Zugriff (`STP`, `ST1`–`ST4` mit mehreren Registern, seitenübergreifend unausgerichtete
  Zugriffe) einen synchronen Data Abort aus, dürfen die nicht fehlschlagenden Ziele bereits geschrieben (architektonisch
  UNKNOWN) sein. Die fehlschlagende Stelle und die Adressregister bleiben unverändert. Das gilt für Normal- und
  Device-Speicher. Nach der Rückkehr wird der Befehl wiederholt, auch seine Device-Zugriffe. Ein **einzelner**
  single-copy-atomarer Device-Zugriff darf dagegen nicht wiederholt werden. Quelle: Arm DDI 0487 M.d, D1.4.1.4
  `R_TNVSL`, B2.2.1, B2.10.2 (mit Note), B2.10.2.4, geprüft in [R-ARCH-06](../research/R-ARCH-06-ddi0487.md)
  §3.3/§3.4. *(Neu durch R-ARCH-06.)*
* **IMPLEMENTATION-DEFINED-Raum:** Systemregister mit op0 = 3, CRn = 0b1x11 (also 11 oder 15) sind für
  implementierungsdefinierte Register reserviert. Quelle: Arm System Register XML, `S3_<op1>_<Cn>_<Cm>_<op2>`
  ([Spiegel](https://df.lth.se/~getz/ARM/SysReg/AArch64-s3_op1_cn_cm_op2.html)).
* **SMCCC:** Eine unbekannte Funktions-ID liefert `-1` (vorzeichenerweitert) in `X0`. Quelle: TF-A-Commit „Redefine
  SMC_UNK as -1 instead of 0xFFFFFFFF“ mit Verweis auf ARM DEN 0028
  ([Commit](https://review.trustedfirmware.org/plugins/gitiles/TF-A/trusted-firmware-a/+/4abd7fa71f3cf921aab7c2b59a838f9c5bb10bb2%5E%21/bl31)).
  Welche Register der Aufgerufene erhalten muss, regelt DEN 0028 je nach Version. Das ist Sache der
  PSCI-Implementierung, nicht dieser Grenze.
* **`std::thread::park`/`unpark`:** `unpark` ist eine Release-Operation, `park` die passende Acquire-Operation. Ein
  `unpark` vor dem `park` geht nicht verloren (Token), und `park` darf spurious zurückkehren. Quelle:
  [`std::thread::park`](https://doc.rust-lang.org/std/thread/fn.park.html), Abschnitt *Memory Ordering*. Damit lässt
  sich Parken ohne neue Abhängigkeit und ohne `unsafe` bauen.
* **KVM-Modell:** `KVM_RUN` kehrt mit einem `exit_reason` in einer geteilten `kvm_run`-Struktur zurück.
  MMIO-Exits schließt der Nutzer ab, indem er `KVM_RUN` erneut aufruft. Quelle:
  [Linux KVM API](https://docs.kernel.org/virt/kvm/api.html), `KVM_RUN` / `KVM_EXIT_MMIO`. KVM hat dabei die
  Hardware-Syndrome (ISV) zur Verfügung. Wir haben den vollständigen Befehl, das Modell passt also nur teilweise.

**Annahmen** (alle Raten sind Schätzungen, gemessen wird nach der Überprüfung unten)

* **Annahme A-B1 (Raten):** Pro Interrupt 2–3 `ICC_*`-Zugriffe (`IAR1`, `EOIR1`, ggf. `DIR`). Unter Last höchstens
  ~20 000 Interrupts/s pro vCPU. Timer neu programmieren höchstens ~10 000/s pro vCPU. MMIO VM-weit höchstens
  ~50 000/s (virtio-Notify, PCI-Config, PL011). `HVC`/`SMC` selten (PSCI beim Boot, CPU-Hotplug), < 100/s.
  `WFI` höchstens ~10 000/s pro vCPU.
* **Annahme A-B2 (Kosten):** Ein Helper-Aufruf aus T1 in einen Rust-Hook über `dyn` kostet etwa 10–30 ns plus die
  Arbeit des Hooks. Ein Exit-Rundlauf (JIT-Code → Dispatcher → Rückkehr aus `run()` → Behandlung → `run()` →
  Jump-Cache → JIT) kostet etwa 150–400 ns.
* **Annahme A-B3:** XNU maskiert Interrupts über `DAIF`, nicht über `ICC_PMR_EL1` (kein Pseudo-NMI-Schema wie bei
  Linux). Dann ist `PMR` kalt. Prüfweg: Zähler aus M-3 unten beim ersten XNU-Boot (WP-DBT-20).
* **Annahme A-B4:** Ein geparkter Thread auf einem freien P-Kern wacht nach `unpark` (Futex-Wake) typischerweise in
  10–50 µs auf, abhängig von C-States und Scheduler.
* **Annahme A-B5:** Ob ein Gerät synchron antworten kann, hängt nicht von langer Host-I/O ab: virtio-Notify gibt nur
  an einen I/O-Thread weiter, PL011 schreibt in einen Puffer. Kein MMIO-Handler blockiert länger als wenige µs.

## Optionen

Allen Optionen gemeinsam: Die Typen liegen in `ritsu-dbt`, die Plattform implementiert sie, `ritsu-vmm` verdrahtet.
R-1 erfüllen alle drei. Sie unterscheiden sich darin, **wer den Kontrollfluss besitzt**.

1. **Option A: Nur Callbacks.** Der DBT besitzt die Ausführungsschleife und ruft für alles einen Plattform-Trait
   synchron im vCPU-Thread auf: `sysreg_read/write`, `mmio_read/write`, `hypercall(&mut regs)`,
   `wait_for_interrupt()`, `power_state()`.
   * Vorteile: Für Systemregister und MMIO die geringste Latenz. Kein Abschlussprotokoll, der DBT setzt den Befehl
     selbst fort.
   * Nachteile: Lebenszyklus und Strategie wandern in den DBT. `CPU_ON` muss eine *andere* vCPU starten, deren
     Zustand einem anderen Thread gehört. `SYSTEM_RESET`/`SYSTEM_OFF` müssen die Schleife verlassen, brauchen also
     doch einen Exit-Pfad. `WFI` blockiert entweder im Callback (die Plattform parkt im fremden Stack-Frame des
     Dispatchers) oder der DBT muss Timer-Deadlines kennen. Der DBT bräuchte einen Ein/Aus-Zustandsautomaten für
     vCPUs, also Plattformwissen. Callbacks mit `&mut`-Registerzugriff öffnen Wiedereintritt (Plattform → Callback
     → vCPU-Zustand → Dispatcher).
   * Kosten: mittel. Risiko: mittel bis hoch (Wiedereintritt, Deadlocks beim Stop-the-world, Plattformlogik im DBT).
   * Experiment, das sie stützen würde: Zeigt der Prototyp, dass `CPU_ON`/`RESET` ohne Exit-Pfad sauber gehen, ist
     A einfacher als C. Dagegen spricht, dass `SYSTEM_OFF` die Schleife per Definition verlassen muss.
2. **Option B: Nur Exits (à la `KVM_RUN`).** `Vcpu::run()` kehrt für **jedes** Plattform-Ereignis zurück:
   `Mmio{gpa, width, write, value}`, `SysReg{id, rt, write}`, `Hypercall`, `Wfi`, `Stopped`. Die Plattform
   behandelt das Ereignis und schließt es mit `complete_*` ab.
   * Vorteile: Der DBT ruft nie fremden Code auf. Die Schnittstelle ist reine Daten und lässt sich aufzeichnen und
     wiederholen. Das Modell ist bekannt (KVM, Hypervisor.framework).
   * Nachteile: MMIO und Systemregister entstehen **mitten im Block**. Ein Exit verlangt dort einen präzisen
     Ausstieg mit halb ausgeführtem Befehl und einen Wiedereinstieg, der den Befehl mit dem gelieferten Wert
     *abschließt*: Zielregister, Writeback bei Post-Index, zweiter Teilzugriff bei `LDP`/`STP`. KVM umgeht das
     über ISV und lehnt alles ohne Syndrom ab. Das dürfen wir nicht („niemals still falsch“). Wir bräuchten einen
     Wiederholungs-Puffer pro Befehl in T0, T1 und T2, also dreimal neue, fehleranfällige Maschinerie. Pro
     Interrupt kommen 2–3 Exit-Rundläufe dazu (A-B1, A-B2: ≈ 0,5–1 µs pro Interrupt, bei 20 000/s etwa 1–2 % einer
     vCPU). Dazu kommen MMIO-Exits.
   * Kosten: hoch (Abschlussprotokoll in drei Stufen). Risiko: mittel (Komplexität im heißesten Code).
   * Experiment, das sie stützen würde: Ist der gemessene Exit-Rundlauf (M-2) höchstens doppelt so teuer wie ein
     Hook-Aufruf **und** zeigt der Gast nur Einzelregister-Zugriffe auf MMIO (Zensus WP-DBT-19), wird B für
     Systemregister und MMIO wieder interessant.
3. **Option C: Hybrid.** Die Aufteilung folgt zwei Eigenschaften: *blockiert es* und *betrifft es mehr als diese
   vCPU*?
   * **Synchrone Hooks** (`PlatformHooks`, ein Objekt pro vCPU, im vCPU-Thread aufgerufen) für das, was kurz ist,
     nie blockiert und mitten im Block passiert: Plattform-Systemregister (`ICC_*`, `CNTV_*`, `CNTP_*`, beanspruchte
     IMPDEF-Register) und MMIO.
   * **Exits** (`Vcpu::run() -> VcpuExit`) für das, was blockiert, andere vCPUs oder den Lebenszyklus betrifft und
     an einer Befehlsgrenze liegt: `HVC`/`SMC`, `WFI`, Stopp, fatale Fehler.
   * **Signale** (`VcpuSignals`, atomar, `Sync`) für alles, was von außen kommt: IRQ-/FIQ-Pegel, SError, Stopp-
     und Kick-Anforderungen.
   * Vorteile: Mitten im Block gibt es nur Aufrufe ohne Abschlussprotokoll. Lebenszyklus und Strategie (PSCI,
     Parken, Reset) liegen vollständig in der Plattform. Hooks bekommen keinen Zugriff auf die vCPU, damit gibt es
     keinen Wiedereintritt. Testdoubles für beide Seiten sind klein.
   * Nachteile: Zwei Mechanismen statt einem. Hooks laufen im vCPU-Thread, deshalb gelten für sie Regeln (nicht
     blockieren, auf keine vCPU warten), die nur per Review und Stresstest durchsetzbar sind.
   * Kosten: niedrig bis mittel (WP-DBT-07/08, PLAT-06/07/15). Risiko: niedrig bis mittel (Deadlock-Regeln, siehe
     §7).

**Verworfen ohne nähere Ausarbeitung**

* **GIC-CPU-Interface im DBT** (Schattenregister für `PMR`, laufende Priorität und Active-Priorities im DBT, nur der
  Distributor in der Plattform): bricht „`ritsu-dbt` kennt keine Geräte“ und verteilt den GIC-Zustand auf zwei
  Crates. Bleibt als gezielte Optimierung *nur für `ICC_PMR_EL1`* erhalten, falls A-B3 fällt (M-3).
* **Kick per POSIX-Signal** (`pthread_kill` in den laufenden JIT-Code, wie der KVM-Kick aus `KVM_RUN`), um die
  Prüfung am Blockeintritt zu sparen: Der Signal-Handler müsste an einer beliebigen Host-Adresse den präzisen
  Gast-Zustand rekonstruieren. Die Prüfung kostet etwa einen Zyklus pro Block (R-3).
* **Generischer Hook-Typ** (`Vcpu<P: PlatformHooks>`, monomorphisiert): T1-Code ruft Helper ohnehin über eine
  Tabelle mit C-ABI auf (dbt.md §14.3). Ein `dyn`-Aufruf mehr im Helper kostet wenige ns und hält `Vcpu`
  nicht-generisch (geteilter Code-Cache, persistenter Cache).

## Entscheidung

**Option C.** Ab Freigabe gilt:

### 1. Eigentum und Ort

* Alle Typen der Grenze liegen in `ritsu-dbt`, Modul `vcpu` (Taktquelle: Modul `clock`). Die Plattform implementiert
  `PlatformHooks` und besitzt den Run-Loop (`ritsu_platform::cpu`, Arbeitstitel `VcpuRunner`). `ritsu-vmm` erzeugt
  die Threads, pinnt sie (dbt.md §13.1) und verdrahtet. Es entsteht keine neue Kante (R-1).
* Breite eines MMIO-Zugriffs ist `ritsu_mem::Width` aus [ADR-0009](0009-shared-guest-ram.md). Wird ADR-0009 nicht
  angenommen, definiert `ritsu-dbt` einen gleichwertigen Typ. `ritsu_platform::mmio::AccessSize` bekommt eine
  verlustfreie Umwandlung (`From<Width>`).

### 2. Schnittstellen-Skizze

Verbindlich sind die Eigenschaften und die Aufteilung. Die exakten Namen legen WP-DBT-07/-08 fest.

```rust
// ritsu-dbt, mod vcpu

/// MRS/MSR encoding (op0, op1, CRn, CRm, op2), packed into 16 bits (2+3+4+4+3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SysRegId(u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SysRegAccess { Read, Write, ReadWrite }

/// A system register the platform services; validated once at `Vcpu::new` (§3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SysRegClaim { pub reg: SysRegId, pub access: SysRegAccess }

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SysRegFault {
    /// → Undefined Instruction (EC 0x00) at the MRS/MSR, state unchanged.
    #[error("undefined system register access")]
    Undefined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MmioAbort {
    /// → synchronous Data Abort, DFSC 0x10 (§5).
    #[error("synchronous external abort")]
    External,
}

/// Implemented by ritsu-platform; one instance per vCPU, owned by that `Vcpu`, called
/// only on its thread. Never re-enters the vCPU (it gets no handle to it). Rules: §7.4.
pub trait PlatformHooks: Send {
    fn sysreg_claims(&self) -> &[SysRegClaim];
    fn sysreg_read(&mut self, reg: SysRegId) -> Result<u64, SysRegFault>;
    fn sysreg_write(&mut self, reg: SysRegId, value: u64) -> Result<(), SysRegFault>;
    fn mmio_read(&mut self, gpa: GuestPhysAddr, width: Width) -> Result<u64, MmioAbort>;
    fn mmio_write(&mut self, gpa: GuestPhysAddr, width: Width, value: u64) -> Result<(), MmioAbort>;
}

/// Cross-thread inputs of one vCPU. `Sync`, shared as `Arc` with GIC, timer thread,
/// other vCPUs and the runner. Protocol: §6.
pub struct VcpuSignals { /* exit byte, level lines, request bits, bound Thread */ }
impl VcpuSignals {
    pub fn set_irq_line(&self, level: bool);
    pub fn set_fiq_line(&self, level: bool);
    pub fn raise_serror(&self);                  // latched until taken
    pub fn request_stop(&self);                  // consumed by run() → VcpuExit::Stopped
    pub fn kick(&self);                          // forces one pass through the dispatcher
    /// Blocks until a WFI wake-up condition holds (§7.2), `deadline` passes, or spuriously.
    pub fn wait_for_wake(&self, deadline: Option<std::time::Instant>);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conduit { Hvc, Smc }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HypercallOutcome { Handled, Undefined }

#[derive(Debug)]
#[non_exhaustive]
pub enum VcpuExit {
    /// HVC/SMC from EL1. PC = address of the instruction, registers canonical.
    Hypercall { conduit: Conduit, imm: u16 },
    /// WFI retired (PC already advanced), no wake-up condition was pending.
    Wfi,
    /// A stop request was observed at an instruction boundary.
    Stopped,
    /// Host-side failure (not a guest error); the VM must stop.
    Fatal(VcpuFatal),
}

pub struct Vcpu { /* Send, !Sync */ }
impl Vcpu {
    pub fn new(cfg: VcpuConfig, hooks: Box<dyn PlatformHooks>, vm: &Arc<DbtVm>) -> Result<Self, VcpuBuildError>;
    pub fn signals(&self) -> &Arc<VcpuSignals>;
    pub fn run(&mut self) -> VcpuExit;
    pub fn state(&self) -> &CpuState;            // only between runs (borrowck enforces it)
    pub fn state_mut(&mut self) -> &mut CpuState;
    pub fn complete_hypercall(&mut self, outcome: HypercallOutcome);
    pub fn reset(&mut self, entry: ResetState);  // architectural reset (dbt.md §2), flushes TLB/jump cache/monitor
}

// ritsu-dbt, mod clock
/// Guest counter: frequency plus host-time → guest-tick mapping. One instance per VM,
/// shared by the DBT (inline CNTVCT/CNTPCT) and the platform (timer deadlines).
pub struct GuestClock { /* freq_hz, source (invariant TSC or CLOCK_MONOTONIC_RAW), mult, shift, offset */ }
```

`DbtVm` ist das VM-weite DBT-Objekt (Übersetzungs-Cache, `Arc<GuestRam>`, `GuestClock`, Signale aller vCPUs für
TLBI- und `SEV`-Rundrufe). Seine Form legt WP-DBT-13 fest. Für diese Grenze ist nur wichtig, dass DBT-interne
Rundrufe dieselbe `VcpuSignals`-Struktur nutzen (crate-private Anforderungsbits). So deckt eine Prüfung pro Block
alle Gründe ab.

### 3. Systemregister

**Zuordnung.** Die generierte Systemregister-Tabelle (dbt.md §10.1) bekommt die Spalte `owner ∈ {Dbt, Platform}`.
Zur Übersetzungszeit steht fest, welcher Fall vorliegt:

| Register | Eigentümer | Weg |
|---|---|---|
| `ICC_*_EL1`: op0 = 3, op1 = 0, CRn = 12, CRm ∈ {8, 9, 11, 12}, dazu `ICC_PMR_EL1` (3, 0, 4, 6, 0) | Plattform (GIC) | Hook |
| `CNTV_CTL/CVAL/TVAL_EL0`, `CNTP_CTL/CVAL/TVAL_EL0` | Plattform (Timer) | Hook |
| IMPDEF-Raum (op0 = 3, CRn ∈ {11, 15}), **nur** was die Plattform beansprucht | Plattform | Hook |
| `CNTFRQ_EL0`, `CNTVCT_EL0`, `CNTPCT_EL0` | DBT | inline aus `GuestClock` (§4) |
| `CNTKCTL_EL1` | DBT | Zustand im `CpuState`. Steuert EL0-Rechte und Event-Stream, also DBT-Belange (Rechteprüfung, `WFE`). |
| `ISR_EL1` | DBT | aus `VcpuSignals` (Pegel I/F, SError anstehend) |
| `VBAR_EL1`, `RVBAR_EL1` (CRn = 12, CRm ∈ {0, 1}) und alle übrigen | DBT | eigene Tabelle |

**Beanspruchen.** `Vcpu::new` liest einmal `sysreg_claims()` und baut daraus eine unveränderliche Tabelle. Jeder
Anspruch außerhalb der drei Plattform-Zeilen oben ist ein Konstruktionsfehler (`VcpuBuildError`). So kann die
Plattform kein DBT-Register (z. B. `SCTLR_EL1`) übernehmen, und kein Plattform-Register kann `TbFlags` beeinflussen.
Ein Register, das das täte, braucht einen neuen ADR.

**Trap-Semantik (präzise, R-2).**

1. Weder DBT-Register noch beansprucht → UNDEFINED (EC 0x00), **statisch** in der Beschreibung (`raise`), ohne
   Hook-Aufruf. Das gilt auch für die falsche Richtung (`MSR` auf einen nur lesbaren Anspruch) und für unbekannte
   IMPDEF-Register (dbt.md §10.1, „nie still ignorieren“).
2. **Rechte prüft der DBT vor dem Hook:** `ICC_*` nur ab EL1. Aus EL0 sind sie UNDEFINED. `CNTV_*`/`CNTP_*` aus EL0
   nur, wenn `CNTKCTL_EL1.EL0VTEN`/`EL0PTEN` gesetzt sind, sonst Trap nach EL1 mit EC 0x18 (`ISS` mit
   Registerkodierung und Richtung). Der Hook sieht nur architektonisch erlaubte Zugriffe.
3. Liefert der Hook `Err(SysRegFault::Undefined)`, entsteht UNDEFINED am Befehl. Der Zustand bleibt unverändert,
   weil die Beschreibung Ziel-Writes erst nach dem Hook ausführt (Reihenfolge-Regel aus R-DBT-01).
4. Ein **Diagnose-Modus** der Plattform (analog zum MMIO-Trace-Modus, platform.md §4) darf den ganzen IMPDEF-Raum
   beanspruchen, protokollieren und RAZ/WI liefern. Er ist nie Standard.

**Wirkung auf Interrupts.** Ein Hook, der die Zustellbarkeit **senkt** (`PMR` erhöht die Maske, `IGRPEN1 = 0`,
`IAR1` quittiert), aktualisiert den Pegel in `VcpuSignals`, **bevor** er zurückkehrt. Damit ist die
Selbstsynchronisation von `ICC_PMR_EL1` erfüllt: Der DBT nimmt Interrupts nur im Dispatcher und liest dort den
aktuellen Pegel. Ein Hook, der die Zustellbarkeit **hebt**, setzt den Pegel und das Exit-Byte (§6). Der Interrupt
wird dann an der nächsten Blockgrenze genommen. Plattform-`MSR` beendet deshalb den Block **nicht**.

**Kosten.**

| Stufe | Plattform-`MRS`/`MSR` | DBT-eigenes Register |
|---|---|---|
| T0 | direkter Aufruf `hooks.sysreg_*` | Tabellenzugriff |
| T1 | `call [r15 + HELPER_SYSREG_{RD,WR}]` mit `SysRegId` als Immediate, lebende caller-saved Cache-Register (`r8`–`r10`) sichern, `test eax, eax; jnz .cold`. Schätzung ~10–30 ns plus Hook (A-B2). Der Fehlerpfad liegt im kalten Stub (dbt.md §4.3). | inline (`mov` aus/in `CpuState`) bzw. Helper für Register mit Seiteneffekt |
| T2 | `CallHelper{effects: MayRaise ∣ Barrier}`. Keine Lade-/Speicher-Elimination und keine Bewegung von Speicher-Ops über den Aufruf. Gast-Register dürfen promoted bleiben, weil der Hook `CpuState` nicht liest. | wie T1, Konstantenfaltung möglich |

Der RAM-Fastpath und DBT-eigene Register zahlen nichts (R-3).

### 4. Generic Timer: Zähler im DBT, Vergleicher in der Plattform

* **Eine Zeitquelle:** `GuestClock` (in `ritsu-dbt`) bildet Host-Zeit auf Gast-Ticks ab. Mit invariantem TSC
  (CPUID `0x8000_0007` EDX Bit 8) gilt `((rdtsc − base) · mult) >> shift + offset` (dbt.md §10.4), sonst ein
  Helper über `CLOCK_MONOTONIC_RAW` (platform.md §6). DBT (Inline-`MRS CNTVCT_EL0`) und Plattform (Deadlines,
  `TVAL`, `ISTATUS`) rechnen mit **derselben** Funktion. Die Frequenz (Startwert 24 MHz, **Annahme** A-10 /
  platform.md §6) wird einmal beim Bau übergeben.
* `offset` (das Gegenstück zu `CNTVOFF`, für Pausen und Snapshots) ändert sich **nur, solange alle vCPUs stehen**
  (nach `Stopped`). Jede vCPU kopiert `mult/shift/offset` bei jedem `run()`-Eintritt in ihren Kontext. Der JIT liest
  damit nur eigene Daten, ohne Lock und ohne Tearing.
* `CNTV_*`/`CNTP_*` liegen als Zustand in der Plattform (pro vCPU) und werden per Hook gelesen und geschrieben. Die
  Plattform betreibt den Timer-Thread (platform.md §6) und setzt bei Ablauf die PPI (INTID 27 bzw. 30, **Annahme**
  platform.md §5.1) im GIC. Das hebt den Pegel und weckt die vCPU (§6).
* Begründung gegen die Variante aus dbt.md §10.4 (Vergleicher im DBT): Läuft die vCPU, muss ohnehin ein anderer
  Thread die Deadline überwachen. Die Deadline muss also in jedem Fall zur Plattform. Liegt der Vergleicher dort,
  braucht der DBT weder Timer-Logik noch einen Rückkanal „Deadline erreicht“, und `WFI` kann die Plattform mit
  eigener Kenntnis der Deadline parken (§7.2). Kosten: `CVAL`/`TVAL`-Zugriffe sind Hook-Aufrufe (A-B1: ≤ 10 000/s,
  also < 0,1 % einer vCPU).

### 5. MMIO-Slow-Path

1. **TLB-Füllung** (SoftMMU, dbt.md §7.3/§7.4): Stage-1-Walk → PA. Liegt die PA in einer RAM-Region von `GuestRam`
   (ADR-0009, `host_range`), entsteht ein normaler Eintrag mit `addend`. Sonst bekommt der Eintrag das Flag `MMIO` in
   `cmp_read`/`cmp_write`, und eine Seitentabelle neben dem TLB merkt sich `PA − VA` sowie den Speichertyp aus dem
   Walk (Device/Normal). Jeder Zugriff auf die Seite verfehlt damit den Fastpath, RAM-Zugriffe zahlen nichts.
2. **Slow-Path-Helper** (T1/T2 per Helper, T0 direkt). Der Helper bekommt VA, Breite, Richtung, Wert und die
   konstanten `MemAttrs` der Beschreibung (R-DBT-01 P1-2), dann:
   * PA = VA + gespeicherter Abstand.
   * **Device-Typ und unausgerichtet** → Alignment Fault (DFSC 0x21), durch den DBT, ohne Hook. **Normal-Typ und
     unausgerichtet** → kein Aufteilen, der Hook bekommt den Zugriff nicht, der DBT meldet einen synchronen External
     Abort (Geräte sehen nur natürlich ausgerichtete Zugriffe, platform.md §8.1).
   * **Exklusive und atomare Zugriffe** (`LDXR/STXR`, LSE, `CASP`) auf Nicht-RAM → synchroner External Abort durch
     den DBT, ohne Hook. Das ersetzt „Gerät entscheidet“ aus dbt.md §8.4. Keine Plattform-Schnittstelle für Atomics.
   * **Mehrfachzugriffe** (`LDP/STP`, `LD1 {…}` mit mehreren Registern): Die Beschreibung erzeugt Einzelzugriffe in
     Architekturreihenfolge (dbt.md §7.5). Scheitert ein Teilzugriff, meldet der DBT einen **synchronen** Data Abort
     (Punkt 3) mit `FAR_EL1` = VA des fehlschlagenden Teilzugriffs. Bereits ausgeführte Teil-Stores, auf RAM wie auf
     MMIO, bleiben bestehen. Das ist architekturkonform (Fakt „Mehrfachzugriffe“ oben, `R_TNVSL`). Nach der Rückkehr
     wiederholt der Gast den Befehl, und die erneuten Gerätezugriffe sind erlaubt (B2.10.2.4). Die `probe`-Regel aus
     R-DBT-01 P1-2 bleibt zulässig, ist aber nicht nötig. Ob sie bleibt, entscheidet `dbt-engineer` (R-ARCH-06
     §3.4). **Pflicht:** Der DBT ruft für einen einzelnen Zugriff `mmio_read`/`mmio_write` **höchstens einmal** auf.
     Kein Retry, kein Wiederabspielen nach einem kalten Stub, und eine Schreibbarkeitsprüfung auf MMIO liest nie.
     Ein einzelner single-copy-atomarer Device-Zugriff darf architektonisch nicht wiederholt werden (B2.10.2).
     *(Geändert durch R-ARCH-06: Die frühere SError-Regel beruhte auf der Annahme, Teilaktualisierung sei verboten.
     Diese Annahme ist widerlegt. Die Annahme zur Device-Wiederholung ist bestätigt und jetzt Fakt.)*
   * Aufruf `hooks.mmio_read/write(pa, width[, value])`. Bei `Ok` setzt die Beschreibung fort (Zielregister,
     Writeback). Es gibt keinen Exit und keinen Wiederholungs-Puffer.
3. **Fehler** (`Err(MmioAbort::External)`): synchroner Data Abort nach EL1, EC 0x24 (aus EL0) bzw. 0x25 (aus EL1),
   `IL = 1`, `ISS`: DFSC 0x10, `WnR` aus der Richtung (bei atomaren Befehlen erlaubt die Architektur einen
   beliebigen Wert, Ritsu setzt 1), `ISV = 0` (Fakt oben), `FnV = 0`, `EA = 0`, `FAR_EL1` = VA des Teilzugriffs,
   `ELR_EL1` = PC des Befehls. `SET` (ISS[12:11]) gibt es nur, wenn FEAT_RAS gemeldet wird. Das entscheidet
   WP-ARCH-03. Registerzustand unverändert, bei Mehrfach-Stores dürfen frühere Teil-Stores bestehen bleiben (Punkt
   2). In T1/T2 gibt der Helper einen Status zurück. Der kalte Stub schreibt die gecachten Register der vorherigen
   Befehle zurück, setzt `PC` und verlässt den Block (dbt.md §4.3, §10.2). *(`IL`, `EA`, `WnR` bei Atomics und
   `SET` ergänzt durch R-ARCH-06 §3.5.)*
4. **Ordnung:** MMIO ist ein synchroner Aufruf in Programmreihenfolge und damit nie spekulativ (dbt.md §7.5). Vorher
   ausgeführte Gast-Stores auf RAM sind x86-Stores desselben Threads. Die Synchronisation zum Geräte-Thread
   (Release/Acquire im Gerät, z. B. eventfd oder Lock) macht sie dort sichtbar. Der Gast muss ohnehin `DMB`/`DSB`
   vor die Doorbell setzen.
5. **Befehlsabruf** aus Nicht-RAM: Instruction Abort (External, IFSC 0x10) durch den DBT, ohne Hook. Code, der
   ausgeführt wird (Boot-ROM), muss deshalb in einer RAM-artigen Region liegen. Nur lesbare, ausführbare Regionen
   legt der Nachtrag zu [ADR-0009](0009-shared-guest-ram.md) §9 fest. *(Verweis aktualisiert durch R-ARCH-06.)*

### 6. Interrupt-Zustellung: `VcpuSignals`

**Ablage.** `VcpuSignals` und der JIT-Kontext (`CpuState`) liegen in **einer** Allokation pro vCPU. Die Signale
sind Atomics, `CpuState` liegt dahinter in einer `UnsafeCell`, auf die nur der Besitzer der `Vcpu` zugreift. So
erreicht `r15` das Exit-Byte mit festem Offset, und die Prüfung bleibt **ein** Befehl
(`cmp byte [r15 + EXIT_OFF], 0; jne exit`), ohne dass je eine `&mut`-Referenz die Atomics überdeckt (kein UB, R-5).
Die `unsafe`-Begründung lebt in einem Modul mit sprechendem Namen (CLAUDE.md §3 Regel 3, z. B. `vcpu::frame`).

**Inhalt.** Ein Exit-Byte (`AtomicU8`, ≠ 0 heißt „zum Dispatcher“). Pegel (`AtomicU32`: `IRQ`, `FIQ`) als
**Level**, also das IRQ/FIQ-Signal des GIC an diese CPU, unabhängig von `PSTATE`. Anforderungen (`AtomicU32`:
`STOP`, `SERROR`, `KICK` sowie crate-private DBT-Bits für TLB-Flush, Invalidierung, `SEV`) als **Flanken**. Dazu der
gebundene `std::thread::Thread` zum Wecken (`OnceLock`, bei jedem `run()`-Eintritt geprüft).

**Protokoll (Ordnung).**

| Seite | Schritte |
|---|---|
| Erzeuger (GIC, Timer-Thread, andere vCPU, Runner) | (1) eigenen Zustand ändern (z. B. GIC unter der Sperre des Ziel-Redistributors); (2) Pegel bzw. Anforderungsbit mit `Release` setzen. **Der Pegel wird unter derselben Sperre geschrieben, die den Zustand dieses Redistributors schützt**, sonst überholt ein veralteter Pegel einen neuen; (3) bei steigendem Pegel oder neuer Anforderung: Exit-Byte mit `Release` auf 1; (4) `thread.unpark()` (Release, billig, wenn der Thread nicht parkt). |
| JIT (T1/T2) | Byte lesen (x86-Load, auf TSO mit Acquire-Wirkung, außerhalb des Rust-Modells wie in ADR-0009 A-M3). Bei ≠ 0 Exit zum Dispatcher an der Blockgrenze. |
| Dispatcher | (1) Exit-Byte per `swap(0, Acquire)`; (2) Anforderungen per `swap(0, AcqRel)`; (3) Pegel per `load(Acquire)`; (4) abarbeiten. Weil zuerst das Byte gelöscht und **danach** gelesen wird, setzt ein Erzeuger, der nach (3) schreibt, das Byte erneut. Kein Ereignis geht verloren. |
| Parken | `loop { if wake_condition() { break } ; park_timeout(..) }`. Das Token von `park/unpark` verhindert verlorene Wakeups, und spurious Rückkehr ist durch die Schleife abgedeckt. |

**Prüfpunkte.** T0 an jeder Blockgrenze und spätestens nach 64 Befehlen. T1 am Eintritt jedes Blocks, das gilt auch
für verkettete Sprünge (dbt.md §5.2). T2 am Regionseintritt und an **jeder Rückwärtskante** innerhalb der Region.
Befehle, die `PSTATE.{A,I,F}` ändern (`MSR DAIFClr`, `ERET`), beenden den Block (platform.md §5.2). Ein Pegelabfall
braucht keinen Kick. Der Dispatcher entscheidet immer nach dem aktuellen Pegel, und ein Interrupt, der danach nicht
mehr ansteht, kann architekturkonform als spurious (`INTID 1023`) am GIC enden.

**Zustellung im Dispatcher.** SError, wenn `PSTATE.A = 0`, FIQ, wenn `F = 0`, IRQ, wenn `I = 0`, jeweils mit
Exception-Eintritt nach dbt.md §10.2. Die Reihenfolge unter gleichzeitig anstehenden Interrupts legt WP-DBT-07 nach
Arm ARM fest. Sie ist nicht Teil der Grenze. Die Quittierung macht der Gast per `ICC_IAR1_EL1` (Hook). Ein
Rückkanal „Exception genommen“ zum GIC ist nicht nötig.

**Latenzziele** (Ziele, gemessen in M-4):

| Fall | p50 | p99 |
|---|---|---|
| vCPU läuft (T1/T2): von `set_irq_line(true)` bis zum ersten Befehl am IRQ-Vektor | ≤ 2 µs | ≤ 10 µs |
| vCPU parkt in `WFI`: dasselbe | ≤ 30 µs | ≤ 200 µs (A-B4) |
| Timer: von der `CVAL`-Deadline bis zum Vektor | ≤ 50 µs | ≤ 1 ms (PLAT-07) |

### 7. Run-Loop-Vertrag

**7.1 `run()`.** `Vcpu::run(&mut self) -> VcpuExit` arbeitet beim Eintritt zuerst alle Anforderungen ab (TLB-Flush,
Invalidierung, Stopp), nimmt fällige Interrupts und führt dann Gast-Code aus, bis ein Exit-Grund eintritt. Bei der
Rückkehr liegt der Architekturzustand kanonisch im `CpuState` (E-7) an einer Befehlsgrenze. `state()`/`state_mut()`
sind nur zwischen zwei `run()`-Aufrufen erreichbar (Borrow-Regeln). `run()` panickt nie auf Gast-Eingaben.

**7.2 Exit-Gründe**

| Exit | Wann | Zustand bei Rückkehr | Pflicht des Runners |
|---|---|---|---|
| `Hypercall{conduit, imm}` | `HVC #imm` oder `SMC #imm` aus **EL1**. Aus EL0 sind beide UNDEFINED, ohne Exit (Architektur). | `PC` = Adresse des Befehls | Mit `X0…X17` per `state_mut()` (SMCCC/PSCI) arbeiten, dann `complete_hypercall(Handled)`: `PC += 4` und Monitor löschen (wie Eintritt und Rückkehr einer echten Exception). Oder `complete_hypercall(Undefined)`: UNDEFINED am Befehl (EC 0x00, `ELR` = Befehl). Ohne Abschluss behandelt das nächste `run()` den Aufruf als `Undefined` (`debug_assert`). |
| `Wfi` | `WFI` aus EL1 (oder EL0 mit `SCTLR_EL1.nTWI = 1`) **und** keine Wake-Bedingung: kein IRQ/FIQ-Pegel (unabhängig von `PSTATE`), kein SError, keine Anforderung. Mit Wake-Bedingung ist `WFI` ein NOP ohne Exit. EL0 mit `nTWI = 0` **und ohne anstehenden IRQ-/FIQ-Pegel oder SError** → Trap EC 0x01, ohne Exit. Steht ein Pegel an, ist `WFI` auch dort ein NOP, weil `Hint_WFI` anstehende Interrupts vor dem Trap prüft (R-ARCH-06 §3.2). | `WFI` ist abgeschlossen, `PC` zeigt auf den Folgebefehl | Parken mit `signals.wait_for_wake(eigene_nächste_Timer_Deadline)`, danach eigene Timer auswerten (PPI setzen), wieder `run()`. Spurious Rückkehr ist erlaubt (Arm: `WFI` darf jederzeit enden). |
| `Stopped` | `STOP` angefordert (Pause, Snapshot, Reset, Shutdown, Debugger) | Befehlsgrenze | Runner entscheidet (Barriere, Reset, Ende) |
| `Fatal(VcpuFatal)` | Host-Fehler (Code-Cache dauerhaft voll, verletzte Invariante) | best effort | VM kontrolliert beenden, nie still weiterlaufen (platform.md §11) |

`WFE`, `SEV`, `SEVL` erledigt der DBT **intern** (kurz spinnen, dann über denselben Park-Mechanismus mit begrenztem
Timeout, dbt.md §10.5). Es gibt keinen Exit. Für das Event-Register und die Weckereignisse gilt (Fakt oben,
R-ARCH-06 §3.2):

* Das Event-Register setzen `SEV` (jede vCPU, per Rundruf über `VcpuSignals`), `SEVL`, **jede Exception-Rückkehr**
  (`ERET`), das **Löschen des globalen Monitors** dieser vCPU und der Event-Stream aus `CNTKCTL_EL1`. Ist es beim
  `WFE` gesetzt, wird es gelöscht, und der Befehl endet sofort. Das gilt auch aus EL0 mit `SCTLR_EL1.nTWE = 0`. Nur
  wenn es nicht gesetzt ist, folgt dort der Trap (EC 0x01).
* Geweckt wird ein schlafendes `WFE` durch *nicht maskierte* Interrupts (`PSTATE.I`/`F`/`A` = 0 für IRQ/FIQ/SError),
  `SEV` anderer vCPUs, den Event-Stream, das Löschen des globalen Monitors und Anforderungen.
* **Löschen des globalen Monitors:** Ein fremder Schreibzugriff auf den per `LDXR` markierten Block muss ein
  schlafendes `WFE` wecken. Spinlock-Code darf sich darauf verlassen und beim Freigeben auf `SEV` verzichten
  (B2.12.6). Weil der DBT normale Stores anderer vCPUs nicht beobachtet, ist bei scharfem lokalem Monitor das
  **Polling des markierten Werts** während des Wartens Pflicht (dbt.md §10.5, O-DBT-17). Der begrenzte Timeout ist
  die Rückfallebene für Korrektheit, nicht nur eine Optimierung. Ein Aufwachen ohne Grund ist immer erlaubt
  (`I_QGHTM`).

*(Absatz korrigiert durch R-ARCH-06: Die frühere Fassung „geweckt wird nur durch …“ ließ das Löschen des globalen
Monitors und die Event-Register-Quellen `SEVL`/Exception-Rückkehr aus.)* Ein späterer User-Mode-Betrieb (WP-DBT-03)
darf `VcpuExit` um `Syscall{imm}` erweitern (`#[non_exhaustive]`).

**7.3 Threading.** Ein Host-Thread pro vCPU. `Vcpu` ist `Send`, nicht `Sync`, `VcpuSignals` ist `Sync`. Der Runner
(`ritsu-platform`) läuft in diesem Thread. `ritsu-vmm` startet und pinnt ihn. Fremde Threads greifen auf eine vCPU nur
über ihre `VcpuSignals` zu, nie auf `CpuState`. PSCI `CPU_ON` für vCPU *k* schreibt deshalb Einstieg und
`context_id` in ein Postfach des Runners von *k* (Plattform-intern) und weckt ihn. Der Runner von *k* ruft
`reset(ResetState{pc: entry, x0: context_id, el: El1h, mmu: off})` (PSCI DEN 0022, `CPU_ON`) und dann `run()`. Alle
vCPUs außer 0 beginnen im Runner-Zustand „aus“ (platform.md §7).

**7.4 Regeln für Hooks** (Review-Pflicht, Stresstest M-6):

1. Ein Hook **blockiert nicht** länger als wenige µs und wartet **nie** auf den Fortschritt einer vCPU (auch nicht
   der eigenen). Längere Arbeit geht an einen Geräte-Thread (A-B5).
2. Sperren in Hooks sind **Blatt-Sperren**: Sie werden innerhalb des Aufrufs genommen und freigegeben. Kein Hook
   hält eine Sperre über seine Rückkehr hinaus.
3. Hooks bekommen **keinen** Zugriff auf `Vcpu` oder `CpuState`. Wiedereintritt ist damit per Typ ausgeschlossen.
   Wirkung auf andere vCPUs nur über deren `VcpuSignals`.
4. Gast-Fehler werden zu `SysRegFault`/`MmioAbort`, nie zu Panics (R-7).

**7.5 Ruhepunkte und Stopp.** Eine vCPU **außerhalb** von `run()` gilt für DBT-interne Rundrufe (TLB-Shootdown,
Epochen-Freigabe, dbt.md §5.6, §13.3) als ruhend und bestätigt. `run()` holt beim Eintritt alles nach. Interne
Wartestellen des DBT (`DSB` nach `TLBI …IS`, strenger LL/SC-Modus) prüfen dabei das eigene `STOP`. Bei Stopp
verlassen sie `run()` mit `PC` am wartenden Befehl, der später erneut ausgeführt wird. Damit kann
Stop-the-world nicht mit einem Shootdown verklemmen.

### 8. PSCI, Shutdown, Reset (Plattform-Seite, informativ)

* Der DBT kennt weder PSCI noch Conduit. Die Plattform entscheidet pro `Conduit`, ob sie `Handled` oder `Undefined`
  meldet. Klärt die Spec (S-BOOT-1/PLAT-07), dass macOS SMC statt HVC nutzt, PSCI gar nicht nutzt oder Apple-eigene
  Hypercalls hat, ändert sich nur der Runner (R-6). Unbekannte Funktions-IDs liefern `-1` in `X0` (SMCCC).
* `SYSTEM_OFF`: Der Runner der aufrufenden vCPU setzt `request_stop()` bei allen vCPUs, wartet an einer Barriere,
  bis alle `Stopped` gemeldet haben, und gibt „aus“ an `ritsu-vmm` zurück.
* `SYSTEM_RESET`: wie `SYSTEM_OFF`, danach Geräte-Reset, `DbtVm` invalidiert alle Übersetzungen, das Boot-ROM wird neu
  geladen, `reset()` an vCPU 0, alle anderen vCPUs „aus“.
* `CPU_OFF`: Der Runner hört auf, `run()` aufzurufen, und parkt bis zu einem `CPU_ON`.

## Konsequenzen

* **Positiv:**
  * Eine Grenze mit drei klar getrennten Mechanismen. Mitten im Block gibt es nur Aufrufe ohne Abschlussprotokoll,
    an Befehlsgrenzen nur Exits, von außen nur Signale.
  * Präzise Exceptions entstehen an einer Stelle im DBT (Systemregister statisch oder per Hook-Status, MMIO per
    Helper-Status). `ESR`-Felder sind festgelegt (ISV = 0).
  * Die Widersprüche zwischen dbt.md und platform.md (Timer, `WFI`, PSCI) sind aufgelöst. Die Spec-Lücken
    (Conduit, IMPDEF-Register, Timer-PPIs) bleiben in der Plattform eingekapselt.
  * Getrennt testbar (R-8). `ritsu-dbt` liefert ein kleines `NullPlatform`-Testdouble (keine Ansprüche, MMIO →
    `External`). Mini-Kernel-Tests aus WP-DBT-07/08 nutzen ein Double mit einem Fake-GIC. Der Plattform-Runner lässt
    sich gegen eine `Vcpu` auf T0 testen.
* **Negativ / Kosten:**
  * Zwei Mechanismen (Hooks und Exits) statt einem, und Hook-Regeln (§7.4), die kein Typ erzwingt.
  * Ein kleiner `unsafe`-Kern für die gemeinsame Allokation aus Signalen und `CpuState` (§6).
  * Timer-Vergleichsregister kosten einen Hook-Aufruf statt eines `mov`.
  * `Vcpu` hängt an `ritsu_mem::Width` und damit an ADR-0009 (Rückfall in §1).
* **Folgearbeiten (Work Packages):**
  * **WP-DBT-07:** Spalte `owner` in der Systemregister-Tabelle, `SysRegId`, Anspruchsprüfung, statisches
    UNDEFINED, EL0-Rechte inkl. `CNTKCTL_EL1`, `ISR_EL1` aus Signalen, Data-Abort-Syndrom nach §5.3.
  * **WP-DBT-08:** Modul `vcpu` (`Vcpu`, `VcpuExit`, `VcpuSignals`, `PlatformHooks`, `NullPlatform`), `clock::GuestClock`
    mit Inline-Zähler, `WFI`-Exit, internes `WFE`/`SEV`, Latenz-Benchmark M-4 (Teil „läuft“).
  * **WP-DBT-12 / WP-DBT-17:** Helper-ABI für Hooks und MMIO, kalte Stubs, Prüfung am Blockeintritt (T1) und an
    Rückwärtskanten (T2), Effektklasse `Barrier` für `CallHelper`.
  * **WP-DBT-13:** `DbtVm`, abbrechbare Wartestellen (§7.5).
  * **PLAT-06:** GIC implementiert die `ICC_*`-Seite von `PlatformHooks`, Pegel unter der Redistributor-Sperre (§6).
  * **PLAT-07:** Timer (`CNTV_*`/`CNTP_*` per Hook, Timer-Thread mit `GuestClock`), PSCI im Runner auf
    `VcpuExit::Hypercall`.
  * **PLAT-15:** `VcpuRunner`, Stopp-Barriere, Reset und Shutdown, Messung M-4 (Teil „parkt“).
  * **ADR-0009-Nachtrag** (bei dessen Umsetzung WP-MEM-02): nur lesbare RAM-Regionen (ROM, Befehlsabruf erlaubt,
    Writes → MMIO-Hook oder Abort) für das Boot-ROM (§5.5). Der Nachtrag liegt als ADR-0009 §9 vor (Proposed).
    *(Stand aktualisiert durch R-ARCH-06.)*
  * **test-engineer:** Stresstest M-6, Fuzz-Ziel für die Anspruchsprüfung und für `PlatformHooks`-Folgen gegen den
    GIC.
  * **Dokumente nach Freigabe** auf diesen ADR verweisen lassen: dbt.md §1, §7.5, §8.4, §10.1, §10.3–§10.5,
    O-DBT-14. platform.md §5.1, §5.2, §6, §7, §8.1, §11, §15 Frage 4 und 5. Die konkrete Änderungsliste steht im
    Bericht zu WP-ARCH-02.

## Überprüfung

Die Entscheidung ist falsch oder muss angepasst werden, wenn eine der folgenden Messungen bzw. Prüfungen scheitert.
Messorte: `bench/dbt-platform/` (neu) und die Zähler pro Exit- und Hook-Grund (dbt.md §15.3).

1. **M-1 Hook-Kosten:** T1-Mikrobench, Schleife aus `MRS`/`MSR` auf ein beanspruchtes Register mit `NullPlatform`.
   Ziel: Median ≤ 30 ns pro Zugriff. Mit echtem GIC: Paar `IAR1`+`EOIR1` ≤ 300 ns. Über 100 ns mit `NullPlatform`
   → zuerst das Helper-ABI prüfen (Spills, `dyn`), die Grenze bleibt.
2. **M-2 Exit-Kosten:** derselbe Bench mit `HVC` und `Handled`. Liegt der Exit-Rundlauf bei höchstens dem
   Doppelten eines Hook-Aufrufs **und** zeigt der ISA-Zensus (WP-DBT-19) nur Einzelregister-Zugriffe auf MMIO, ist
   der Hybrid zu komplex für seinen Nutzen. Dann wird Option B für Systemregister neu bewertet (neuer ADR).
3. **M-3 Raten beim ersten XNU-Boot** (WP-DBT-20, beim Nutzer, nicht in CI):
   * `Hypercall`-Exits > 10 000/s pro vCPU (etwa paravirtuelle Apple-Hypercalls) → synchroner Hypercall-Hook für die
     heißen Funktions-IDs.
   * `ICC_PMR_EL1`-Writes > 100 000/s pro vCPU (A-B3 fällt) → PMR-Schatten im DBT (die verworfene Variante, nur für
     `PMR`) per neuem ADR.
   * `CNTV_*`/`CNTP_*`-Hook-Aufrufe > 50 000/s pro vCPU → Vergleicher in den DBT verlegen (dbt.md-§10.4-Variante),
     Deadline über ein Atomic an die Plattform.
   * Ein IMPDEF-Register auf einem heißen Pfad → DBT-Intrinsic statt Hook (neuer ADR, A-2 in dbt.md fällt dann
     ohnehin).
4. **M-4 Interrupt-Latenz:** Bench mit Gerät-Thread → `set_irq_line` → Zeitstempel am IRQ-Vektor (Mini-Kernel),
   getrennt für „läuft“ und „parkt“, Ziele aus §6. Läuft die vCPU und p99 > 10 µs → fehlender Prüfpunkt (Fehler im
   DBT, nicht in der Grenze). Parkt sie und p99 > 200 µs → kurzes Spinnen vor dem Parken in `wait_for_wake` oder
   Timer-Slack/Prioritäten des vCPU-Threads anpassen. Erst wenn beides nicht reicht, wird die Grenze neu bewertet.
5. **M-5 Prüfkosten:** CoreMark-artiger Bench in T1 mit und ohne Blockeintritts-Prüfung. Über 1 % → Prüfung nur an
   Rückwärtskanten und indirekten Sprüngen, Latenzschranke neu berechnen. Signal-Kick bleibt verworfen.
6. **M-6 Verklemmungsfreiheit:** Stresstest (`tests/`, 8 vCPUs auf T0 und T1): SGI-Stürme über `ICC_SGI1R_EL1`,
   `TLBI …IS` plus `DSB`, MMIO auf ein Zählergerät, Stopp/Weiter alle 100 ms. 24 h ohne Hänger, jede SGI genau einmal
   quittiert, kein verlorenes Wakeup. Ein Hänger, der sich nicht durch die Regeln in §7.4/§7.5 beheben lässt, kippt
   die synchronen Hooks für den betroffenen Pfad.
7. **Spec-Ergebnis S-BOOT-1/PLAT-07:** Braucht der Gast Plattformwirkung, die weder als Hook noch als Exit an einer
   Befehlsgrenze ausdrückbar ist (etwa ein Systemregister, das `TbFlags` ändert, oder EL2 im Gast, platform.md §15
   Frage 10), ist dieser ADR zu ersetzen.

## Änderungen

* **2026-10-14, `architect` (WP-ARCH-06), Status bleibt Proposed:** Normative Prüfung der bisher nur sekundär
  belegten Regeln gegen Arm DDI 0487 M.d, Bericht [R-ARCH-06](../research/R-ARCH-06-ddi0487.md). Die Entscheidung
  (Option C) und die Schnittstellen-Skizze sind unverändert. Geändert:
  * Kontext, Fakten: Quellen für ISV, `HVC`/`SMC` und `WFI`/`WFE` durch Arm-Zitate ersetzt. `Handled` als bewusste
    Modellabweichung benannt. `WFE`-Fakt um das Löschen des globalen Monitors und die Event-Register-Quellen
    ergänzt. Neuer Fakt zu Teilaktualisierung und Wiederholung bei Mehrfachzugriffen.
  * §5.2 Mehrfachzugriffe: Die SError-Regel ist entfallen, weil Arm Teilaktualisierung bei synchronem Data Abort
    erlaubt (`R_TNVSL`). Neu gelten der synchrone Abort und die Pflicht „höchstens ein Hook-Aufruf pro
    Einzelzugriff“.
  * §5.3: `IL = 1`, `EA = 0`, `WnR` bei atomaren Befehlen, `SET` nur mit FEAT_RAS (WP-ARCH-03).
  * §5.5 und Folgearbeiten: Verweis auf den ADR-0009-Nachtrag (§9).
  * §7.2: Trap von `WFI` aus EL0 nur ohne anstehenden Interrupt. `WFE`-Absatz mit vollständigen Weckereignissen und
    Event-Register-Quellen sowie Monitor-Polling als Pflicht.
