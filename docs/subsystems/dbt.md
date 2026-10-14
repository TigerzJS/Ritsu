# DBT – Detaildesign (`ritsu-dbt`, `ritsu-mem`)

> Status: **Entwurf v0.1** (WP-M0-DBT, 2026-10-14). Autor-Rolle: `dbt-engineer`.
> Verbindlich sind die ADRs, vor allem [ADR-0006](../adr/0006-dbt-tiered-design.md) (mehrstufiger DBT),
> [ADR-0004](../adr/0004-guest-platform-vmapple.md) (vmapple-Plattform) und [ADR-0008](../adr/0008-clean-room.md)
> (Clean Room). Wo dieses Dokument einem ADR widerspricht, gilt der ADR.

**Markierungen in diesem Dokument**

| Marke | Bedeutung |
|---|---|
| **E-n** | Entscheidung dieses Designs. Änderbar durch ein neues Design-Review, bei Querschnittswirkung per ADR. |
| **A-n** | Annahme, die noch nicht verifiziert ist. Jede Annahme hat einen Prüfweg. |
| **O-DBT-n** | Offene Frage, gesammelt in §18. |
| ★ | Hohes Risiko (bei Work Packages: zuerst als Spike). |

Alle Leistungszahlen in diesem Dokument sind **Ziel- oder Schätzwerte**, keine Messungen (CLAUDE.md, Regel 8).
Gemessen wird ab WP-DBT-12 mit Benchmarks in `bench/`.

---

## Inhalt

1. [Überblick und Einordnung](#1-überblick-und-einordnung)
2. [Gastmodell](#2-gastmodell)
3. [Decoder und eine Semantik-Quelle](#3-decoder-und-eine-semantik-quelle)
4. [IR](#4-ir)
5. [Stufen T0/T1/T2](#5-stufen-t0t1t2)
6. [Registerabbildung und Flags](#6-registerabbildung-und-flags)
7. [Speicher: SoftMMU, TLB, RAM, MMIO, SMC](#7-speicher-softmmu-tlb-ram-mmio-smc)
8. [Speichermodell: ARMv8 → x86-TSO](#8-speichermodell-armv8--x86-tso)
9. [arm64e: PAC und BTI](#9-arm64e-pac-und-bti)
10. [Systemregister, Exceptions, Interrupts, Timer, WFI/WFE](#10-systemregister-exceptions-interrupts-timer-wfiwfe)
11. [SIMD/FP und Krypto](#11-simdfp-und-krypto)
12. [Gemeldeter CPU-Feature-Satz](#12-gemeldeter-cpu-feature-satz)
13. [Multi-Core](#13-multi-core)
14. [Persistenter Übersetzungs-Cache (AOT)](#14-persistenter-übersetzungs-cache-aot)
15. [Leistung, Budget, Profiling](#15-leistung-budget-profiling)
16. [Teststrategie](#16-teststrategie)
17. [Work Packages](#17-work-packages)
18. [Offene Fragen](#18-offene-fragen)
19. [Stand des Skeletts (M0)](#19-stand-des-skeletts-m0)

---

## 1. Überblick und Einordnung

Der DBT führt **jeden** Gast-Befehl aus: Kernel (XNU, EL1) und Userland (EL0). Es gibt keine
Hardware-Virtualisierung, die uns etwas abnimmt. Der DBT ist damit zugleich CPU, MMU und Teil des
Interrupt-Controllers (die CPU-Schnittstelle des GICv3 sind Systemregister).

```
                ritsu-vmm (vCPU-Threads, Verdrahtung)
                        │
          ┌─────────────┴──────────────┐
          ▼                            ▼
   ritsu-platform  ── Hooks ──►  ritsu-dbt ──────────────► ritsu-mem ──► ritsu-core
   (GIC, Timer-IRQ,  (MMIO,       Decoder, Semantik,       GuestMemory,
    MMIO-Geräte)      ICC_*,      T0/T1/T2, PAC,           SoftMMU (Translate),
                      WFI-Wake)   Code-Cache               TLB, Code-Seiten-Tracking
```

* `ritsu-dbt` kennt **keine Geräte**. MMIO, GIC-Systemregister (`ICC_*`) und Interrupt-Leitungen erreicht er
  über Traits, die `ritsu-platform` implementiert (Abhängigkeitsrichtung aus ARCHITECTURE.md §4 bleibt erhalten).
* `ritsu-mem` hält physischen Gastspeicher, die SoftMMU-Schnittstelle (`mmu::Translate`) und den TLB (`tlb::Tlb`).
* Der DBT läuft pro vCPU in einem eigenen Host-Thread (§13). JIT-Compiler-Threads für T2 laufen getrennt (§5.5).

### 1.1 Ausführungsschleife (Dispatcher)

```
loop {
    pending = vcpu.take_requests()          // TLB-Flush, Invalidierung, IRQ, Stop
    if irq_deliverable(pending, PSTATE.DAIF) { take_exception(IRQ); continue }
    key  = BlockKey { pa: translate(PC), va: PC, flags: TbFlags(vcpu) }
    code = jump_cache[PC] ?: code_cache.lookup(key) ?: tier_policy(key)   // T0 oder T1 übersetzen
    exit = code.run(&mut vcpu)              // kehrt mit Exit-Grund zurück
    handle(exit)                             // Exception, Helper, Sync, Chain-Patch
}
```

Translated code verlässt den Dispatcher nur über **verkettete Sprünge** (Block-Chaining) und kehrt bei jedem
nicht-direkten Ereignis zurück (Exception, Interrupt-Anforderung, Jump-Cache-Miss, Systemregister mit
Kontextwechsel).

## 2. Gastmodell

| Aspekt | Festlegung |
|---|---|
| Architektur | AArch64, **arm64e** (ARMv8.3-PAuth und folgende, Feature-Satz §12) |
| Exception Levels | **EL0 und EL1**. Kein EL2, kein EL3, kein Secure State, kein AArch32 (E-1). |
| Übersetzungsregime | Nur EL1&0, nur Stage 1, nur **16-KiB-Granule** (E-2, §7) |
| Plattform | vmapple-kompatibel (ADR-0004): GICv3 (Systemregister-Schnittstelle), Generic Timer, PL011, virtio |
| Endianness | Nur Little Endian (`SCTLR_EL1.EE/E0E = 0` fest, `ID_AA64MMFR0_EL1.BigEnd = 0`) |
| Reset | EL1h (`SP_EL1`), `DAIF = 1111`, MMU aus. Einstiegspunkt setzt der Bootpfad (`ritsu-platform`). |

**A-1:** Der vmapple-Kernelcache läuft in EL1 (wie unter Virtualization.framework, wo der Host-Hypervisor EL2
belegt) und benötigt kein EL2 (kein VHE, kein `HCR_EL2`). *Prüfweg:* Spike S-BOOT-1 / WP-DBT-19.

**A-2:** Der vmapple-Kernel nutzt keine proprietären Apple-Erweiterungen (GXF/Guarded Levels, SPRR/APRR,
AMX, Apple-`HID*`-Register, `ACTLR_EL1`-TSO-Bit außer als No-op), weil diese unter Virtualization.framework für
Gäste nicht verfügbar sind. *Prüfweg:* ISA-Zensus WP-DBT-19. Fällt die Annahme, braucht es einen ADR.

Für AArch32 gilt: `ID_AA64PFR0_EL1.EL0 = EL1 = 0b0001` (nur AArch64). Ein `ERET` in einen AArch32-Zustand
ist dann ein *Illegal Exception Return* (`PSTATE.IL`).

## 3. Decoder und eine Semantik-Quelle

### 3.1 Anforderung aus ADR-0006

Die Befehlssemantik wird **einmal** beschrieben (Decoder → IR-Lowering), T0, T1 und T2 leiten sich daraus ab.
Zwei naheliegende Umsetzungen haben Haken:

* *T0 interpretiert die IR.* Einheitlich, aber T0 ist dann an IR-Bau pro Befehl gebunden und als Orakel nicht
  mehr unabhängig vom IR-Lowering.
* *T1 übersetzt aus der IR.* Der IR-Bau kostet Zeit (Allokation, Werte-Nummerierung). Das gefährdet das Ziel
  < 1 µs pro Block.

### 3.2 Entscheidung: generische Semantik-Beschreibung mit drei Backends (E-3)

Jede Befehlsklasse wird **einmal** als Rust-Funktion über einem Trait `Sem` beschrieben. Das Trait abstrahiert
Werte und Operationen. Drei Implementierungen instanziieren dieselbe Beschreibung:

| Backend | `Sem::V` (Wert) | Ergebnis |
|---|---|---|
| **T0** `Concrete` | `u64`/`u128`/`[u8;16]` | führt sofort aus (Interpreter) |
| **T1** `Template` | Host-Register oder Temp-Slot | emittiert direkt x86-64 (kein IR-Objekt) |
| **T2** `IrBuilder` | `ir::Value` (SSA) | baut IR für Optimierung und Cranelift |

Skizze:

```rust
pub trait Sem {
    type V: Copy;            // Wert
    type F: Copy;            // abstrakter NZCV-Wert (lazy)
    type Abort;              // T0: Gast-Exception; T1/T2: core::convert::Infallible

    fn imm(&mut self, ty: Ty, v: u64) -> Self::V;
    fn gpr(&mut self, r: GpReg, ty: Ty) -> Self::V;
    fn set_gpr(&mut self, r: GpReg, ty: Ty, v: Self::V);
    fn add_with_carry(&mut self, ty: Ty, a: Self::V, b: Self::V, cin: Self::V) -> (Self::V, Self::F);
    fn set_nzcv(&mut self, f: Self::F);
    fn load(&mut self, ty: Ty, addr: Self::V, a: MemAttrs) -> Result<Self::V, Self::Abort>;
    fn branch(&mut self, cond: Option<(Cond, Self::F)>, target: Target);
    // … ca. 150 Operationen
}

fn sem_add_sub_imm<S: Sem>(s: &mut S, i: AddSubImm) -> Result<(), S::Abort> {
    let ty = if i.sf { Ty::I64 } else { Ty::I32 };
    let a = s.gpr(i.rn, ty);
    let imm = s.imm(ty, i.imm());
    let (b, cin) = match i.op { Add => (imm, s.imm(Ty::I1, 0)), Sub => (s.not(ty, imm), s.imm(Ty::I1, 1)) };
    let (r, f) = s.add_with_carry(ty, a, b, cin);
    if i.set_flags { s.set_nzcv(f) }
    s.set_gpr(i.rd, ty, r);
    Ok(())
}
```

**Regel für Beschreibungen:** Rust-Kontrollfluss nur über **Decode-Zeit-Konstanten** (Felder des `Inst`).
Datenabhängige Entscheidungen laufen über `Sem`-Operationen (`select`, `branch`, `if_then` mit Rückgabewert).
Damit ist jede Beschreibung für alle drei Backends gleich gültig.

**Konsistenz der Stufen** entsteht dann auf drei Ebenen:

1. **Eine Beschreibung** pro Befehl (ca. 1500 A64-Befehlsformen inkl. NEON).
2. **Operations-Konformanztests:** Jede der rund 150 `Sem`-Operationen wird in allen drei Backends mit
   Zufalls- und Randwerten gegeneinander geprüft. Die Divergenzfläche schrumpft von "1500 Befehle × 3" auf
   "150 Operationen × 3".
3. **Differenztests** T0 ↔ T1 ↔ T2 ↔ Referenz-Hardware auf Befehlsebene (§16).

### 3.2.1 Umsetzung (WP-DBT-01)

Code: `crates/ritsu-dbt/src/sem/`. Die Beschreibungen stehen in `sem/a64.rs` (`sem::lower(s, inst)` verteilt
ein `Inst` auf seine Beschreibung), die Pseudocode-Primitive (`AddWithCarry`, `ConditionHolds`, Shifts mit
`ShiftReg`-Semantik, Vergleiche) in `sem/prim.rs`, das T0-Backend in `sem/concrete.rs`, das T2-Backend in
`ir/builder.rs`. `interp.rs` enthält nur noch Fetch und Dispatch.

| Gruppe | `Sem`-Operationen (Stand WP-DBT-01) |
|---|---|
| Konstanten/PC | `imm(ty, v)`, `pc_rel(offset)` |
| Gast-Zustand | `gpr(r, ty)`, `set_gpr(r, ty, v)`, `nzcv()`, `set_nzcv(f)` |
| Integer | `add_with_carry(ty, a, b, cin) -> (V, F)`, `binop(op, ty, a, b)` mit `op ∈ {Add, Sub, And, Or, Xor, Shl, Lshr, Ashr, Ror}`, `not`, `icmp(cc, ty, a, b)` (Cranelift-`IntCC`-Satz), `select`, `cond_holds(cond, f)` |
| Kontrolle | `branch(Option<cond>, offset, kind)`, `branch_indirect(target, kind)`, `raise(exception)` |

Abweichungen von der Skizze oben, jeweils mit Grund:

* **PC nur relativ** (`pc_rel`, `branch` mit Offset): Beschreibungen sehen nie die absolute PC. T1/T2 können so
  positionsunabhängigen Code erzeugen (§14); in der IR sind alle Adressen relativ zum Regionseinstieg.
* **`Ty` umfasst nur skalare Integer bis 64 Bit** (`I1, I8, I16, I32, I64`), Werte in kanonischer Form (Bits
  oberhalb der Breite sind 0). 128-Bit- und Vektorwerte bekommen in WP-DBT-10 einen eigenen assoziierten
  Werttyp, damit T0-Integercode nicht mit `u128` rechnet.
* **`BranchKind` (`Jump`, `Call`, `Return`)** ist nur ein Hinweis für Vorhersagestrukturen (Return-Stack, §5.2)
  und ändert nie die Architekturwirkung.
* **Reihenfolge-Regeln** statt Transaktionen: Alles, was eine Exception auslösen kann, kommt vor dem ersten
  Architektur-Schreibzugriff; höchstens ein Kontrolltransfer pro Befehl, als letzte Operation. Debug-Builds von
  `Concrete` prüfen beides und zusätzlich, dass jeder Wert mit dem Typ benutzt wird, mit dem er erzeugt wurde.
* Speicher- und Systemregister-Operationen (`load`, `store`, `MemAttrs`, …) kommen mit WP-DBT-02/-06/-07 dazu.

Prüfung: Operations-Konformanztests (`sem/conformance.rs`) prüfen jede Operation des `Concrete`-Backends über das
Trait gegen unabhängig formulierte Orakel (128-Bit-Arithmetik, Bit-für-Bit-Schleifen, ausgeschriebene
Bedingungstabelle), über alle Typen, Rand- und Zufallswerte. `ir/tests.rs` baut für jedes M0-Befehlswort eine
Region, verifiziert sie und spielt sie auf `Concrete` zurück; Zustand und Fehler müssen exakt T0 entsprechen
(auch für einen ganzen Schleifenblock). Stichprobenhafte Mutationen in Primitiven, Beschreibungen und Builder
wurden von diesen Tests erkannt.

**Helper-Fallback:** Für seltene oder komplexe Befehle darf T1/T2 eine Operation als Aufruf der T0-Semantik
emittieren (`call helper_exec_inst(state, word)`). Das ist per Konstruktion exakt und erlaubt schrittweises
Ausrollen. Anforderung: Der Block schreibt vorher alle gecachten Gast-Register zurück.

### 3.3 Decoder

* Handgeschriebener, hierarchischer Decoder entlang des **A64 Encoding Index** des Arm ARM (`op0`
  = `word[28:25]`, dann Klassen-Unterfelder), reine Funktion `decode(u32) -> Inst`, kein Zustand, keine
  Allokation, kein Panic. Was nicht erkannt wird, ist `Inst::Unknown(word)` → Undefined-Instruction-Exception
  (`ESR_EL1.EC = 0x00`).
* `Inst` trägt bereits **aufgelöste Operanden**: Registerfeld 31 wird pro Operand als `GpReg::Sp` oder
  `GpReg::Zr` dekodiert (z. B. `ADD (imm)`: `Rd|SP`, `Rn|SP`; `ADDS`: `Rd|ZR`, `Rn|SP`; `MOV*`, `CBZ`, `BR`: ZR).
  Executoren müssen die Regel nicht kennen. Im M0-Skelett umgesetzt und getestet.
* Immediates werden beim Dekodieren normalisiert (Branch-Offsets vorzeichenerweitert × 4). Ausnahme:
  `DecodeBitMasks` für logische Immediates wird ebenfalls im Decoder berechnet und **gesondert exhaustiv**
  getestet (alle 2^13 `N:immr:imms`-Kombinationen pro Breite gegen die Pseudocode-Transkription), weil das ein
  klassischer Fehlerherd ist.
* **Hints:** Nicht zugewiesene `HINT`-Kodierungen verhalten sich architektonisch wie `NOP`. Zugewiesene Hints
  (`PACIASP`, `AUTIBSP`, `BTI`, `ESB`, `CSDB`, `YIELD`, `WFE`, `WFI`, `SEV`, …) werden einzeln dekodiert, weil
  mehrere davon auf arm64e echte Semantik haben. Bis dahin liefert der Decoder `Unknown` (lieber eine laute
  Exception als still falsch, Regel 5).
* **Gemeldete vs. ausführbare Features:** Befehle eines nicht gemeldeten Features (z. B. `BC.cond`, FEAT_HBC)
  sind `Unknown`, auch wenn sie leicht zu implementieren wären. So bleibt der Decoder deckungsgleich mit den
  ID-Registern (§12).
* **Decode-Cache für T0:** pro physischer 16-KiB-Seite ein Feld von 4096 dekodierten `Inst`, invalidiert wie
  Übersetzungen (§7.6).

**E-4:** Eine Generierung des Decoders aus Arms maschinenlesbarer Spezifikation (A64-XML/ASL) wäre ideal, die
Lizenzlage ist aber ungeklärt (O-DBT-11). Bis dahin handgeschrieben, abgesichert durch einen nächtlichen
**exhaustiven Klassifikationsvergleich über alle 2^32 Wörter** gegen einen externen Disassembler (LLVM
`llvm-mc --disassemble` oder Capstone, nur als Testwerkzeug, keine Laufzeitabhängigkeit). Verglichen wird nur
"zugewiesen/nicht zugewiesen" und die Befehlsklasse.

## 4. IR

Die IR existiert nur für **T2** (und Analysen). T0 und T1 brauchen sie dank E-3 nicht.

### 4.1 Struktur

* **Region** = Kontrollflussgraph aus IR-Blöcken; ein IR-Block = gerader Code ohne innere Sprünge.
* **SSA mit Block-Parametern** (wie Cranelift, keine Phi-Knoten). Das Lowering nach Cranelift ist dadurch 1:1.
* **Typen:** `I1, I8, I16, I32, I64, I128, F16, F32, F64, V128, Flags`. `Flags` ist ein abstrakter
  NZCV-Wert, der erst bei Bedarf materialisiert wird (§6.3).
* Jede Operation trägt die **Gast-PC** ihres Ursprungsbefehls (für präzise Exceptions, Profiling und
  jitdump-Debuginfo).
* **Umsetzung (WP-DBT-01):** `crates/ritsu-dbt/src/ir/`. `Region` (Blöcke + Wertetabelle), `Block`, `Value`,
  `Type`, `Op` (1:1 die `Sem`-Operationen, §3.2.1), `Terminator` (`Jump`, `Branch`, `Exit`), `Exit` (`Direct`,
  `Indirect`, `Raise`). PC-Werte sind Offsets zum Regionseinstieg (`pc_offset`, `PcRel`, `Exit::Direct`).
  `ir::Builder` ist das `Sem`-Backend, das die IR erzeugt; `Region::verify` prüft Terminierung, Sprungziele,
  Argumentanzahl, Typen, Einmal-Definition und Dominanz (Verfügbarkeit auf allen Pfaden). Speicher-, PAC-,
  FP- und Helper-Operationen aus §4.2 folgen mit den jeweiligen WPs; Codegen erst in WP-DBT-17.

### 4.2 Operationen (Auswahl)

| Gruppe | Operationen | Anmerkung |
|---|---|---|
| Gast-Zustand | `GetGpr/SetGpr`, `GetVreg/SetVreg`, `GetNzcv/SetNzcv`, `GetSp/SetSp`, `ReadSys/WriteSys(id)` | **explizit**; T2-Pass *State Promotion* hebt sie wie `mem2reg` in SSA-Werte und speichert nur an Exits |
| Arithmetik | `Add, Sub, AddWithCarry→(V,Flags), Mul, MulHiS/U, DivS/U, Shl/Lshr/Ashr/Ror, Clz, Cls, Rbit, Bswap, Ext/Trunc, Select` | `DivS/U`: Arm liefert bei Division durch 0 **0** und bei `INT_MIN / -1` **INT_MIN**; x86 `IDIV/DIV` lösen `#DE` aus → Lowering mit Wächter |
| Flags | `CondTest(cond, Flags)→I1`, `MaterializeNzcv(Flags)→I32`, `FlagsFromNzcv(I32)` | Flags-Elimination über Lebendigkeit |
| Speicher | `Load{ty, ext, mmu_idx, align, order}`, `Store{…}`, `Fence(kind)`, `LoadEx`, `StoreEx→status`, `AtomicRmw{op}`, `Cas`, `Cas128`, `ClearEx` | `order ∈ {Plain, Acquire, AcquirePc, Release}`; Lowering mit Inline-TLB-Fastpath (§7.3) |
| PAC | `PacSign{key, ptr, mod}`, `PacAuth{…}`, `PacStrip{data/insn}`, `PacGa` | eigene Ops, damit T2 Redundanzen entfernt (§9) |
| FP/SIMD | skalare und Vektor-Ops mit explizitem `FpEnv`-Operand (aus FPCR) | `FpEnv` als Wert macht FPCR-Abhängigkeit für Optimierungen sichtbar |
| Kontrolle | `Jump(block, args)`, `Branch(I1, …)`, `Exit{target, kind}`, `CallHelper{id, args, effects}` | `kind ∈ {Direct(pc), Indirect(V), Return(V), Exception, Sync}` |

**Seiteneffekt-Klassen** für `CallHelper`: `Pure`, `ReadsState`, `WritesState`, `MayRaise`, `EndsBlock`
(z. B. `MSR SCTLR_EL1`). `MayRaise` und `WritesState` zwingen State Promotion zum Zurückschreiben vor dem Aufruf.

### 4.3 Side Exits und präziser Zustand

Ein `Exit` und jeder Slow-Path einer `MayRaise`-Operation erhalten implizit die Liste der **dirty** Gast-Werte
(nach State Promotion). Das Lowering erzeugt dafür einen **kalten Stub** pro Stelle: Werte zurückschreiben,
`PC` setzen, Helper aufrufen bzw. zum Dispatcher springen. Der heiße Pfad bezahlt dafür nichts.

### 4.4 Optimierungen in T2 (Reihenfolge)

1. State Promotion (Gast-Register → SSA)
2. Konstantenfaltung und -propagation (`ADRP+ADD`, `MOVZ/MOVK`-Ketten → ein 64-Bit-Immediate)
3. Flags-Elimination (tote `Flags`-Werte; `CMP+B.cond` → `cmp+jcc`)
4. Redundante Lade-/Speicher-Elimination innerhalb der Region (nur `Plain`, nur ohne dazwischenliegende
   Barriere oder Helper mit `WritesState`)
5. PAC-Redundanz (§9.4)
6. Fence-Elimination für `LDAR` (§8.2)
7. Übergabe an Cranelift (Registerzuweisung, Instruktionsauswahl)

## 5. Stufen T0/T1/T2

### 5.1 T0 – Interpreter

* Referenz-Semantik und Orakel aller Differenztests (ADR-0006); führt kalten Code aus.
* `step(&mut CpuState, &impl GuestMemory) -> Result<StepOutcome, ExecError>` (M0, §19). Seit WP-DBT-01 über das
  `Concrete`-Backend der Semantik-Schicht (`interp.rs` macht nur Fetch und Dispatch).
* Kein `panic!` auf Gast-Daten: Jedes Gast-Problem ist ein `ExecError` und wird zur Gast-Exception. Bei
  `ExecError` bleibt der vCPU-Zustand unverändert (präziser Zustand am fehlerhaften Befehl).
* FP in T0 über eine **bit-exakte Software-FP-Implementierung** (Arm-`FPRound`-Pseudocode inkl. FPSR-Flags),
  nicht über Host-FP. Grund: Rust bietet keinen sicheren Zugriff auf Rundungsmodi und Exception-Flags (O-DBT-8).
* Zielwert (Schätzung): ≥ 30 MIPS mit Decode-Cache.

### 5.2 T1 – Template-/Basis-JIT

* Einheit: **ein Basisblock** (endet an Branch, Exception-erzeugendem Befehl, kontextänderndem
  Systemregister-Zugriff, `ISB`, Seitengrenze oder nach 64 Befehlen).
* Erzeugung über das `Template`-Backend der Semantik-Schicht: jede `Sem`-Operation emittiert direkt eine
  feste x86-64-Sequenz. Registerzuweisung blocklokal: Gast-Register werden beim ersten Gebrauch in einen Host-
  Register-Pool geladen und an Exits/Slow-Paths zurückgeschrieben (§6.1).
* **Ziel < 1 µs pro Block** (ADR-0006). Mittel: kein IR-Objekt, kein Heap pro Befehl, vorab berechnete
  Encodings, ein Durchlauf.
* **Block-Chaining:** Direkte Exits enden zunächst in einem Stub `jmp dispatcher_exit(slot)`. Findet der
  Dispatcher das Ziel, patcht er den `rel32` des Stubs auf den Zielblock (§13.4 zur Atomarität). Bedingte
  Branches erhalten zwei patchbare Exits.
* **Indirekte Sprünge** (`BR`, `BLR`, `RET`): Inline-Lookup im per-vCPU **Jump-Cache** (direkt abgebildet,
  4096 Einträge, Schlüssel = Gast-VA + TbFlags-Hash), Treffer → `jmp [entry.host]`, Fehlschlag → Dispatcher.
  `BL/BLR` legen zusätzlich `(Rücksprung-VA, Host-Adresse)` auf einen kleinen **Return-Stack** im vCPU-Zustand;
  `RET` vergleicht `X30` mit dem obersten Eintrag und springt bei Treffer direkt (§15.1).
* **Interrupt-Prüfung** am Blockanfang: `cmp byte [r15 + exit_request], 0; jne exit` (makrofusioniert, ca. 1
  Zyklus). Jede Schleife passiert damit mindestens einen Check pro Iteration.

### 5.3 T2 – optimierender Regionen-JIT (Cranelift)

* Einheit: **Region** (Superblock/Trace oder Schleifenkörper) aus heißen T1-Blöcken, gebildet aus
  Kantenprofilen, die T1 zählt.
* Pipeline: `IrBuilder`-Backend → Optimierungen (§4.4) → Cranelift → Code-Cache.
* Cranelift wird gepinnt, Updates nur mit Benchmark-Vergleich (ADR-0006).
* **A-3:** Cranelift reserviert auf x86-64 mit `enable_pinned_reg` das Register `r15` als *pinned register*.
  T1 nutzt deshalb ebenfalls `r15` als Zustandszeiger (§6.1), damit T1↔T2-Übergänge keine Umladung brauchen.
  *Prüfweg:* Cranelift-Quellen der gepinnten Version, Test in WP-DBT-17.

### 5.4 Tier-up-Heuristik (E-5, Startwerte, per Messung zu tunen)

| Übergang | Auslöser | Begründung |
|---|---|---|
| T0 → T1 | Blockeintritt Nr. **2** (Zähler in der Decode-Cache-Seite) | Interpretieren ≈ 30–50 ns/Befehl, Übersetzen ≈ 1 µs/Block (~10 Befehle): Break-even nach ca. 2–3 Ausführungen. Einmal laufender Boot-Code bleibt in T0. |
| T1 → T2 | Blockzähler (`sub dword [ctr], 1; jz tier_up`) erreicht **10 000** oder Rückwärtskante **1 000** Iterationen | heiße Schleifen schnell, sonst nur wirklich heißer Code |
| T2 → T2' | Region-Exit-Zähler zeigt häufigen Ausstieg aus einer Seitenkante | Region erweitern/neu bilden |
| Abstieg | Invalidierung (SMC, `IC`, DMA) | neue Übersetzung beginnt wieder bei T0/T1 |

### 5.5 Asynchrone Kompilierung auf E-Kernen

* T2-Aufträge gehen in eine Prioritätswarteschlange (Hotness). 2–4 Compiler-Threads, per Affinität auf
  **E-Kerne** gelegt (Kerntyp per CPUID-Leaf `0x1A` bzw. `/sys/devices/cpu_atom/cpus`).
* Die vCPU arbeitet währenddessen in T1 weiter. Fertiger T2-Code wird installiert, indem (1) der Eintrag im
  `TranslationCache` ersetzt und (2) der Einstieg des T1-Blocks auf die T2-Region gepatcht wird.
* Ein Auftrag trägt die **Generation** jeder beteiligten Seite. Ist bei Installation eine Seite inzwischen
  invalidiert, wird das Ergebnis verworfen (kein Lock während des Kompilierens nötig).

### 5.6 Invalidierung

* Übersetzungen werden **pro physischer Seite** registriert (eine Übersetzung überschreitet nie eine
  16-KiB-Seitengrenze; Regionen in T2 dürfen mehrere Seiten umfassen und sind bei jeder registriert).
* Pro Seite: Intervallliste der übersetzten Bytebereiche. Eine Invalidierung trifft nur Übersetzungen, die den
  geänderten Bereich überlappen (wichtig für Gast-JITs, die neben laufendem Code schreiben).
* Invalidierte Übersetzungen werden **entkettet** (eingehende Chain-Sprünge zurück auf Dispatcher-Stubs) und
  über **Epoch-basierte Freigabe** gelöscht, sobald jede vCPU einen Ruhepunkt (Dispatcher) passiert hat.
* Auslöser: §7.6 (SMC, Cache-Wartung, DMA).

## 6. Registerabbildung und Flags

### 6.1 Host-Register (E-6)

x86-64 hat 16 GPRs gegenüber 31 + SP in AArch64. Festlegung für T1 (T2: Cranelift verteilt frei, nur `r15` fest):

| Host | Rolle |
|---|---|
| `rsp` | Host-Stack (nie Gast-Daten) |
| `r15` | **Zeiger auf `CpuState` + 128** (Bias: Offsets 0…255 sind dann mit `disp8` erreichbar – genug für `X0`–`X30` (248 Byte) und ein weiteres Feld; die endgültige Feldreihenfolge mit den heißesten Feldern vorn legt WP-DBT-12 fest) |
| `rax, rcx, rdx` | Scratch (benötigt von `MUL/DIV`, Shifts über `cl`, `CMPXCHG` über `rax`) |
| `rsi, rdi, r11` | Scratch, Helper-Argumente, TLB-Fastpath |
| `rbx, rbp, r12, r13, r14` | blocklokaler Cache für Gast-Register (callee-saved → überleben Helper-Aufrufe) |
| `r8, r9, r10` | blocklokaler Cache (caller-saved → vor Helper-Aufrufen sichern) |

**Inter-Block-ABI (E-7):** An Blockgrenzen liegt der kanonische Gast-Zustand **im Speicher** (`CpuState`), nur
`r15` ist gesetzt. Vorteile: T1- und T2-Code sind frei kombinierbar und verkettbar, Persistenz (§14) braucht
keine Registerkonvention, präzise Exceptions sind einfach. Kosten: Laden/Speichern an Blockgrenzen; x86
Store-to-Load-Forwarding hält das billig (Schätzung 4–5 Zyklen Latenz pro Abhängigkeit). Alternative mit fest
gepinnten heißen Gast-Registern (z. B. `SP`, `X0`, `X1`, `X29`, `X30`) über Blockgrenzen hinweg: erst nach
Messung (O-DBT-6), weil sie für T2 eine eigene Aufrufkonvention in Cranelift bräuchte.

`CpuState` ist `#[repr(C)]`; die Feldreihenfolge ist ab WP-DBT-12 Teil der JIT-ABI (Offsets per
`core::mem::offset_of!`, Test sichert sie).

### 6.2 NZCV ↔ EFLAGS

| Arm | x86 nach `ADD`/`ADDS` | x86 nach `SUB`/`CMP` |
|---|---|---|
| N | SF | SF |
| Z | ZF | ZF |
| C | CF | **¬CF** (Arm: C = 1 heißt *kein Borrow*; x86: CF = 1 heißt *Borrow*) |
| V | OF | OF |

Bedingungen direkt nach `SUBS/CMP` (häufigster Fall) ohne Materialisierung:

| Arm | x86 `jcc` | | Arm | x86 `jcc` |
|---|---|---|---|---|
| EQ / NE | `je` / `jne` | | GE / LT | `jge` / `jl` |
| CS(HS) / CC(LO) | `jae` / `jb` | | GT / LE | `jg` / `jle` |
| HI / LS | `ja` / `jbe` | | MI / PL | `js` / `jns` |
| VS / VC | `jo` / `jno` | | AL / NV | `jmp` |

Nach `ADDS/CMN` gilt für C dagegen CF direkt: HS → `jb`, LO → `jae`; **HI** (C ∧ ¬Z) und **LS** haben dort
kein einzelnes x86-Gegenstück und brauchen eine kurze Sequenz. Weitere Fallen:

* `ADC/ADCS` passt direkt (`adc`). **`SBC/SBCS`** verwendet Arms C als *Nicht-Borrow*: vor `sbb` muss der
  Carry invertiert werden (`cmc`) und danach erneut, wenn C weiterverwendet wird.
* `ANDS/TST` setzen in Arm C = V = 0; x86 `and/test` ebenso (CF = OF = 0) → passt.
* `CCMP/CCMN`, `SETF8/16`, `RMIF`, `CFINV`, `AXFLAG/XAFLAG` (FlagM/FlagM2) → kurze Sequenzen bzw. Helper.
* x86 `inc/dec` lassen CF unverändert – werden für Flag-setzende Gast-Ops nie verwendet.

### 6.3 Lazy Flags (E-8)

* **Innerhalb eines Blocks** leben Flags in EFLAGS, solange kein Flag-zerstörender Host-Befehl dazwischen liegt;
  sonst `Flags`-Wert im Template-Backend = (Operation, Operand A, Operand B, Ergebnis) in Scratch-Slots.
* **An Blockgrenzen** wird NZCV nur materialisiert, wenn nicht bewiesen ist, dass es tot ist. T1 macht das
  konservativ (immer, wenn der Block Flags gesetzt hat), T2 per Lebendigkeitsanalyse über die Region.
* Materialisierung (Arm-Form, `C` korrekt invertiert): `setcc`×4 + Kombination, oder `lahf`+`seto`
  + Tabelle. Speicherform im `CpuState`: Layout des `NZCV`-Systemregisters (Bits 31..28), siehe `cpu::Nzcv`.

## 7. Speicher: SoftMMU, TLB, RAM, MMIO, SMC

### 7.1 Stage-1-Walk mit 16-KiB-Granule

Nur das 16-KiB-Granule wird gemeldet (`ID_AA64MMFR0_EL1.TGran16 = 0b0001`, `TGran4 = TGran64 = 0b1111`)
(E-2, **A-4:** macOS/XNU benutzt auf arm64 ausschließlich 16 KiB; *Prüfweg:* WP-DBT-19).

| Level | VA-Bits (bei 48-Bit-VA, `TxSZ = 16`) | Einträge | Blockgröße |
|---|---|---|---|
| 0 | [47] | 2 | – (keine Blöcke) |
| 1 | [46:36] | 2048 | – (64-GiB-Blöcke nur mit FEAT_LPA2, nicht gemeldet) |
| 2 | [35:25] | 2048 | **32 MiB** |
| 3 | [24:14] | 2048 | **16 KiB** (Seite) |

* `TCR_EL1.{T0SZ,T1SZ}` bestimmt Startlevel und VA-Größe; Kernel nutzt `TTBR1` (obere Hälfte; VA-Bit 55
  = 1 wählt `TTBR1`, auch wenn TBI das Top-Byte ignoriert), Userland `TTBR0`.
* Geprüft werden: Valid-Bit, Deskriptortyp pro Level, `AF` (kein Hardware-Update: `HAFDBS = 0` →
  Access-Flag-Fault, Kernel setzt AF in Software; **A-5**), `AP[2:1]`, `UXN/PXN`, `SCTLR_EL1.WXN`, `PAN`
  (`PSTATE.PAN` + `SCTLR_EL1.SPAN`), `nG` (ASID-gebunden), Adressgrößen gegen `TCR_EL1.IPS`/PARange.
* Das **Contiguous-Bit** ist ein Hinweis und darf ignoriert werden (einzelne Einträge sind architekturkonform).
* Fehler werden zu `mmu::TranslationFault` mit `FaultKind` und Level; `FaultKind::fsc()` liefert den
  `DFSC/IFSC`-Code für `ESR_EL1` (im Skelett umgesetzt und gegen den Arm ARM getestet).
* Seitentabellen werden über `GuestMemory` (physisch) gelesen. Walks auf nicht vorhandenen physischen Speicher
  → *Synchronous External Abort on translation table walk* (`FaultKind::ExternalAbortOnWalk`).
* `AT S1E1R/W`, `AT S1E0R/W`, `AT S1E1RP/WP` (PAN2) → Walk ohne Exception, Ergebnis in `PAR_EL1`.

### 7.2 ASIDs, TBI, TLB-Organisation

* 16-Bit-ASIDs (`ASIDBits = 0b0010`, `TCR_EL1.AS`). ASID aus `TTBR0_EL1` oder `TTBR1_EL1` (`TCR_EL1.A1`).
* **TBI** (`TCR_EL1.TBI0/TBI1`, getrennt für Daten und Befehle über `TBID0/1`): Top-Byte wird vor dem
  TLB-Lookup per Sign-Extension aus Bit 55 kanonisiert (2 Host-Befehle: `shl 8; sar 8`). Ob TBI aktiv ist,
  steckt in `TbFlags` → Code wird darauf spezialisiert.
* **mmu_idx (E-9):** pro vCPU drei TLB-Felder: `EL0`, `EL1`, `EL1+PAN`. Die Rechteprüfung steckt damit in der
  Feldwahl, die zur Übersetzungszeit aus `TbFlags` feststeht. `LDTR/STTR` aus EL1 benutzen das `EL0`-Feld.
* **ASID-Wechsel (E-10, Start):** Der JIT-Fastpath vergleicht nur die VA. Einträge im Fastpath-TLB gelten daher
  nur für die *aktuelle* ASID (+ globale). Beim Schreiben eines `TTBR0_EL1` mit neuer ASID werden die
  nicht-globalen Einträge verworfen. Später (nach Messung): k = 4 TLB-Bänke pro mmu_idx mit LRU über ASIDs, so
  dass Kontextwechsel zwischen wenigen Prozessen ohne Neufüllung bleiben.
* Der **Referenz-TLB** `ritsu_mem::tlb::Tlb` (M0) speichert ASID und Global-Bit explizit und implementiert die
  TLBI-Semantik exakt (`VMALLE1`, `ASIDE1`, `VAE1`, `VAAE1`). Er ist Grundlage und Orakel für das
  Fastpath-Layout.
* **Blockabbildungen** (32 MiB) werden als 16-KiB-Fragmente gecacht. Weil `TLBI VAE1` auf eine Adresse im Block
  den *ganzen* Block invalidieren muss (jede gecachte Übersetzung, die für diese VA benutzt werden könnte), führt
  der TLB das geschlossene VA-Intervall `[lo, hi]` mit, das alle seit dem letzten vollen Flush eingetragenen
  Blöcke umspannt. Ein VA-Flush innerhalb des Intervalls wird zum vollen Flush; das ist eine zulässige Obermenge,
  weil Software sich nie auf den Inhalt eines TLB verlassen darf. Außerhalb des Intervalls trifft ein VA-Flush
  nur seinen eigenen Slot. Das Intervall schrumpft nur beim vollen Flush und deckt daher immer alle noch
  vorhandenen Fragmente ab (umgesetzt und getestet, inkl. Blöcken am oberen Ende des Adressraums).

### 7.3 Inline-Fastpath (T1/T2)

Eintrag (32 Byte, Index = `(VA >> 14) & (N−1)`):

```
struct FastTlbEntry { cmp_read: u64, cmp_write: u64, cmp_exec: u64, addend: i64 }
```

`cmp_*` = seitenausgerichtete VA, deren niedrige 14 Bits **Flag-Bits** sind (`INVALID`, `MMIO`,
`NOTDIRTY`/Code-Seite, `WATCH`). `addend` = Host-Adresse − Gast-VA. Ein 8-Byte-Lesezugriff:

```nasm
; rsi = Gast-VA (bereits TBI-kanonisiert), r15 = &CpuState + 128
mov   rax, rsi
shr   rax, 9                          ; (VA >> 14) * 32
and   eax, (N-1) << 5
lea   rdx, [rsi + 7]                  ; letztes Byte des Zugriffs
and   rdx, -0x4000                    ; dessen Seitenbasis, Flag-Bits = 0
cmp   rdx, [r15 + rax + TLB_EL1 + 0]  ; cmp_read
jne   .slow_17                        ; Miss, Flag gesetzt, oder Seitenüberschreitung
add   rsi, [r15 + rax + TLB_EL1 + 24] ; addend → Host-Adresse
mov   rcx, [rsi]                      ; eigentlicher Zugriff
```

* Seitenüberschreitende Zugriffe fallen automatisch in den Slow-Path (die Seite von `VA+7` passt nicht zum
  Tag der Seite von `VA`). Arm erlaubt unausgerichtete Zugriffe auf Normal Memory, der Slow-Path teilt sie.
* Ausrichtungspflicht (Exclusives, Atomics, `SCTLR_EL1.A`, Device-Speicher) wird vor dem Lookup geprüft.
* `TLB_EL1` liegt im `CpuState` (über `r15` mit `disp32` erreichbar), kein weiteres Register nötig.
* Geschätzte Kosten: 8–9 Befehle, ~3–5 Zyklen auf dem Treffer-Pfad.

### 7.4 Host-Abbildung des Gast-RAM

* Gast-RAM liegt als **ein `memfd`**, einmal in den Host-Adressraum gemappt (mit Guard-Bereichen davor und
  dahinter), bevorzugt mit 2-MiB-Huge-Pages (THP/hugetlbfs) gegen Host-TLB-Druck.
* `addend` im Fast-TLB zeigt direkt hinein. Einträge entstehen **nur** für RAM-Seiten → jeder Fastpath-Zugriff
  liegt per Konstruktion in der RAM-Abbildung (Q-SEC-1).
* Die eingefrorene `GuestMemory`-Schnittstelle (`write(&mut self, …)`) bleibt der Weg für Geräte/DMA und T0.
  Für mehrere vCPUs, die **gleichzeitig** und **atomar** auf denselben RAM zugreifen, ist sie ungeeignet
  (`&mut` erzwingt Exklusivität, keine Atomics). Der vCPU-Pfad braucht eine zusätzliche Schnittstelle auf
  Host-Zeiger-Basis (Modul `host_map`, `unsafe` mit Begründung) → **O-DBT-1** (architect/ADR).

### 7.5 MMIO-Slow-Path

* Seiten ohne RAM-Backing erhalten `MMIO` im TLB-Tag → Slow-Path → `MmioBus`-Trait (implementiert von
  `ritsu-platform`): `read(pa, size) -> Result<u64, BusError>`, `write(pa, size, value)`.
* Zugriffe auf Device-Speicher sind in Programmreihenfolge und nie spekulativ; da jeder MMIO-Zugriff ein
  Helper-Aufruf ist, ist das automatisch erfüllt. Unausgerichtete Zugriffe auf Device-Speicher → Alignment Fault.
* `LDP/STP` und `LD1` auf MMIO werden in Einzelzugriffe in Architekturreihenfolge zerlegt.
* Fehlerhafte Gerätezugriffe → *Synchronous External Abort* bzw. gerätedefiniertes Verhalten, nie Panic.

### 7.6 Selbstmodifizierender Code und Cache-Wartung (E-11)

Gemeldet wird `CTR_EL0.DIC = 0` (Befehls-Cache-Invalidierung **erforderlich**) und `IDC = 1`. Damit muss der
Gast nach jeder Code-Änderung die Arm-Sequenz `DC CVAU` → `DSB ISH` → `IC IVAU` → `DSB ISH` → `ISB` ausführen.

Zwei Mechanismen, beide aktiv:

1. **IC-getrieben (Korrektheitsbasis):** `IC IVAU` (VA → PA über die MMU des Ausführenden, Rechteprüfung nach
   `SCTLR_EL1.UCI` für EL0) invalidiert die Übersetzungen, die die 64-Byte-Zeile überlappen. `IC IALLU(IS)`
   invalidiert alle Seiten, die seit dem letzten `IALLU` beschrieben wurden (Dirty-Bitmap, Mechanismus 2),
   sonst alles. `IS`-Varianten wirken auf alle vCPUs (§13). Architekturkonform darf eine vCPU bis zu ihrem
   eigenen `ISB` noch alten Code ausführen.
2. **Schreibverfolgung (Absicherung + Beschleuniger):** Physische Seiten mit Übersetzungen sind in einer
   globalen Code-Bitmap markiert. TLB-Füllungen setzen für solche Seiten `NOTDIRTY` im `cmp_write`; ein
   Schreibzugriff läuft in den Slow-Path, markiert die Seite dirty und invalidiert die überlappenden
   Übersetzungen. DMA-Schreibzugriffe von Geräten markieren über das RAM-Backend ebenfalls dirty.

**A-6:** Gast-Software hält sich an das Arm-Protokoll für Code-Änderungen (bei `DIC = 0` Pflicht). Mechanismus 2
fängt die Fälle ab, in denen Software auf echter Hardware "zufällig" funktioniert. Ob Mechanismus 2 aus
Leistungsgründen eingeschränkt werden kann, entscheidet eine Messung (O-DBT-19).

`DC ZVA` (Nullen eines Blocks der Größe `DCZID_EL0.BS`) → Store-Sequenz, **keine** Non-Temporal-Stores
(schwach geordnet, §8). `DC CVAU/CIVAC/CVAC/CVAP` → No-op (Host-Caches kohärent) außer Rechteprüfung; `DC IVAC`
(Invalidierung ohne Clean) ist auf kohärentem Speicher ebenfalls No-op.

## 8. Speichermodell: ARMv8 → x86-TSO

### 8.1 Grundlage

ARMv8 ist *other-multi-copy-atomic* und schwach geordnet: Ohne Abhängigkeit oder Barriere dürfen alle vier
Kombinationen (Load→Load, Load→Store, Store→Store, Store→Load) umgeordnet werden. x86-TSO erlaubt nur
**Store→Load** (Store-Buffer) und ist ebenfalls multi-copy-atomic (bis auf Store-Forwarding im eigenen Kern).
Daraus folgt: Jede Arm-Ordnung außer Store→Load bekommen wir auf x86 umsonst.

### 8.2 Abbildung (E-12)

| Arm | x86 | Begründung |
|---|---|---|
| `LDR`/`STR` (Normal) | `mov` | TSO ist stärker |
| `DMB ISHLD`/`OSHLD`/`LD` | – | Load→Load, Load→Store sind auf TSO geordnet |
| `DMB ISHST`/`ST` | – | Store→Store ist geordnet |
| `DMB ISH`/`SY` (voll) | `mfence` (oder `lock or dword [rsp], 0`, je nach Messung) | einzige fehlende Ordnung: Store→Load |
| `DSB …` | wie `DMB` + **Synchronisationspunkt**: wartet auf Abschluss ausstehender TLBI/IC-Broadcasts dieser vCPU (§13) | DSB garantiert Abschluss von Wartungsoperationen |
| `ISB` | Blockende, Rückkehr in den Dispatcher (Kontextsynchronisation: neue Systemregister, neuer Code, Interrupts) | |
| `LDAPR`, `LDAPUR` (RCpc) | `mov` | TSO-Loads sind Acquire |
| `STLR`, `STLUR` | `mov` | TSO-Stores sind Release |
| `LDAR`, `LDAXR` (RCsc) | **`mfence` + `mov`** | RCsc: ein vorangehendes `STLR` darf nicht hinter ein `LDAR` rutschen (Store→Load) |
| `SB`, `CSDB`, `SSBB`, `PSSBB` | `lfence` (konservativ, Spekulationsbarriere) | kein Effekt auf die Speicherordnung |

**Warum der Zaun beim Load sitzt (E-12):** Die Regel ist "zwischen jedem `STLR` und jedem späteren
`LDAR`/`LDAXR` in Programmreihenfolge muss eine Store→Load-Barriere liegen". Sie kann am Store (`xchg` statt
`mov`) oder am Load stehen. Mit gemeldetem FEAT_LRCPC übersetzen Compiler `memory_order_acquire` zu `LDAPR`
(kein Zaun nötig) und nur `seq_cst`-Loads zu `LDAR`, während `STLR` für *jeden* Release-Store steht. `LDAR` ist
damit seltener als `STLR` → Zaun am Load ist billiger. T2 entfernt den Zaun, wenn seit der letzten vollen
Barriere (gelockter Befehl, `mfence`) in der Region kein `STLR` liegen kann. *Prüfweg:* Litmus-Tests
(WP-DBT-14) und Messung (O-DBT-7).

**Weitere Regeln**

* **Keine Non-Temporal-Stores** (`movnt*`) und keine `rep movs` mit Fast-String-Semantik für Gast-Stores, die
  Ordnung brauchen; beide sind auf x86 schwach geordnet.
* **Keine Split-Locks:** Gelockte x86-Befehle über eine Cache-Line-Grenze sind extrem langsam, und Linux meldet
  oder bestraft sie (`split_lock_detect`). Arm verlangt für Atomics ohnehin Ausrichtung (Alignment Fault vorher).
* Single-Copy-Atomarität: Arm garantiert sie für ausgerichtete Zugriffe bis 64 Bit, x86 ebenso → passt.

### 8.3 Exklusive Zugriffe (`LDXR`/`STXR`) (E-13) ★

Umsetzung über **CAS-Emulation** (wie QEMU MTTCG):

* `LDXR[B/H/W/X] Rt, [Xn]`: normaler Load; Monitor im vCPU-Zustand ← `(PA, Größe, gelesener Wert)`.
* `STXR Ws, Rt, [Xn]`: wenn Monitor gültig und Adresse/Größe passen: `lock cmpxchg [host], Rt` mit
  erwartetem Wert = gemerkter Wert. Erfolg → `Ws = 0`, sonst `Ws = 1`. Monitor wird in jedem Fall gelöscht.
* `LDXP/STXP` (128 Bit) → `lock cmpxchg16b` (16-Byte-Ausrichtung ist für das Paar ohnehin Pflicht).
* `CLREX`, jede Exception-Rückkehr (`ERET`) und jeder Exception-Eintritt löschen den Monitor.
* Arm erlaubt *spurious failure* von `STXR` → im Zweifel scheitern (z. B. Monitor nach Interrupt) ist korrekt.

**ABA-Problem:** Schreibt eine andere vCPU zwischen `LDXR` und `STXR` den Wert A → B → A, scheitert `STXR` auf
echter Hardware (der Monitor sieht den Schreibzugriff), bei uns gelingt er. Folgen:

* Für Mutex-, Zähler- und Referenzzähler-Muster harmlos (das Ergebnis ist trotzdem eine gültige Linearisierung).
* Gefährlich für lock-freie Strukturen, die sich auf LL/SC als ABA-Schutz verlassen (z. B. Stack-Pop ohne
  Versionszähler).

Gegenmaßnahmen (gestaffelt):

1. **LSE melden** (§12): Moderner Arm64-Code benutzt dann `CAS`/`LDADD`. Diese haben auf echter Hardware dieselbe
   ABA-Semantik wie unsere Emulation, Code dafür ist also ABA-fest geschrieben.
2. **Strenger Modus** (Diagnose, abschaltbar): Zwischen `LDXR` und `STXR` einer vCPU laufen alle anderen vCPUs
   nicht (Stop-the-World über die Request-Schleife, wie QEMUs `cpu_exec_step_atomic`). Korrekt, aber langsam;
   dient zum Eingrenzen von Fehlern.
3. **Nicht verfügbar:** Host-HTM (TSX/RTM) – auf Alder Lake nicht vorhanden. Seitenschutz-basierte Monitore sind
   zu teuer für den Normalbetrieb.

Bekannte Abweichung wird in einem eigenen Test dokumentiert (Test erwartet "STXR gelingt nach ABA"), damit eine
spätere Änderung bewusst geschieht. Akzeptanz → **O-DBT-5**.

### 8.4 LSE-Atomics

| Arm (alle Acquire/Release-Varianten) | x86 |
|---|---|
| `CAS`, `CASA/L/AL` (8–64 Bit) | `lock cmpxchg` |
| `CASP` (2×64) | `lock cmpxchg16b` |
| `SWP` | `xchg` (implizit gelockt) |
| `LDADD` | `lock xadd` |
| `LDCLR` (AND NOT), `LDEOR`, `LDSET` | `cmpxchg`-Schleife (x86 `lock and/xor/or` liefern den alten Wert nicht) |
| `LDSMAX/LDSMIN/LDUMAX/LDUMIN` | `cmpxchg`-Schleife |
| `STADD/STCLR/STEOR/STSET` (= `LD*` mit `XZR`) | `lock add/and/xor/or` direkt (`STCLR`: Operand vorher invertieren) |

Gelockte x86-Befehle sind volle Barrieren → Acquire/Release-Varianten sind automatisch erfüllt.
Atomics auf MMIO-Seiten → Slow-Path, Gerät entscheidet (in der Regel External Abort).

**128-Bit-Atomarität / FEAT_LSE2:** Mit LSE2 sind ausgerichtete 16-Byte-`LDP/STP` single-copy-atomar, und
Compiler nutzen das für `std::atomic<__int128>`-Loads. **A-7:** Intel garantiert Atomarität für 16 Byte
ausgerichtete SSE/AVX-Zugriffe (`MOVDQA`/`VMOVDQA` u. a.) auf Prozessoren mit AVX. *Prüfweg:* Intel SDM Vol. 3,
Abschnitt *Guaranteed Atomic Operations* der aktuellen Ausgabe, insbesondere ob `VMOVDQU` mit ausgerichteter
Adresse eingeschlossen ist (O-DBT-16).
Bis dahin wird LSE2 **nicht** gemeldet, sofern macOS es nicht voraussetzt (§12).

### 8.5 Gast-TSO-Modus

Apple-Kerne haben einen TSO-Modus (für Rosetta). Ein Gast, der ihn über ein (nicht gemeldetes) Apple-Register
anfordert, bekommt auf x86 TSO ohnehin; das Register wird als No-op behandelt, falls es überhaupt erreicht wird
(A-2).

## 9. arm64e: PAC und BTI

### 9.1 Zustand

Fünf 128-Bit-Schlüssel (`APIA`, `APIB`, `APDA`, `APDB`, `APGA`) liegen in `CpuState::pac` (M0-Platzhalter,
`Debug` geschwärzt: Schlüssel sind Gast-Geheimnisse und gehören nicht in Host-Logs). Freigabe pro Schlüssel über
`SCTLR_EL1.{EnIA, EnIB, EnDA, EnDB}` → Teil von `TbFlags`, damit Code darauf spezialisiert wird. Ist ein Schlüssel ausgeschaltet, lassen
`PAC*`/`AUT*` mit diesem Schlüssel den Zeiger unverändert; das gilt auch für die Hint-Kodierungen wie `PACIASP`.

### 9.2 Algorithmus (E-14)

* Die **Lage** der PAC-Bits ist architektonisch festgelegt (`AddPAC`/`Auth`-Pseudocode: Feld zwischen VA-Größe
  aus `TxSZ` und Bit 54 bzw. 63, abhängig von TBI; Bit 55 bleibt erhalten). Das wird exakt umgesetzt.
* Der **Algorithmus** ist wählbar: Wir melden `ID_AA64ISAR1_EL1.API = 0b0001` und `GPI = 0b0001`
  (*implementation defined*) statt `APA/GPA` (QARMA5) oder `APA3` (QARMA3).
* Der Gast braucht nur **Selbstkonsistenz**: Signieren und Prüfen laufen beide durch uns. **A-8:** Kein
  Apple-Bestandteil persistiert PAC-Werte über Schlüssel- oder Maschinengrenzen (Schlüssel werden pro Boot bzw.
  pro Prozess zufällig erzeugt; der dyld shared cache und der Kernelcache werden zur Laufzeit signiert).
  *Prüfweg:* Boot-Test; Signaturfehler zeigen sich sofort als PAC-Fault.
* Vorschlag: **AES-NI-basierte Mischfunktion** – 2–4 `AESENC`-Runden über `(Zeiger ‖ Modifier)` mit dem
  128-Bit-Schlüssel, gekürzt auf die PAC-Breite. Schätzung: 10–15 Zyklen statt > 100 für QARMA5 in Software.
  Sicherheit für den Gast: geringere kryptografische Stärke als QARMA. Das ist ein bewusster Kompromiss
  (Konfigurationsoption "stark" mit mehr Runden) → O-DBT-4.
* FEAT_PAuth2/EPAC/FPAC werden zunächst **nicht** gemeldet: `AUT*` mit falscher Signatur liefert dann einen
  korrumpierten Zeiger (Fehlerbits gemäß Pseudocode), der bei Benutzung eine Translation Fault auslöst. Ob macOS 27
  FPAC voraussetzt → §12/O-DBT-2.

### 9.3 Befehle

`PACIA/IB/DA/DB`, `PACIZA…`, `PACIASP/IBSP`, `PACIA1716…`, `AUTIA…` (alle Formen), `XPACI/XPACD/XPACLRI`,
`PACGA`, kombinierte Sprünge `BRAA/BRAB/BLRAA/BLRAB/RETAA/RETAB/ERETAA/ERETAB`, `LDRAA/LDRAB` (mit Writeback).
Die Hint-Kodierungen (`PACIASP` usw.) sind **keine** NOPs, sobald PAuth gemeldet ist.

### 9.4 Leistung

* `AUT*` = PAC neu berechnen + vergleichen + Bits entfernen → Inline-Sequenz mit AES-NI, ~15–25 Zyklen geschätzt.
* **PAC-Cache:** kleiner direkt abgebildeter Cache pro vCPU `(Zeiger, Modifier, Schlüssel-ID) → PAC`
  (Treffer ~5 Zyklen), invalidiert bei Schlüsselwechsel (`MSR AP*Key*`).
* **Fusion:** `PACIBSP … AUTIBSP; RET` und `BLRAA`/`LDRAA` werden als Einheit übersetzt.
* **T2-Redundanz-Elimination:** Ist ein Zeiger in der Region bereits mit demselben (Schlüssel, Modifier)
  authentifiziert worden und seitdem unverändert, entfällt die zweite Prüfung; `PACIBSP` am Prolog und `AUTIBSP`
  am Epilog derselben Region mit unverändertem `SP`/`LR` → nur Strip.

### 9.5 BTI (E-15)

FEAT_BTI wird zunächst **nicht** gemeldet (`ID_AA64PFR1_EL1.BT = 0`). Dann sind `BTI`-Befehle Hints (NOP), das
GP-Bit in Deskriptoren hat keine Wirkung, und `PSTATE.BTYPE` muss nicht verfolgt werden. **A-9:** macOS
arm64e verlässt sich auf PAC, nicht auf BTI. *Prüfweg:* WP-DBT-19. Wird BTI nötig: `BTYPE` in `TbFlags` (am
Blockanfang bekannt aus dem Sprungtyp) und Prüfung des ersten Befehls bei indirekten Sprüngen in GP-Seiten.

## 10. Systemregister, Exceptions, Interrupts, Timer, WFI/WFE

### 10.1 Systemregister

* `MRS/MSR` werden über `(op0, op1, CRn, CRm, op2)` aus einer **generierten Tabelle** aufgelöst: Name,
  Zugriffsrechte pro EL, Reset-Wert, RES0/RES1-Masken, Seiteneffekte (`EndsBlock`, `FlushesTlb`,
  `ChangesTbFlags`). Unbekannte/nicht gemeldete Register → Undefined-Exception (nie still ignorieren).
* Register, die Übersetzung oder TbFlags ändern (`SCTLR_EL1`, `TCR_EL1`, `TTBR0/1_EL1`, `MAIR_EL1`, `CPACR_EL1`,
  PAC-Schlüssel, `PSTATE.PAN/UAO/DIT/SSBS` über `MSR (imm)`), beenden den Block.
* GICv3-CPU-Interface (`ICC_IAR1_EL1`, `ICC_EOIR1_EL1`, `ICC_PMR_EL1`, `ICC_SGI1R_EL1`, …) → Trait
  `GicCpuInterface` von `ritsu-platform`; `ICC_SGI1R_EL1` erzeugt IPIs zwischen vCPUs.
* ID-Register liefern den Feature-Satz aus §12 (eine Quelle, aus der auch der Decoder seine Freischaltungen
  bezieht).

### 10.2 Exceptions

* **Synchron:** Undefined (`EC 0x00`), `SVC` (`0x15`), Instruction Abort (`0x20` niedrigeres EL / `0x21` gleiches
  EL), PC-Alignment (`0x22`), Data Abort (`0x24`/`0x25`), SP-Alignment (`0x26`), FP-Trap (`0x2C`), `BRK`
  (`0x3C`), MSR/MRS-Trap (`0x18`), FP/SIMD-Zugriff gesperrt (`0x07`, `CPACR_EL1.FPEN`), Illegal Execution State
  (`0x0E`).
* **Eintritt** (nach EL1): `ELR_EL1` ← Rücksprungadresse, `SPSR_EL1` ← PSTATE (`NZCV`, `DAIF`, `M[3:0]`, `PAN`,
  `UAO`, `DIT`, `SSBS`, `BTYPE`, `IL`, `SS`), `ESR_EL1` ← Syndrom, `FAR_EL1` bei Aborts, dann
  `PSTATE.{D,A,I,F} = 1`, `EL = 1`, `SP = SP_EL1`, `PAN = 1` falls `SCTLR_EL1.SPAN = 0`, Monitor löschen.
  Vektor: `VBAR_EL1` + Quelle (`0x000` EL1t, `0x200` EL1h, `0x400` aus EL0 AArch64) + Typ (`0x000` sync,
  `0x080` IRQ, `0x100` FIQ, `0x180` SError).
* **`ERET`:** `PC ← ELR_EL1`, `PSTATE ← SPSR_EL1` mit Prüfung auf illegale Rückkehr (Ziel-EL > aktuelles EL,
  AArch32-Ziel, reservierte `M`-Werte → `PSTATE.IL = 1`), Monitor löschen, Blockende.
* **Präziser Zustand:** T0 lässt den Zustand bei Fehlern unverändert (M0 umgesetzt). T1/T2 nutzen kalte Stubs
  pro fehleranfälliger Stelle (§4.3); Helper geben einen Status zurück, der Stub springt zum Exit. Es gibt
  **kein** `longjmp` und kein Unwinding durch JIT-Frames (Release-Build hat `panic = "abort"`).

### 10.3 Asynchrone Interrupts

* `ritsu-platform` (GIC) setzt pro vCPU atomar `irq_pending/fiq_pending` und `exit_request`. Die vCPU prüft
  `exit_request` an jedem Blockanfang (§5.2) und im Dispatcher.
* Zustellung nur, wenn nicht maskiert (`PSTATE.I/F`) – geprüft im Dispatcher. Latenz: höchstens ein Block.
* SError (`PSTATE.A`) nur für gerätegemeldete asynchrone Fehler.

### 10.4 Generic Timer

* `CNTFRQ_EL0` = **24 MHz** (Vorschlag, wie Apple-Hardware; **A-10**, *Prüfweg:* Device Tree/Boot-Test, O-DBT-14).
* `CNTVCT_EL0`/`CNTPCT_EL0` (ohne EL2 ist der virtuelle Offset 0): Inline aus dem invarianten TSC,
  `((rdtsc − base) · mult) >> shift` (64×64→128-Multiplikation), `mult/shift` aus der TSC-Frequenz (CPUID-Leaf
  `0x15` oder Kalibrierung). Monotonie über vCPUs: TSC ist auf Alder Lake kernübergreifend synchron (**A-11**).
* `CNTV_CTL/CVAL/TVAL`, `CNTP_*`, `CNTKCTL_EL1` (EL0-Zugriff, Event-Stream): Registerzustand im DBT; das
  Erreichen der Deadline meldet der DBT an `ritsu-platform`, das die PPI (virtuell INTID 27, physisch 30) im GIC
  setzt.

### 10.5 WFI/WFE (E-16)

* **`WFI`:** vCPU-Thread parkt (Futex/Condvar), geweckt durch: Interrupt wird *pending* (auch wenn maskiert –
  architektonisch weckt jeder pending Interrupt), Timer-Deadline (Timeout des Wartens), Stop-/Flush-Request.
* **`WFE`:** Event-Register gesetzt → löschen, weiter. Sonst kurz spinnen (~1–2 µs, Monitor-Wert prüfen), dann
  parken mit Timeout. Geweckt durch `SEV` (Broadcast), `SEVL`, Interrupts, Event-Stream des Timers und das
  Löschen des globalen Monitors durch fremde Schreibzugriffe. Letzteres sehen wir nicht direkt (kein Hook auf
  normale Stores) → Polling des überwachten Werts beim Aufwachen und begrenzter Timeout (Start: 50 µs) →
  **O-DBT-17**. Typisches Muster: Spinlock mit `LDAXR` + `WFE`.
* `WFIT/WFET` (FEAT_WFxT) werden nicht gemeldet.

## 11. SIMD/FP und Krypto

### 11.1 Register

32 × 128-Bit-`V`-Register gegenüber 16 XMM/YMM (ohne AVX-512). `V`-Register liegen im `CpuState`; T1 cacht sie
blocklokal in XMM, T2 über Cranelift. Kein Codepfad darf AVX-512 voraussetzen (CLAUDE.md Regel 4); Ziel ist
**SSE4.2 als Basis, AVX2/FMA/BMI2/F16C optional per Laufzeiterkennung**.

### 11.2 FP-Semantik: Unterschiede Arm ↔ x86

| Thema | Arm | x86 SSE/AVX | Behandlung |
|---|---|---|---|
| Rundung | `FPCR.RMode` | `MXCSR.RC` | 1:1 abbildbar; MXCSR pro Block aus FPCR (in `TbFlags`) |
| Flush-to-zero | `FPCR.FZ` (Ein- **und** Ausgaben) | `FTZ` (Ausgaben) + `DAZ` (Eingaben) | beide setzen; Tininess-Erkennung vor/nach Rundung prüfen (O-DBT-8) |
| Default-NaN | `0x7FC00000` (positiv) | "QNaN indefinite" `0xFFC00000` (negativ) | Ergebnis-NaN prüfen (`vcmpunordps` + `vptest`), seltener Slow-Path korrigiert |
| NaN-Propagation (`FPCR.DN = 0`) | sNaN hat Vorrang vor qNaN, dann Operandenreihenfolge | bei zwei NaNs gewinnt der erste Quelloperand, ohne sNaN-Vorrang | gleicher Slow-Path |
| `FMAX/FMIN` | NaN, wenn ein Operand NaN; `max(−0, +0) = +0` | `maxps`: liefert bei NaN oder ±0 den **zweiten** Operanden | Sequenz mit Blend/Vergleich |
| `FMAXNM/FMINNM` | IEEE maxNum (Zahl gewinnt gegen qNaN) | kein Gegenstück | Sequenz |
| `FPCR.AH` (FEAT_AFP) | alternatives (x86-ähnliches!) Verhalten | – | nicht melden (§12) |
| Exception-Flags | `FPSR.{IOC,DZC,OFC,UFC,IXC,IDC}` kumulativ | `MXCSR.{IE,ZE,OE,UE,PE,DE}` | abbilden; `IDC` ≠ `DE` exakt (IDC nur bei FZ-Flush) |
| FP-Traps | `FPCR.{IOE,…}` (meist nicht implementiert) | maskierbar | Traps **nicht** melden (`FPCR`-Trap-Bits RAZ/WI) |
| Konvertierung → Int | sättigend, NaN → 0 | `cvttsd2si` liefert `0x8000…` ("integer indefinite") | Prüfsequenz |
| `FJCVTZS` (JSCVT) | JavaScript-Semantik (mod 2^32) | – | Sequenz/Helper |
| FP16 | volle Arithmetik (FEAT_FP16) | nur Konvertierung (F16C) | über FP32 rechnen: für `+ − × ÷ √` ist Doppelrundung harmlos (24 ≥ 2·11+2); **FMA** in FP16 nicht → über FP64-FMA mit Prüfung oder Software (O-DBT-8) |

Grundsatz: Der Fastpath rechnet mit Host-SSE und erkennt die seltenen Abweichungsfälle (NaN-Ergebnis, Denormal
mit FZ) billig; korrigiert wird im Slow-Path mit der T0-Software-FP. Niemals still falsch (Regel 5).

### 11.3 NEON → SSE4.2/AVX2 (Auswahl problematischer Fälle)

* `TBL/TBX` → `pshufb`; Indizes ≥ 16·n müssen 0 bzw. "unverändert" ergeben → Index mit `paddusb 0x70` sättigen
  (setzt Bit 7 für ≥ 16), mehrere Tabellenregister über Vergleich/Blend.
* 8-Bit-Shifts und -Multiplikationen gibt es auf x86 nicht → Sequenzen über 16-Bit-Lanes + Maske.
* `RBIT` (Vektor und skalar) → **GFNI** `gf2p8affineqb` (**A-12:** Alder Lake hat GFNI; Laufzeiterkennung,
  Fallback `pshufb`-Nibble-Tabelle).
* Sättigende, rundende, halbierende Ops (`SQADD`, `URHADD`, `SQRDMULH`, …) teils direkt (`paddsb`, `pavgb`),
  sonst Sequenzen; `QC`-Bit in FPSR muss gesetzt werden → Vergleich vorher/nachher.
* Widening/Narrowing (`*L`, `*L2`, `XTN`, `SQXTN`) → `pmovzx/pmovsx`, `packss/packus` mit Vorab-Maskierung.
* Horizontale/paarweise Ops (`ADDP`, `ADDV`, `UMAXV`) → Shuffles + `phminposuw` wo passend.
* `SDOT/UDOT` (Dotprod) → `pmaddubsw`/`pmaddwd`-Kombinationen; Vorzeichenkombination beachten.
* Lade-/Speicher-Strukturen `LD2/LD3/LD4/ST2…` → Laden + Shuffles; Fehler müssen pro Element präzise sein
  (Arm: Teilwirkung möglich, aber Fault-Adresse korrekt) → Seitengrenzen-Check vorab, sonst Slow-Path.

### 11.4 Krypto-Erweiterungen

| Arm | x86 | Anmerkung |
|---|---|---|
| `AESE` (= SubBytes∘ShiftRows∘AddRoundKey) | `aesenclast(s ⊕ k, 0)` | |
| `AESE`+`AESMC` (fusioniert) | `aesenc(s ⊕ k, 0)` | häufigstes Muster, wird erkannt |
| `AESMC` allein | `aesenc(aesdeclast(x, 0), 0)` | MixColumns isoliert (SubBytes/ShiftRows heben sich auf) |
| `AESD` / `AESIMC` | `aesdeclast(s ⊕ k, 0)` / `aesimc` | |
| `PMULL/PMULL2` (64×64→128) | `pclmulqdq` | `PMULL.8B` (8-Bit-polynomial) → Sequenz |
| `SHA1*`, `SHA256H/H2/SU0/SU1` | `sha1rnds4`, `sha1nexte`, `sha1msg1/2`, `sha256rnds2`, `sha256msg1/2` | **keine 1:1-Abbildung** (Arm: 4 Runden pro `SHA256H` mit anderer Operandenaufteilung; x86: 2 Runden) → Anpassungssequenzen, Fallback Software |
| `SHA512*` | – (Alder Lake hat keine SHA512-Erweiterung) | Software-Sequenz (AVX2) |
| `EOR3/BCAX/RAX1/XAR` (SHA3) | – (`vpternlog` ist AVX-512!) | 2–3 Befehle |
| `CRC32C*` (Castagnoli) | `crc32` | direkt |
| `CRC32*` (IEEE 802.3, Polynom `0x04C11DB7`) | – (x86 `crc32` ist Castagnoli!) | `pclmulqdq`-Faltung oder Tabelle |

**A-13:** Der i5-12500H unterstützt AES-NI, PCLMULQDQ, SHA-NI (SHA1/SHA256), VAES, GFNI. *Prüfweg:* CPUID zur
Laufzeit (AES-NI: Leaf 1 ECX[25]; PCLMULQDQ: Leaf 1 ECX[1]; SHA: Leaf 7 EBX[29]; VAES: Leaf 7 ECX[9];
GFNI: Leaf 7 ECX[8]). Jede Abbildung hat einen Fallback ohne die Erweiterung.

## 12. Gemeldeter CPU-Feature-Satz

### 12.1 Grundsatz und Spannungsfeld

ADR-0004 verlangt einen **minimalen** Feature-Satz: Jedes gemeldete Feature muss vollständig und getestet
unterstützt werden. **Aber:** macOS-Binärdateien werden für eine feste Apple-CPU-Baseline kompiliert
(Compiler-Ziel etwa `apple-m1`) und benutzen deren Befehle **unbedingt**, ohne die ID-Register zu fragen.
Die untere Schranke ist deshalb nicht "so wenig wie möglich", sondern "**die Baseline, für die Apple
kompiliert**". **A-14:** Die Baseline von macOS 27 ist Apple M1 (ARMv8.4-A plus Apple-Auswahl). Wenn macOS 27 M1
nicht mehr unterstützt, kommen mindestens BF16 und I8MM dazu. *Prüfweg:* ISA-Zensus (WP-DBT-19): T0 zählt alle
Undefined-Befehle beim Boot und im Userland und meldet sie gruppiert; dazu gehören auch Befehle nicht gemeldeter
Features, weil der Decoder diese als `Unknown` behandelt.

### 12.2 Vorschlag (O-DBT-2)

| Register.Feld | Wert | Feature | Begründung |
|---|---|---|---|
| `ID_AA64PFR0_EL1.EL0/EL1` | 1 / 1 | nur AArch64 | E-1 |
| `…EL2/EL3` | 0 / 0 | nicht vorhanden | Gast ohne EL2/EL3 |
| `…FP/AdvSIMD` | 1 / 1 | FP + NEON **mit FP16** | M1-Baseline |
| `…GIC` | 1 | GICv3-Systemregister | vmapple |
| `…SVE`, `ID_AA64PFR1_EL1.SME/MTE` | 0 | **kein SVE, SME, MTE** | Aufwand, Apple nutzt SVE/SME nicht in der M1-Baseline |
| `…DIT` | 1 | DIT | nur ein PSTATE-Bit |
| `…CSV2/CSV3` | offen | Spectre-Aussagen | O-DBT-20 |
| `ID_AA64PFR1_EL1.BT` | 0 | **kein BTI** | E-15 |
| `…SSBS` | 1 | SSBS | PSTATE-Bit, No-op |
| `ID_AA64ISAR0_EL1.AES` | 2 | AES + PMULL | M1 |
| `…SHA1/SHA2/SHA3` | 1 / 2 / 1 | SHA1, SHA256+SHA512, SHA3 | M1 |
| `…CRC32` | 1 | CRC32/CRC32C | |
| `…Atomic` | 2 | **LSE** | ABA-Entschärfung (§8.3), M1 |
| `…RDM`, `…DP`, `…FHM` | 1 / 1 / 1 | `SQRDMLAH`, Dotprod, FP16FML | M1 |
| `…TS` | 1 oder 2 | FlagM (/FlagM2) | M1 (zu prüfen) |
| `…TLB` | 0 oder 2 | TLBI-Range/Outer-Shareable | Kernel könnte es unbedingt nutzen → Zensus |
| `…RNDR` | 0 (Option 1) | `RNDR/RNDRRS` | leicht über `RDRAND/RDSEED`, aber nicht nötig |
| `…SM3/SM4/TME` | 0 | | |
| `ID_AA64ISAR1_EL1.API/GPI` | 1 / 1 | **PAuth, impdef-Algorithmus** | E-14 |
| `…APA/GPA` | 0 / 0 | kein QARMA | E-14 |
| `…JSCVT`, `…FCMA` | 1 / 1 | `FJCVTZS`, komplexe FP-Ops | v8.3, JavaScriptCore |
| `…LRCPC` | 2 | `LDAPR` + `LDAPUR/STLUR` | §8.2 (E-12) |
| `…FRINTTS`, `…SB`, `…DPB`, `…SPECRES` | 1 / 1 / 1–2 / 1 | | billig (No-ops oder einfache Ops) |
| `…BF16`, `…I8MM` | 0 | | M2+, abhängig von A-14 |
| `ID_AA64ISAR2_EL1` | 0 | kein MOPS, HBC, WFxT, CSSC, RPRES, QARMA3 | |
| `ID_AA64MMFR0_EL1.TGran16` / `TGran4/64` | 1 / 0xF | nur 16 KiB | E-2 |
| `…ASIDBits` | 2 | 16 Bit | |
| `…PARange` | 0b0010 (40 Bit) | 1 TiB phys. | genügt für vmapple-Speicherkarte (zu prüfen mit platform) |
| `…BigEnd/BigEndEL0` | 0 | nur LE | |
| `ID_AA64MMFR1_EL1.HAFDBS` | 0 | kein HW-AF/Dirty | A-5 |
| `…PAN` | 2 | PAN + `AT S1E1RP/WP` | v8.2 |
| `…VH`, `…VMIDBits` | 0 | kein EL2 | |
| `ID_AA64MMFR2_EL1.UAO` | 1 | UAO | PSTATE-Bit |
| `…AT` (LSE2) | 0 | vorerst nicht | A-7 / A-14 |
| `…E0PD` | 1 | E0PD | billige Zusatzprüfung im Walker |
| `CTR_EL0` | `DIC=0, IDC=1`, 64-Byte-Zeilen | | E-11; Zeilengröße O-DBT-15 |
| `DCZID_EL0.BS` | 4 (64 Byte) | `DC ZVA` | O-DBT-15 |
| `ID_AA64DFR0_EL1` | min. 2 Breakpoints, 2 Watchpoints, `PMUVer = 0` | | Architektur-Minimum; Watchpoints über TLB-`WATCH`-Flag |
| `MIDR_EL1` | offen | Hersteller/Teil | O-DBT-14: Apple-MIDR kann Apple-spezifische Kernel-Pfade auslösen |

Diese Tabelle ist die **einzige Quelle**: Decoder-Freischaltungen, ID-Register-Werte und Linux-`HWCAP` (für
den User-Mode-Harness) werden daraus generiert.

## 13. Multi-Core

### 13.1 Threads und Kerne (E-17)

* Ein Host-Thread pro vCPU. **Startwert: 4 vCPUs**, je eine auf einem P-Kern (erster Hyperthread). JIT-Threads
  (2–4) auf E-Kernen, Geräte-/GPU-Threads auf E-Kernen bzw. den zweiten Hyperthreads der P-Kerne.
* Kerntyp-Erkennung: `/sys/devices/cpu_core/cpus` und `/sys/devices/cpu_atom/cpus`, Fallback CPUID-Leaf `0x1A`
  (`0x40` = Core/P, `0x20` = Atom/E). Pinning per `sched_setaffinity`.
* Ob 8 vCPUs (beide Hyperthreads) mehr bringen → Messung (O-DBT-13).

### 13.2 Geteilter Code-Cache

* Ein `TranslationCache` für alle vCPUs (Speicher, einmal übersetzen), per-vCPU Jump-Cache und Return-Stack.
* Code-Region: ein zusammenhängender reservierter Bereich ≤ 2 GiB (für `rel32`-Sprünge). **W^X:** derselbe
  `memfd` zweimal gemappt (RW zum Schreiben/Patchen, RX zum Ausführen), nie RWX (Q-SEC-1).
* Freigabe über Epochen (§5.6).

### 13.3 TLB-Shootdown

* `TLBI …IS` (Inner Shareable, Broadcast): Die ausführende vCPU invalidiert lokal sofort und stellt allen
  anderen vCPUs eine **Flush-Anfrage** zu (lockfreie Queue pro vCPU, Überlauf → "flush all"), setzt deren
  `exit_request`.
* Die Anfrage wird vom Ziel am nächsten Blockanfang/Dispatcher bearbeitet. Geparkte (WFI) vCPUs bearbeiten sie
  beim Aufwachen; der Initiator darf deren TLB **nicht** selbst ändern (gehört dem Ziel-Thread).
* **`DSB ISH`** auf der Initiator-vCPU wartet, bis alle ausstehenden Anfragen bestätigt sind (geparkte vCPUs
  zählen als bestätigt, weil sie vor jeder weiteren Ausführung zuerst die Queue leeren).
* ★ macOS ruft TLBI-Broadcasts häufig auf (Prozesswechsel, `munmap`, Copy-on-Write). Kosten pro Shootdown
  (Schätzung): ein Round-Trip zwischen Threads, 1–5 µs. Optimierungen: Zusammenfassen mehrerer TLBIs bis zum
  `DSB`, Flush nur auf vCPUs, die die ASID zuletzt aktiv hatten.

### 13.4 Code-Invalidierung zwischen vCPUs und Patching

* Invalidierung (§5.6) entkettet global; per-vCPU Jump-Caches werden über dieselbe Request-Queue geleert. Eine
  vCPU kann einen alten Block noch zu Ende ausführen – erlaubt bis zu ihrem `ISB`/Kontextwechsel (§7.6).
* **Patchen von Sprüngen**, während andere Threads den Code ausführen: Wir patchen nur den 4-Byte-`rel32` eines
  `jmp`, der so ausgerichtet ist, dass er nicht über eine 8-Byte-Grenze läuft, mit einem atomaren Store über die
  RW-Abbildung. Andere Kerne sehen alten oder neuen Wert. Das ist gängige JIT-Praxis (z. B. HotSpot), formal
  verlangt Intels Cross-Modifying-Code-Protokoll aber Serialisierung → Risiko, eigener Stresstest (O-DBT-21).

## 14. Persistenter Übersetzungs-Cache (AOT)

### 14.1 Ziel

Der dyld shared cache (mehrere GB) und der Kernelcache sind pro macOS-Build unveränderlich. Ab dem zweiten Boot
soll deren heißer Code ohne JIT-Kosten laufen (Q-PERF-2).

### 14.2 Schlüssel (E-18)

ADR-0006 nennt (macOS-Build, physischer Seiten-Hash, DBT-Version, Host-Feature-Satz). Konkret:

```
CacheKey = H( content(16-KiB-Seite) ‖ TbFlags ‖ seitenrelative Einstiegs-Offsets )
Datei    = (DBT-Version, Host-Feature-Maske, Format-Version) im Header; macOS-Build nur als Partitionierung/GC-Hinweis
```

* **Inhalts-Hash statt Build-ID:** Der DBT sieht physische Seiten, keine Dateien. Der Inhalts-Hash ist
  unabhängig davon, wo die Seite physisch liegt, und automatisch richtig, wenn sich Code ändert.
* **Kryptografischer Hash ist Pflicht** (z. B. BLAKE3, 128 Bit+; neue Abhängigkeit → Begründung im WP): Ein
  bösartiges Gast-Programm könnte sonst eine Seite mit derselben Prüfsumme wie eine Kernel-Seite erzeugen und
  bekäme deren Übersetzung → Rechteausweitung *im Gast*. Kosten (Schätzung): ~2–5 µs pro 16 KiB, einmal pro
  Seite und Boot, nur für Code-Seiten.
* Übersetzungen im persistenten Cache überschreiten **nie** eine Seitengrenze (sonst bräuchte der Schlüssel
  mehrere Seiten). T2-Regionen über mehrere Seiten werden nicht persistiert oder pro Seite aufgeteilt.

### 14.3 Verschiebbarkeit

Der shared cache und der Kernel liegen pro Boot an zufälligen Adressen (ASLR-Slide). Übersetzter Code darf
deshalb **keine absoluten Gast-VAs** enthalten. Zwei Optionen (O-DBT-10):

1. **Gast-PC-relativer Code:** Der Dispatcher übergibt die Block-VA in einem Register bzw. im `CpuState`;
   `ADR/ADRP`, Rücksprungadressen (`BL`) und direkte Sprungziele werden relativ dazu berechnet. Vorteil:
   Code ist ohne Patchen ausführbar → Cache-Datei kann **direkt `PROT_EXEC` gemappt** werden (kein Kopieren).
   Kosten: ein zusätzliches `lea` pro VA-abhängigem Wert.
2. **Relokationen:** Code enthält absolute Werte plus Relokationsliste (`GuestVaAbs64` an Offset X); beim Laden
   kopieren und patchen.

Vorschlag: Option 1 für persistierbare Übersetzungen, Option 1 auch für T1 allgemein, falls die Messung keinen
nennenswerten Nachteil zeigt. Host-seitig ist der Code ohnehin positionsunabhängig: Helper werden über eine
Tabelle im `CpuState` aufgerufen (`call [r15 + helper_off]`), Chain-Sprünge starten unverkettet.

### 14.4 Invalidierung und Lebenszyklus

* Anderer Seiteninhalt → anderer Schlüssel (keine explizite Invalidierung nötig).
* DBT-Version oder Host-Feature-Maske ändern sich → ganze Datei ungültig.
* Laufzeit-Invalidierung (SMC, IC) betrifft nur die geladene Instanz, nicht die Datei.
* Garbage Collection: LRU nach "zuletzt benutzt in Boot Nr.", Größenlimit (Start: 4 GiB), getrennte Dateien
  pro macOS-Build erleichtern das Aufräumen nach Updates.
* Was persistiert wird: Seiten, deren Übersetzungen T1-Zähler > Schwelle erreicht haben und die bei Boot-Ende
  unverändert sind (Hash bei Schreiben neu prüfen).

### 14.5 Dateiformat (Skizze)

```
Header      magic "RITSUTC1", format_version, dbt_version_hash[32], host_features: u64,
            entry_count, index_off, blob_off, created_unix, flags
Index       offene Adressierung: { key: [u8; 16], blob_off: u64, blob_len: u32, crc32c: u32 }
Blob        pro Seite: { tbflags, n_entries, entries[] { page_off: u16, code_off: u32 },
                         n_exits, exits[] { code_off, kind, target_page_off },
                         code[] (ausgerichtet auf 64 Byte) }
```

* **Vertrauensmodell:** Der Cache ist **host-vertrauenswürdig**: nur `ritsu-vmm` schreibt ihn, er liegt auf einer
  Host-Partition, auf die der Gast keinen Zugriff hat. Der Gast beeinflusst nur den *Inhalt* der übersetzten
  Seiten, nicht die Zuordnung (dafür der kryptografische Schlüssel).
* **Integrität gegen Beschädigung:** CRC32C pro Blob (Hardware-`crc32`), Header-Prüfsumme; bei Fehler wird der
  Eintrag verworfen, nie ausgeführt. Schutz gegen Manipulation durch Host-Nutzer ist kein Ziel (wer die
  Host-Partition schreiben kann, kontrolliert ohnehin den Host). Option für später: HMAC mit Host-Schlüssel.
* Schreiben atomar (temporäre Datei + `rename`), Lesen per `mmap`.

## 15. Leistung, Budget, Profiling

### 15.1 Techniken (nach erwartetem Nutzen)

1. Block-Chaining und Jump-Cache (§5.2) – vermeidet Dispatcher-Rundläufe.
2. Inline-TLB-Fastpath (§7.3).
3. Return-Stack für `BL`/`RET` (häufigstes indirektes Sprungmuster).
4. Flags-Elimination, `CMP+B.cond`-Fusion (§6).
5. Konstanten: `ADRP+ADD/LDR`, `MOVZ/MOVK`-Ketten.
6. `LDP/STP` als Paar (zwei `mov` oder ein 16-Byte-Zugriff, wo atomar unkritisch).
7. PAC-Cache und -Redundanz (§9.4).
8. T2: Registerzuweisung über Blockgrenzen, Schleifen in Registern.
9. Persistenter Cache für Systemcode (§14).
10. Huge Pages für Gast-RAM und Code-Cache (Host-iTLB/dTLB).

### 15.2 Budget (Zielwerte, Schätzung)

| Größe | Ziel |
|---|---|
| T0 | ≥ 30 MIPS |
| T1 Übersetzung | < 1 µs/Block (Median), < 5 µs (p99) |
| T1 Codequalität | 3–6 Host-Befehle pro Gast-Befehl |
| T2 Übersetzung | 50–500 µs/Region auf E-Kern |
| T2 Codequalität | 1,5–2,5 Host-Befehle pro Gast-Befehl |
| Anteil Dispatcher-Zeit | < 5 % im eingeschwungenen Zustand |
| Gesamtziel | Q-PERF-1: ≥ 25 % (M6), ≥ 50 % (M8) der nativen Leistung pro Takt |

### 15.3 Profiling

* **perf map** (`/tmp/perf-<pid>.map`, Zeilen `START SIZE name`) für einfache Symbolisierung, und **jitdump**
  (`jit-<pid>.dump` mit `JIT_CODE_LOAD`-Records, Marker-`mmap`, danach `perf inject --jit`). Namen:
  `g:el1:ffff_fe00_0123_4560:T1`. Debug-Info-Records (`JIT_CODE_DEBUG_INFO`) ordnen Host-Adressen der Gast-PC
  zu → `perf annotate` zeigt Gast-Befehle.
* Eigene Zähler pro vCPU: Exits nach Grund, TLB-Hit-Rate pro mmu_idx, Jump-Cache-/Return-Stack-Treffer,
  Shootdowns, Invalidierungen, Tier-Verteilung der ausgeführten Befehle (Sampling), Kompilierzeiten.
* Ausgabe über eine Statistik-Schnittstelle an `ritsu-vmm` (später Overlay/Log).

## 16. Teststrategie

Verbindlich: Jede Befehlsklasse bekommt Differenztests T0 ↔ T1 ↔ T2 ↔ Referenz (CLAUDE.md §5).

### 16.1 Ebenen

| Ebene | Inhalt | Wo |
|---|---|---|
| Unit | Decoder (bekannte Wörter, Register-31-Regeln, Encoder-Roundtrip), Semantik-Primitive gegen Pseudocode-Orakel, TLB/Walker | `#[cfg(test)]` in den Crates |
| Operations-Konformanz | jede `Sem`-Operation in allen Backends, Rand- und Zufallswerte | `ritsu-dbt` |
| Differenz | Zufallssequenzen pro Befehlsklasse, Startzustand zufällig (Register, Flags, FPCR, Speicherfenster) | `tests/dbt-diff/` |
| Litmus | Speichermodell (`MP`, `SB`, `LB`, `IRIW`, `2+2W`, `+dmb`, `+lda/stl`) mit mehreren vCPUs; verbotene Ergebnisse dürfen nie auftreten, Häufigkeiten erlaubter werden protokolliert | `tests/litmus/` |
| Fuzzing | Decoder (kein Panic, Roundtrip), `step` (kein Panic, Invarianten), T0↔T1 auf Zufallsprogrammen | `fuzz/` |
| System | Linux-User-Programme, eigene Mini-Kernel (EL1, MMU, Exceptions, Timer), später XNU-Boot | `tests/` |

### 16.2 Referenz-Orakel

1. **Echte arm64-Hardware (primär):** GitHub-Runner `ubuntu-24.04-arm` (ARMv9.0-Kerne). Ein statisch gelinktes
   aarch64-Linux-Testprogramm (`ritsu-ref-runner`) lädt Testvektoren (Code + Startzustand), führt sie nativ in
   einem `mmap`-Puffer aus (`__clear_cache` nach dem Schreiben), sichert den Endzustand per Trampolin und schreibt
   **Golden Files**. Diese werden ins Repo eingecheckt (eigene, generierte Daten – keine Apple-Binärdateien,
   ADR-0008) und auf x86-CI ohne Arm-Hardware gegen T0/T1/T2 verglichen. Grenzen: PAC-Werte sind nicht
   vergleichbar (anderer Algorithmus) → PAC wird über Eigenschaften getestet (Sign/Auth-Roundtrip, Fehlerbits,
   Strip). Verfügbarkeit/Kosten für private Repos → O-DBT-11.
2. **QEMU (sekundär):** `qemu-aarch64` (User-Mode) lokal und in x86-CI für schnelle Rückmeldung. QEMU hat eigene
   Abweichungen; Rangfolge bei Widerspruch: Hardware > Arm-Pseudocode > QEMU.
3. **Arm-ASL-Spezifikation** als drittes Orakel, falls die Lizenz es erlaubt (O-DBT-11).

### 16.3 Linux-User-Mode als erster Meilenstein (M1)

Bevor XNU bootet, führt der DBT **statische aarch64-Linux-ELF-Programme im User-Mode** aus (EL0, Syscalls
emuliert, keine MMU-Emulation). Begründung:

* Testbar **ohne Apple-Binärdateien** (Clean Room, CI-tauglich, rechtlich unkritisch).
* Echte Compiler-Ausgabe (musl/glibc, `-O2`) übt die Basis-ISA viel breiter als handgeschriebene Tests.
* Dieselben Programme laufen nativ auf dem Arm-Runner → Ausgabe ist direkt das Orakel.
* Benchmarks (z. B. CoreMark-artige eigene Kerne) früh messbar (R-2: früh messen).

Umfang M1: ELF64-Loader (`ET_EXEC`, `PT_LOAD`), Startstack (argc/argv/envp/auxv mit `AT_PAGESZ = 16384`,
`AT_RANDOM`, `AT_HWCAP` aus §12), Syscalls `write`, `writev`, `exit`, `exit_group`, `brk`, `mmap`(anonym),
`munmap`, `set_tid_address`, `uname`, `getrandom`, `clock_gettime`, Stubs für `ioctl(TCGETS)` (→ `ENOTTY`) und
`rt_sigprocmask`. Speicher über eine dünn besetzte `GuestMemory`-Implementierung + `IdentityTranslate`.

### 16.4 Was hier nicht testbar ist

Boot eines echten macOS-Images (keine Apple-Binärdateien im Repo/CI): nur lokal beim Nutzer, Ergebnisse als
Bericht. ISA-Zensus (WP-DBT-19) ebenso.

## 17. Work Packages

Reihenfolge = empfohlene Abarbeitung. ★ = hohes Risiko (zuerst Spike). Alle WPs: Tests nach §16, Doku-Update
dieses Dokuments, `make check` grün.

| WP | Ziel | Abhängig | Risiko | Akzeptanz |
|---|---|---|---|---|
| **WP-M0-DBT** | Dieses Dokument + Skelett (Decoder-Teilmenge, T0, TLB, MMU-Trait) | – | | `cargo test/clippy/fmt` grün (erledigt, §19) |
| **WP-DBT-01** | Semantik-Schicht `Sem` + `Concrete`-Backend + IR-Typdefinitionen; M0-Befehle darauf umgestellt | M0 | ★ (prägt alles) | M0-Tests unverändert grün; Ops-Konformanztests für alle `Sem`-Ops; Review durch architect (umgesetzt, §3.2.1, §19; architect-Review offen) |
| **WP-DBT-02** | Integer-Basis-ISA in T0: DP-imm (inkl. `DecodeBitMasks`, Bitfield, Extract, `ADR/ADRP`), DP-reg (Shift/Extend, `ADC/SBC`, `CCMP`, `CSEL`-Familie, `MADD/SMULH/UDIV`…, `CLZ/RBIT/REV`), alle Loads/Stores inkl. `LDP/STP`, Pre/Post-Index, Literal; `SVC/BRK`; `MRS/MSR NZCV/TPIDR_EL0/FPCR` | 01 | | Decoder-Tests je Klasse, Encoder-Roundtrip, `DecodeBitMasks` exhaustiv, T0-Tests inkl. Division durch 0 und `INT_MIN/−1` |
| **WP-DBT-03** | Linux-User-Harness (ELF-Loader, Startstack, Syscalls §16.3) | 02 | | **Meilenstein M1: statisches aarch64-Linux-ELF "hello world" (musl und glibc) läuft im User-Mode über T0** und gibt korrekt aus; Exit-Code stimmt |
| **WP-DBT-04** | Differenztest-Infrastruktur: Generator, `ritsu-ref-runner`, Golden Files vom Arm-Runner, QEMU-Orakel | 02 (mit test-engineer) | | CI-Job erzeugt/prüft Golden Files; ≥ 10⁵ Vektoren Integer-ISA ohne Abweichung |
| **WP-DBT-05** | Decoder-Fuzzing + nächtlicher 2³²-Klassifikationsvergleich | 02 | | Fuzz-Targets in `fuzz/`; 0 Abweichungen in der Klassifikation für dekodierte Klassen |
| **WP-DBT-06** | SoftMMU: Stage-1-Walker (16K, TTBR0/1, TxSZ, TBI, ASID, AF, AP, UXN/PXN, WXN, PAN, E0PD), `AT`, TLBI (lokal), TLB-Integration in T0 | 02 | | Walker-Tests mit handgebauten Tabellen (alle Level, 32-MiB-Block, jeder Fault-Typ mit korrektem FSC/Level) |
| **WP-DBT-07** | Systemregister-Tabelle, Exceptions (sync/async), `ERET`, Vektoren, EL0↔EL1, Illegal Return | 06 | | Eigene Mini-Kernel-Tests (EL1 mit MMU, Syscall aus EL0, Page Fault mit Wiederaufnahme) |
| **WP-DBT-08** | Generic Timer, `WFI/WFE`-Parken, GICv3-CPU-Interface-Hooks | 07 (mit platform-engineer) | | Timer-Interrupt erreicht Mini-Kernel; WFI-Wake-Latenz gemessen |
| **WP-DBT-09** | Exklusive, LSE, Barrieren in T0 (mehrere vCPU-Threads über Host-Map-Schnittstelle, O-DBT-1) | 07, O-DBT-1 | ★ | Mehr-Thread-Zählertests, ABA-Test dokumentiert, Litmus-Basis in T0 |
| **WP-DBT-10** | FP/NEON in T0 (bit-exakte Software-FP, FPCR/FPSR, FP16) | 02 | ★ (Umfang, Semantik) | Golden Files FP/NEON inkl. NaN, Denormal, alle Rundungsmodi |
| **WP-DBT-11** | PAC (impdef-Algorithmus, alle PAC/AUT/XPAC-Formen, kombinierte Sprünge/Loads, `PACGA`) | 07 | ★ | Eigenschaftstests (Roundtrip, Fehlerbits nach Pseudocode, Lage der PAC-Bits für alle TBI/TxSZ-Kombinationen) |
| **WP-DBT-12** | T1-Basis-JIT: x86-Emitter, Code-Cache (W^X dual-mapped), Dispatcher, Chaining, Jump-Cache, Return-Stack, Inline-TLB, kalte Stubs, präzise Faults, perf map | 06, 07, 01 | ★ | Alle Differenztests T0↔T1 grün; Median < 1 µs/Block (Benchmark in `bench/`); M1-Programme auch über T1 |
| **WP-DBT-13** | Multi-vCPU: Threads, P-Kern-Pinning, geteilter Code-Cache mit Epochen, TLB-Shootdown, Cross-vCPU-Invalidierung, SMC/IC-Mechanismen | 12, 09 | ★ | Stresstests (Invalidierung während Ausführung, Shootdown-Last), keine Abweichung in 24-h-Lauf |
| **WP-DBT-14** | Atomics/Barrieren in T1 (CAS-Emulation, LSE-Abbildung, `LDAR`-Zaun), Litmus-Suite | 13 | ★ | Litmus: verbotene Ergebnisse 0 in 10⁸ Läufen pro Test |
| **WP-DBT-15** | NEON/FP in T1 (SSE4.2/AVX2 mit Laufzeitwahl) + Krypto | 10, 12 | | Differenztests T0↔T1 für alle NEON/FP/Krypto-Klassen, mit und ohne AVX2/SHA-NI |
| **WP-DBT-16** | Profiling (jitdump mit Debug-Info), Zähler, Bench-Integration | 12 | | `perf report` zeigt Gast-Symbole; Zähler im Bench-Output |
| **WP-DBT-17** | T2: Regionen, IR-Passes (§4.4), Cranelift-Backend, asynchrone Kompilierung auf E-Kernen | 12, 13 | ★ | Differenztests T0↔T2; Speedup gegenüber T1 auf Benchmarks gemessen |
| **WP-DBT-18** | Persistenter Übersetzungs-Cache (Format, Schlüssel, PIC-Code, GC, Integrität) | 12 (17 optional) | ★ | Zweiter Lauf eines großen Linux-Programms ohne T1-Übersetzung der gecachten Seiten; Korruptionstest (kaputter Blob wird verworfen) |
| **WP-DBT-19** | Spike: ISA-Zensus und Feature-Satz mit echtem vmapple-Kernelcache (beim Nutzer, nicht in CI) | 07, 11 | ★ (rechtlich/Test-Infrastruktur) | Bericht in `docs/research/`: benutzte, nicht gemeldete Features; A-1, A-2, A-4, A-9, A-14 bestätigt/widerlegt |
| **WP-DBT-20** | XNU-Bring-up mit platform bis zur ersten Konsolenausgabe | 19, 08, 13 | ★ | Kernel-Log auf PL011 |

## 18. Offene Fragen

| ID | Frage | Wer entscheidet |
|---|---|---|
| O-DBT-1 | `GuestMemory::write(&mut self)` erlaubt keine gleichzeitigen/atomaren Zugriffe mehrerer vCPUs. Neue, zusätzliche Schnittstelle (z. B. `HostRam` mit Host-Zeigern, `unsafe` im Modul `host_map`) in `ritsu-mem`? | architect, ADR |
| O-DBT-2 | Exakter Feature-Satz (§12): M1- oder M2-Baseline, LSE2, FPAC/PAuth2, FlagM2, TLB-Range, BF16/I8MM | WP-DBT-19, dann Mensch |
| O-DBT-3 | Nutzt der vmapple-Kernel Apple-proprietäre Erweiterungen (GXF, SPRR, AMX, Apple-Systemregister)? (A-2) | WP-DBT-19 |
| O-DBT-4 | PAC-Algorithmus: AES-NI mit 2 oder 4 Runden; ist die geringere Stärke für den Gast akzeptabel? | Mensch |
| O-DBT-5 | Ist die ABA-Abweichung der CAS-Emulation akzeptabel, oder braucht es einen strengeren Default? | Mensch nach Litmus-/Boot-Ergebnissen |
| O-DBT-6 | Inter-Block-ABI: Zustand im Speicher (E-7) vs. gepinnte heiße Register | Messung in WP-DBT-12 |
| O-DBT-7 | `LDAR`-Zaun am Load vs. `xchg` am `STLR` (E-12) | Messung + Litmus in WP-DBT-14 |
| O-DBT-8 | Software-FP für T0: eigene Implementierung oder Abhängigkeit (z. B. `rustc_apfloat`)? FP16-FMA exakt; Tininess-Erkennung FZ Arm vs. x86 | dbt-engineer + Orchestrator (Abhängigkeit) |
| O-DBT-9 | x86-Emitter für T1: eigener Mini-Assembler oder Crate (`iced-x86`, `dynasmrt`)? | Orchestrator (Abhängigkeit) |
| O-DBT-10 | Persistenter Cache: PC-relativer Code (zero-copy `mmap`) vs. Relokationen; Hash-Abhängigkeit (BLAKE3) | Messung in WP-DBT-18 |
| O-DBT-11 | Verfügbarkeit/Kosten von GitHub-Arm-Runnern für das (private?) Repo; Lizenz der Arm-ASL/XML-Spezifikation für Decoder-Generierung und Orakel | Mensch |
| O-DBT-12 | Wo lebt der Linux-User-Harness? Neues Crate/Tool (Root-`Cargo.toml` ist nicht im DBT-Scope) | Orchestrator/architect |
| O-DBT-13 | Anzahl vCPUs (4 auf P-Kernen vs. 8 mit SMT) | Messung |
| O-DBT-14 | `MIDR_EL1` (Apple vs. Arm-Hersteller-ID) und `CNTFRQ_EL0` (24 MHz) | WP-DBT-19 + platform |
| O-DBT-15 | Cache-Zeilengröße in `CTR_EL0` und `DCZID_EL0.BS` (64 vs. 128 Byte) | WP-DBT-19 |
| O-DBT-16 | Intel-Garantie für 16-Byte-Atomarität: welche Befehle genau (Voraussetzung für LSE2) | dbt-engineer (SDM) |
| O-DBT-17 | `WFE`-Weckstrategie (Polling-Timeout) und Auswirkung auf Spinlock-Latenz/Energie | Messung |
| O-DBT-18 | T1-Registerpool: welche Gast-Register profitieren am meisten (Statistik aus eigenen arm64-Programmen) | Messung |
| O-DBT-19 | Kann die Schreibverfolgung für Code-Seiten (E-11, Mechanismus 2) aus Leistungsgründen eingeschränkt werden? | Messung |
| O-DBT-20 | `CSV2/CSV3` melden (Spectre-Aussagen) oder nicht; Folgen für Kernel-Mitigations | Mensch/architect |
| O-DBT-21 | Atomares Patchen von `rel32` unter laufender Ausführung vs. Intels Cross-Modifying-Code-Protokoll | Stresstest WP-DBT-13 |
| O-DBT-22 | Härtung des JIT gegen gast-kontrollierte Immediates (JIT-Spraying, Q-SEC-1): Konstanten-Blinding nötig? | architect |

## 19. Stand des Skeletts (M0)

| Datei | Inhalt | Tests |
|---|---|---|
| `crates/ritsu-mem/src/mmu.rs` | `AccessKind`, `Privilege`, `Access`, `PagePerms`, `FaultKind` (mit `fsc()` nach `ESR_ELx`), `TranslationFault`, Trait `Translate`, `IdentityTranslate` (MMU aus, Address-Size-Fault oberhalb der PA-Größe) | Rechte-Matrix, FSC-Codes, Identität/Fault |
| `crates/ritsu-mem/src/tlb.rs` | Direkt abgebildeter Referenz-TLB für 16 KiB: `insert`, `lookup` (mit ASID/global/Rechten), `flush_all`, `flush_asid`, `flush_va`, `flush_va_all_asids`, Block-Fragment-Behandlung über ein konservatives VA-Intervall `[lo, hi]` (§7.1) | Treffer/Miss, Konflikte, TLBI-Semantik, Blöcke, Blöcke am Adressraumende |
| `crates/ritsu-dbt/src/cpu.rs` | `CpuState` (`#[repr(C)]`: `x[31]`, `sp_el0/el1`, `pc`, `Nzcv`, EL, `PSTATE.SP`, `DAIF`, `SysRegs`-Platzhalter, `PacKeys` mit geschwärztem `Debug`; alle eingebetteten Typen `#[repr(C)]`/`#[repr(u8)]`/`#[repr(transparent)]`), `XReg`, `GpReg` (SP/ZR aufgelöst) | SP-Auswahl, ZR, 32-Bit-Zero-Extension, JIT-ABI-Offsets per `offset_of!` |
| `crates/ritsu-dbt/src/decode.rs` | `decode(u32) -> Inst` für `MOVN/MOVZ/MOVK`, `ADD/ADDS/SUB/SUBS (imm)`, `B`, `BL`, `BR`, `BLR`, `RET`, `CBZ/CBNZ`, `B.cond`, `NOP`; `Cond` mit `holds()` (delegiert an `sem::condition_holds`) | bekannte Wörter, Register-31-Regeln, Unallocated, Offsets an den Grenzen, Encoder-Roundtrip auf 140 000 Zufallswörtern |
| `crates/ritsu-dbt/src/sem/` | Trait `Sem`, `Ty`, `BinOp`, `CmpOp`, `BranchKind`, `Exception` (`mod.rs`); Beschreibungen der M0-Befehle + `lower` (`a64.rs`); Pseudocode-Primitive (`prim.rs`); T0-Backend `Concrete` (`concrete.rs`) (WP-DBT-01) | Konformanz jeder Operation gegen unabhängige Orakel über alle Typen (`conformance.rs`), `add_with_carry64/32` gegen 128-Bit-Orakel, Debug-Prüfungen für Beschreibungsfehler |
| `crates/ritsu-dbt/src/ir/` | IR-Typen (`Region`, `Block`, `Value`, `Type`, `Op`, `Terminator`, `Exit`), `Region::verify`, `Builder` (IR-Backend von `Sem`) (WP-DBT-01) | M0-Befehle: IR-Replay = T0 (Zustand und Fehler) auf Zufallszuständen, ganzer Schleifenblock, Form bedingter Sprünge, relative PCs, Verifier-Negativtests |
| `crates/ritsu-dbt/src/interp.rs` | T0: `step`, `step_with`, `fetch`, `execute` (seit WP-DBT-01 nur Fetch + Dispatch an `sem::lower` auf `Concrete`), `ExecError` (Undefined, PC-Alignment, Instruction Abort, Bus-Fehler), `StepOutcome` | Countdown-Schleife, Call/Return, `BLR X30`, CBZ 32/64, Flags 32/64, alle 16 Bedingungen × 16 Flag-Kombinationen, präziser Zustand bei Fehlern |
| `crates/ritsu-dbt/src/tier.rs` | Schnittstellen `Tier`, `TbFlags`, `BlockKey`, `BlockId`, `TranslateError`, Traits `CodeSource`, `Translator`, `TranslationCache` (ohne Implementierung) | – |
| `crates/ritsu-dbt/src/asm.rs` | Test-Encoder (nur `#[cfg(test)]`), gegen bekannte Wörter geprüft | – |

Bewusst ausgelassen: `BranchAddr`/TBI bei Sprüngen (WP-DBT-06, zentral in `Sem::branch_indirect` der Backends),
Speicher-/Systemregister-Operationen in `Sem` (WP-DBT-02/-07), 128-Bit-/Vektorwerte (WP-DBT-10), T1-`Template`-
Backend (WP-DBT-12), IR-Codegen und -Passes (WP-DBT-17), alle weiteren Befehlsklassen, Hints außer `NOP`.
