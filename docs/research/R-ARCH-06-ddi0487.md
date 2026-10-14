# R-ARCH-06: Normative Prüfung offener Architekturregeln gegen Arm DDI 0487 (WP-ARCH-06)

> Status: **Prüfbericht** (2026-10-14). Autor-Rolle: `architect`. Anlass: [ADR-0010](../adr/0010-dbt-platform-boundary.md)
> und [R-DBT-01](R-DBT-01-sem-review.md) stützen fünf Architekturregeln bisher nur auf Sekundärquellen (QEMU-Commit,
> LKML, Register-Spiegel) oder markieren sie als **Annahme**. Dieser Bericht prüft sie gegen das Arm Architecture
> Reference Manual. Er ändert keinen Code. Die Korrekturen an ADR-0010 sind dort mit Änderungsvermerk eingearbeitet.

## 1. Quelle und Methode

* **Primärquelle:** *Arm Architecture Reference Manual for A-profile architecture*, ARM DDI 0487, **Ausgabe M.d**
  (Dokumentqualität EAC, veröffentlicht 2026-09-29, laut Versionsliste die neueste öffentliche Ausgabe).
  * PDF (autoritativ): `DDI0487M_d_a-profile_architecture_reference_manual.pdf`, abgerufen über
    <https://developer.arm.com/documentation/ddi0487/latest/> (Dokumentations-API
    <https://documentation-service.arm.com/documentation/ddi0487/md>, Ressource „DDI0487M_d…pdf“).
  * HTML-Fassung derselben Ausgabe (laut Arm „early access preview“, bei Abweichungen gilt das PDF):
    <https://developer.arm.com/documentation/ddi0487/md/>.
* **Vorgehen:** Die Abschnitte wurden aus der HTML-Fassung gelesen. Jede unten zitierte Regel-ID und jede
  Kernaussage wurde anschließend im Text des PDF wiedergefunden. Die Seitenangaben (z. B. „D1-7214“) sind die
  Seitenzahlen des PDF M.d. Abweichungen zwischen HTML und PDF gab es bei den zitierten Stellen keine.
* **Notation:** Arm druckt Regel-IDs als R/I mit tiefgestellter Kennung, hier geschrieben als `R_TNVSL` (Regel,
  normativ) bzw. `I_MFLXH` (Information, nicht normativ). Abschnitte ohne IDs (Teil B2) werden mit Abschnittsnummer,
  Seite und wörtlichem Satz zitiert. Pseudocode steht in Kapitel J1 und ist normativ.
* **Sekundärquellen** wurden nicht gebraucht. Die bisher genannten (QEMU-Commit, LKML-Beitrag, Register-Spiegel
  `df.lth.se`) stimmen in der Sache mit der Norm überein, werden aber durch die Zitate unten ersetzt.

### 1.1 PE-Modell, gegen das geprüft wird

Die Regeln hängen von den implementierten Exception Levels und Features ab. Geprüft wird gegen das Gast-Modell aus
[dbt.md](../subsystems/dbt.md) §2 (E-1) und §12:

| Eigenschaft | Wert im Gast-Modell | Wirkung auf die Prüfung |
|---|---|---|
| Exception Levels | EL0, EL1, nur AArch64. **Kein EL2, kein EL3**, kein Secure State | `HaveEL(EL2) = HaveEL(EL3) = FALSE`, `EL2Enabled() = FALSE` (J1.4.547, J1-16670) |
| FEAT_NMI | nicht gemeldet | `AllIntMask` ist effektiv 0 (`R_SFMGZ`, D1-7247), keine Superpriority |
| FEAT_LS64 | nicht gemeldet | kein ISV-Sonderfall für `LD64B`/`ST64B` |
| FEAT_LSE2 | vorerst nicht gemeldet (dbt.md §8, A-7/A-14) | `STP` ist immer zwei Zugriffe (siehe Regel 4) |
| FEAT_RAS | **offen** (Feature-Baseline, WP-ARCH-03) | betrifft nur `ESR_EL1.SET`, siehe Regel 5 |
| FEAT_WFxT | nicht gemeldet (dbt.md §10.5) | kein `WFIT`/`WFET` |

## 2. Ergebnis in Kürze

| Nr. | Regel (Stand ADR-0010 / R-DBT-01) | Urteil | Folge |
|---|---|---|---|
| 1 | Ohne EL2 ist `HVC`, ohne EL3 `SMC` UNDEFINED | **bestätigt** | Quelle in ADR-0010 ersetzt. Der `Hypercall`-Exit ist als bewusste Modellabweichung benannt. |
| 2 | `WFI` wacht bei per `PSTATE.{I,F}` maskierten Interrupts auf, `WFE` nicht | **bestätigt**; ADR-0010 §7.2 aber **in zwei Details falsch** | §7.2 korrigiert: vollständige `WFE`-Weckliste (u. a. Löschen des globalen Monitors), Trap-Reihenfolge bei `WFI`/`WFE` aus EL0 |
| 3 | Device-Zugriffe dürfen nach einem Abort wiederholt werden | **bestätigt, mit Einschränkung**: nur bei Befehlen, die mehrere single-copy-atomare Zugriffe erzeugen | Annahme in ADR-0010 §5.2 durch Fakt ersetzt, Einschränkung als Pflicht für den DBT ergänzt |
| 4 | Teilweise Speicheraktualisierung bei Fault in einem Mehrfach-Store | **erlaubt**, für Normal und Device gleichermaßen | Die SError-Ausweichregel in ADR-0010 §5.2 ist **unnötig und schlechter** → durch synchronen Data Abort ersetzt. `probe` (R-DBT-01 P1-2) bleibt zulässig, ist aber nicht nötig. |
| 5 | `ESR_EL1.ISV` ist ohne Stage 2 immer 0 | **bestätigt** | Quelle in ADR-0010 ersetzt. Nebenbefund: `IL` muss dann 1 sein. |

## 3. Die Regeln im Einzelnen

### 3.1 `HVC`/`SMC` ohne EL2/EL3

**Zitate (DDI 0487 M.d)**

* **C6.2.175 HVC** (C6-2220): „The HVC instruction is UNDEFINED: When EL3 is implemented and SCR_EL3.HCE is set to 0.
  When EL3 is not implemented and HCR_EL2.HCD is set to 1. When EL2 is not implemented. At EL1 if EL2 is not enabled
  in the current Security state. At EL0.“ Der Decode-Pseudocode macht den Befehl ohne EL2 schon beim Dekodieren
  undefiniert: `if !HaveEL(EL2) then EndOfDecode(Decode_UNDEF); end;`.
* **D1.4.9, `R_ZVTPR`** (D1-7258): „If any of the following is true, the HVC instruction is UNDEFINED: The PE is
  executing at EL0. If EL2 is not enabled in the current Security state, and the PE is executing at EL1. The
  Effective value of SCR_EL3.HCE is 0. EL2 is not implemented.“
* **D1.4.9, `R_HMXQS`** (D1-7258): „If any of the following are true, the SMC instruction is UNDEFINED: The PE is
  executing at EL0. EL3 is not implemented and the SMC instruction is not trapped to EL2 by HCR_EL2.TSC. SCR_EL3.SMD
  is 1 and the SMC instruction is not trapped to EL2 by HCR_EL2.TSC.“
* **J1.2.179 `AArch64_CheckForSMCUndefOrTrap`** (J1-16005): Ohne EL3 wird `SMC` nur dann nach EL2 umgeleitet, wenn
  `PSTATE.EL == EL1 && EL2Enabled() && HCR_EL2.TSC == '1'` gilt (plus NV- oder IMPDEF-Bedingung). Sonst
  `Undefined()`.
* **C6.2.372 SMC** (C6-2642): „If the value of HCR_EL2.TSC is 0 and the value of SCR_EL3.SMD is 1, the SMC
  instruction is UNDEFINED.“

**Die Steuerbits und ihre Gegenstücke ohne EL2/EL3**

| Steuerbit | Existiert nur, wenn | Wirkung | Im Ritsu-Gast |
|---|---|---|---|
| `HCR_EL2.HCD` | EL2 implementiert **und** EL3 nicht | `HVC` UNDEFINED | Register existiert nicht. Die Bedingung „EL2 is not implemented“ greift vorher. |
| `SCR_EL3.HCE` | EL3 implementiert | `HVC` UNDEFINED bei 0 | entfällt |
| `SCR_EL3.SMD` | EL3 implementiert | `SMC` UNDEFINED bei 1, außer bei TSC-Trap | entfällt. Gegenstück ist „EL3 is not implemented“ in `R_HMXQS`. |
| `HCR_EL2.TSC` | EL2 implementiert | leitet `SMC` aus EL1 nach EL2 um | entfällt (`EL2Enabled()` = FALSE) |

**Ergebnis für unser PE-Modell:** `HVC` und `SMC` sind auf EL0 **und** EL1 bedingungslos UNDEFINED. Die Exception
ist eine Undefined Instruction Exception nach EL1 (`R_SXYZB`, D1.4.7, D1-7255), Syndrom `ESR_EL1.EC = 0x00`, `IL = 1`,
`ISS` RES0 (D24.2.44, „ISS encoding for exceptions with an unknown reason“). `ELR_EL1` ist die Adresse des
Befehls (`R_QYCWH`, D1-7216: Für synchrone Exceptions außer *ausgeführten* Exception-erzeugenden Befehlen ist die
bevorzugte Rücksprungadresse der Befehl selbst. `R_DKWPP` mit `PC+4` gilt nur, wenn `HVC`/`SMC` tatsächlich
ausgeführt wird). Priorität 20 in `R_ZFGJP` (D1-7230) führt „HVC … disabled by SCR_EL3.HCE or HCR_EL2.HCD“
ausdrücklich als UNDEFINED.

**Urteil: bestätigt.**

**Konsequenz für ADR-0010.** Die Grenze ist richtig gebaut: `complete_hypercall(Undefined)` erzeugt genau das
Architekturergebnis (EC 0x00, `ELR` = Befehl). `Handled` ist dagegen eine **bewusste Modellabweichung**. Der Gast
sieht `ID_AA64PFR0_EL1.EL2 = EL3 = 0` und trotzdem einen behandelten `HVC`. Das entspricht einer unsichtbaren
Firmware-Ebene, wie sie QEMU für PSCI nutzt. Diese Abweichung ist jetzt in ADR-0010 als solche benannt. Daraus
folgt: Die Plattform darf `Handled` nur für Funktions-IDs auf dem Conduit melden, den die Gast-Firmware-Beschreibung
(Device-Tree/ACPI-Äquivalent) ankündigt. Alles andere bekommt `Undefined`. Für R-DBT-01 P1-3 heißt das:
`Hvc{imm}`/`Smc{imm}` mit Rücksprung `PC+4` beschreiben nur den `Handled`-Fall. Der architekturreine Pfad ist
`Undefined` mit eigenem `PC`.

### 3.2 Weck-Bedingungen von `WFI` und `WFE` bei maskierten Interrupts

**Zitate (DDI 0487 M.d)**

* **D1.7.2.1, `R_VRLPB`** (D1-7269), `WFI`: „All of the following are WFI wakeup events: Any physical SError
  exception, physical IRQ interrupt, or physical FIQ interrupt received by the PE that is marked as A or B in the
  tables in Asynchronous exception masking, **regardless of the value of the corresponding PSTATE.{A, I, F} mask
  bit**.“
* **D1.7.2.1, `R_NLVGY`** (D1-7269): „WFI wakeup events are never disabled by EDSCR.INTdis, and are never masked by
  the PSTATE.{A, I, F} mask bits, or by the state of AllIntMask. If wakeup is invoked by an interrupt that is
  disabled or masked, the interrupt is not taken.“
* **D1.7.1.4, `R_KBMFC`** (D1-7267/7268), `WFE`: Ein physischer IRQ/FIQ/SError ist ein WFE-Weckereignis, wenn er
  „marked as A … regardless of the value of the corresponding PSTATE.{A, I, F} mask bit“ ist oder „marked as B …
  and … **The value of the corresponding PSTATE.{A, I, F} mask bit is 0**“ (oder Superpriority bei
  `AllIntMask = 0`). Weitere Weckereignisse: `SEV` eines beliebigen PE, der Timer-Event-Stream, „An event caused by
  the clearing of the global monitor for the PE“, IMPDEF-Ereignisse, Debug-Requests.
* **Tabelle D1-22 / `R_SXLWJ`** (D1-7249): Ohne EL2 und EL3 (alle Steuerbits mit Effektivwert 0) sind IRQ und FIQ
  beim Ausführen in EL0 und EL1 **„B“**: „it might be subject to masking … If the interrupt is masked, it is not
  taken.“ „A“ (unmaskierbar) tritt nur bei Zielen in einem höheren EL auf (`R_XBYXL`, D1-7248), also nie in unserem Modell.
* **D1.7.1.1, `R_XRZRK`** (D1-7266): Das Event-Register wird gesetzt durch `SEV` (beliebiger PE), `SEVL` (eigener
  PE), „An exception return“, „The clearing of the global monitor for the PE“, den Event-Stream und
  IMPDEF-Mechanismen.
* **B2.12.2.1.1 „Clear global monitor event“** (B2-339): „Whenever the global monitor state for a PE changes from
  Exclusive access to Open access, an event is generated and held in the Event register for that PE.“
  **B2.12.6** (B2-344): Das ist gleichwertig zu einem `SEVL` auf diesem PE und „removes the need for spinlock code
  to include an SEV instruction after clearing a spinlock.“
* **`I_MFLXH`** (D1-7269) und **`I_QGHTM`** (D1-7267): Der PE darf den Schlafzustand „for any reason“ verlassen
  und `WFI`/`WFE` als NOP behandeln. `R_MRVLC`: Beim Verlassen ist `WFI` abgeschlossen.
* **Pseudocode** `Hint_WFI` (J1-16698): `if AArch64_InterruptPending() then EndOfInstruction(…Retired)` steht
  **vor** `AArch64_CheckForWFxTrap`. `AArch64_InterruptPending` (J1-16199) fragt `IRQPending()`/`FIQPending()`/
  `IsPhysicalSErrorPending()` ohne Blick auf `PSTATE` ab. `Hint_WFE` (J1-16696) prüft ebenso zuerst
  `IsEventRegisterSet()` (löschen, fertig) und erst danach den Trap.

**Urteil: Kernregel bestätigt.** `WFI` wacht bei jedem anstehenden IRQ/FIQ/SError unabhängig von `PSTATE.{A,I,F}`
auf. `WFE` wacht bei Interrupts nur, wenn das zugehörige Maskenbit 0 ist. Für SError gilt dasselbe mit `PSTATE.A`.

**Zwei Abweichungen in ADR-0010 §7.2 (korrigiert):**

1. *`WFE`-Weckliste.* ADR-0010 sagte „Geweckt wird **nur** durch nicht maskierte Interrupts, `SEV` anderer vCPUs,
   den Event-Stream … und Anforderungen“. Es fehlen das **Löschen des globalen Monitors** (ein fremder
   Schreibzugriff auf den per `LDXR` markierten Block) sowie die Quellen des Event-Registers **Exception-Rückkehr**
   und `SEVL`. Das erste ist praktisch wichtig: Nach B2.12.6 darf Spinlock-Code das Freigeben **ohne** `SEV`
   schreiben, und der wartende `LDXR`+`WFE`-Pfad verlässt sich auf genau dieses Ereignis. Eine Emulation, die nur auf
   `SEV` wartet, schläft dann bis zum Timeout. dbt.md §10.5 hat diesen Punkt bereits (Polling des überwachten Werts,
   begrenzter Timeout, O-DBT-17), ADR-0010 widersprach ihm.
2. *Trap-Reihenfolge.* ADR-0010 sagte „EL0 mit `nTWI = 0` → Trap EC 0x01“. Nach `Hint_WFI` ist ein `WFI` bei
   anstehendem Interrupt (Pegel, unabhängig von `PSTATE`) ein NOP, **auch** aus EL0 mit `nTWI = 0`. Getrappt wird
   nur ohne anstehenden Interrupt. Analog trappt `WFE` aus EL0 mit `nTWE = 0` nicht, wenn das Event-Register gesetzt
   ist (es wird gelöscht, der Befehl endet).

### 3.3 Wiederholung von Device-Zugriffen nach einem Abort

**Zitate (DDI 0487 M.d)**

* **B2.10.2 Device memory** (B2-324): „Where a load or store instruction performs a sequence of memory accesses, as
  opposed to one single-copy atomic access as defined in the rules for single-copy atomicity, these accesses might
  occur multiple times as a result of executing the load or store instruction.“
* **B2.10.2, Note** (B2-324): „An instruction that generates a sequence of accesses … might be abandoned as a result
  of an exception being taken during the sequence of accesses. On return from the exception, the instruction is
  restarted, and therefore, one or more of the memory locations might be accessed multiple times. … For this reason,
  Arm strongly recommends that no accesses to Device memory are performed from a single instruction that spans the
  boundary of a translation granule or which in some other way could lead to some of the accesses being aborted.“
* **B2.10.2.4 Multi-register loads and stores that access Device memory** (B2-328): „The architecture permits that
  the non-speculative execution of an instruction that loads or stores more than one general-purpose or SIMD and
  floating-point register might result in repeated accesses to the same address, even if the resulting accesses are
  to any type of Device memory.“
* **B2.2.1** (B2-251): „If, according to these rules, an instruction is executed as a sequence of accesses,
  exceptions, including interrupts, can be taken during that sequence, regardless of the memory type being accessed.
  If any of these exceptions are returned from using their preferred return address, the instruction that generated
  the sequence of accesses is re-executed, and so any access performed before the exception was taken is repeated.“
* **Grenzen, B2.10.2** (B2-324): „Speculative data accesses are not permitted to any memory location with any Device
  memory attribute.“ **B2.10.2.1 Gathering** (B2-325, nG): „The number of memory accesses that are made corresponds to the
  number that would be generated by a simple sequential execution of the program.“

**Urteil: bestätigt, mit Einschränkung.** Wiederholung ist erlaubt, wenn der Befehl eine *Folge* von Zugriffen
erzeugt: `LDP`/`STP`, `LD1`–`LD4`/`ST1`–`ST4` mit mehreren Registern oder Elementen sowie unausgerichtete Zugriffe,
die nach B2.2.1 in Bytezugriffe zerfallen. Ein **einzelner** single-copy-atomarer Device-Zugriff darf dagegen weder
wiederholt noch spekulativ ausgeführt werden.

**Konsequenz für ADR-0010 §5.2 (eingearbeitet):** Die Annahme wird zum Fakt, die Einschränkung zur Pflicht. Der DBT
ruft für einen einzelnen Zugriff `mmio_read`/`mmio_write` **höchstens einmal** auf. Kein Retry eines Hooks, kein
Wiederabspielen nach einem kalten Stub, und eine Schreibbarkeitsprüfung (`probe`) darf auf MMIO **nie** lesen.
Hinweis an `dbt-engineer`: Der aktuelle Arbeitsstand `crates/ritsu-dbt/src/mem.rs` prüft Schreibbarkeit per Lesen
(`probe_by_read`). Die Moduldoku sagt selbst, dass das für MMIO falsch wäre. Die seiteneffektfreie Abfrage liefert
der Nachtrag zu ADR-0009 (§9, `region_attrs`).

### 3.4 Teilweise Speicheraktualisierung bei Fault in einem Mehrfach-Store

**Zitate (DDI 0487 M.d)**

* **D1.4.1.4, `R_TNVSL`** (D1-7214), Definition der präzisen Exception: „However, for an instruction executing
  immediately at the point in the instruction stream that the exception was taken from, the definition of precise
  also permits any of the following: For synchronous Data Abort and Watchpoint exceptions that are taken to AArch64
  state generated by an instruction that performs more than one single-copy atomic memory access, **the values in
  registers or memory affected by the instruction can be UNKNOWN**, if all of the following apply: The accesses
  affecting those registers or memory locations do not, themselves, generate exceptions or debug events. The
  registers are not involved in the calculation of the memory address used by the instruction.“ Für Loads: Ein
  geladenes Basis- oder Offsetregister wird auf den alten Wert zurückgesetzt, andere Zielregister werden UNKNOWN.
* **B2.2.1** (B2-250): „Writes that are generated by a Store pair instruction that stores two general-purpose
  registers and are aligned to the size of the store of each register are treated as two single-copy atomic
  writes, one for each register being stored.“ SIMD-Element- und Strukturzugriffe sind je Element single-copy-atomar.
  Ein 128-Bit-Wert zählt als zwei 64-Bit-Zugriffe. **Ausnahme mit FEAT_LSE2** (B2-251): Ein 16-Byte-ausgerichtetes
  `STP` zweier 64-Bit-Register auf Normal-WB-Speicher ist **ein** single-copy-atomarer Zugriff.
* **D7.6 External aborts** (D7-7674): „External aborts on data accesses … can be either synchronous or
  asynchronous.“ und „The effect of a failed memory access is described in Definition of a precise exception and
  imprecise exception.“ Ferner: „An imprecise asynchronous External abort is likely to be fatal to the process that
  is running, Arm recommends that implementations make External aborts precise wherever possible.“
* **`R_ZFGJP`, Prioritäten 45–49** (D1-7236): „when an instruction results in more than one single-copy atomic
  memory access, the architecture will not prioritize the individual Synchronous exceptions generated as the result
  of the multiple memory accesses.“

**Urteil: Teilaktualisierung ist erlaubt, für Normal- und Device-Speicher gleichermaßen.** `R_TNVSL` unterscheidet
nicht nach Speichertyp, und B2.10.2 (Note) behandelt den Device-Fall ausdrücklich. Es gelten nur zwei Bedingungen:

* Die Speicherstelle, deren Zugriff den Fault auslöst, bleibt unverändert. Alle anderen Ziele des Befehls dürfen
  geschrieben, ungeschrieben oder (architektonisch) UNKNOWN sein. In unserer Emulation enthalten sie entweder den
  alten Wert oder die Daten des Befehls selbst.
* Register, die in die Adressberechnung eingehen (Basis, Offset, Writeback), sind nach dem Fault unverändert.

Welcher Teilzugriff den Fault meldet, ist nicht festgelegt (`R_ZFGJP`). Die Architekturreihenfolge ist also eine
zulässige Wahl, aber keine Pflicht.

**Konsequenz für ADR-0010 §5.2 (eingearbeitet).** Die Regel „Scheitert ein Teil-Store nach einem bereits
ausgeführten MMIO-Store, meldet der DBT einen **SError** statt eines synchronen Aborts“ beruhte auf der vorsichtigen
Annahme, eine Teilaktualisierung sei verboten. Diese Annahme ist widerlegt. Ein SError wäre nach D7.6 zwar zulässig,
ist aber schlechter: Er ist für den Gast-Prozess wahrscheinlich fatal, und Arm empfiehlt präzise Aborts. Neu gilt:
**synchroner Data Abort** (Syndrom wie §5.3, `FAR_EL1` = VA des fehlschlagenden Teilzugriffs). Bereits ausgeführte
Teil-Stores (RAM oder MMIO) bleiben bestehen. Nach der Rückkehr wiederholt der Gast den Befehl, und die
Wiederholung des MMIO-Teils ist nach Regel 3 erlaubt.

**Konsequenz für R-DBT-01 P1-2 / dbt.md §3.2.1 (Vorschlag, nicht eingearbeitet, Pflege `dbt-engineer`).** Die
`probe`-Regel bleibt korrekt, weil „kein Teil geschrieben“ eine der erlaubten Lesarten ist. Sie ist aber **nicht
nötig**. Die Reihenfolge-Regel kann gelockert werden:

> Speicher-Writes eines Befehls dürfen vor einem späteren, fehlschlagenden Speicherzugriff desselben Befehls liegen.
> Register-Writes (Zielregister, Writeback, `NZCV`) folgen erst nach dem letzten Speicherzugriff.

Vorteile: kein zweiter Adressübersetzungs-Durchlauf für `STP`/`ST1`–`ST4` und keine seiteneffektfreie Prüfung auf
MMIO. Seitenübergreifende unausgerichtete Stores lassen sich einfach aufteilen. Ob `probe` bleibt (einfacheres
Testorakel, „alles oder nichts“) oder entfällt (weniger Ops in T1), entscheidet `dbt-engineer` per Messung in
WP-DBT-12. Beide Varianten sind architekturkonform. Für die Differenztests gegen echte Hardware oder eine Referenz
folgt daraus: Nach einem Data Abort in einem Mehrfach-Store darf der Vergleich die nicht fehlschlagenden Ziele des
Befehls **nicht** prüfen, weil die Architektur sie offenlässt.

### 3.5 `ESR_EL1.ISV` für Data Aborts ohne Stage 2

**Zitate (DDI 0487 M.d)**

* **D24.2.44 ESR_EL1, „ISS encoding for an exception from a Data Abort“, ISV bit [24]** (D24-8676): „The ISV field
  is 1 if all of the following apply: The fault was not a stage 2 fault on a stage 1 translation table walk. If
  FEAT_RAS is implemented, the fault was not any kind of synchronous External abort. Any of the following apply: The
  fault was generated by one of the described AArch64 load or store instructions that transfer a single
  general-purpose register, and … The fault was a stage 2 fault. … A Translation fault, Access flag fault, or
  Permission fault was generated by … LD64B or ST64B. ST64BV. ST64BV0. …“ und „Otherwise the value of this field
  is 0.“ Die Note nennt ausdrücklich ISV = 0 für LSE-Atomics, MOPS, MTE-Tag-Zugriffe und GCS.
* **J1.2.197 `AArch64_InstructionSyndromeValid`** (J1-16023): `TRUE` nur für `ls64` (bei Translation/AF/Permission)
  oder `IsSecondStage(fault)`, sonst `FALSE`. Mit FEAT_RAS immer `FALSE` bei synchronem External Abort.
* **ESR_EL1.IL, bit [25]**: `IL = 1` gilt auch für „A Data Abort exception for which the value of the ISV bit is 0“
  und für „An exception reported using EC value 0b000000“.

**Urteil: bestätigt.** Ohne EL2 gibt es keine Stage 2, und ohne FEAT_LS64 entfällt der zweite Fall. `ISV` ist für
jeden Data Abort an unseren Gast 0. Damit sind auch `SAS`, `SSE`, `SRT`, `SF`, `AR` ohne Bedeutung.

**Nebenbefunde für das Syndrom (ADR-0010 §5.3 ergänzt):**

* `IL = 1` (folgt aus `ISV = 0`).
* `WnR` ist bei einem External Abort eines atomaren Befehls (LSE, `CAS`) laut `AArch64_FaultSyndrome` (J1.2.196, J1-16021)
  `ARBITRARY`. Ritsu setzt dort 1. Das ist erlaubt und deterministisch.
* `EA` (ISS[9]) ist eine IMPDEF-Klassifikation (D7.6.1, D7-7674). Ritsu setzt 0.
* `SET` (ISS[12:11]) existiert nur mit FEAT_RAS. Ob Ritsu FEAT_RAS meldet, entscheidet die Feature-Baseline
  (WP-ARCH-03). Wird es gemeldet, muss der DBT `SET` füllen (Vorschlag: `0b00` „Recoverable state (UER)“). Die
  Entscheidung gehört in WP-ARCH-03.
* `DFSC = 0b010000` heißt in M.d „Synchronous External abort, not on translation table walk or hardware update of
  translation table“.

## 4. Eingearbeitete Änderungen an ADR-0010 (Status Proposed)

Alle Stellen sind im ADR mit dem Vermerk „R-ARCH-06“ und im Abschnitt *Änderungen* am Ende nachvollziehbar.

| Stelle | Änderung |
|---|---|
| Kontext, Fakten (ISV, `HVC`/`SMC`, `WFI`/`WFE`) | Normative Zitate aus DDI 0487 M.d statt Spiegel/QEMU/LKML. `WFE`-Fakt um das Löschen des globalen Monitors und die Quellen des Event-Registers ergänzt. `Handled` ausdrücklich als Modellabweichung benannt. |
| Kontext, Fakten (neu) | Teilaktualisierung und Wiederholung bei Mehrfachzugriffen (Regeln 3 und 4). |
| §5.2 Mehrfachzugriffe | SError-Regel ersetzt durch synchronen Data Abort mit erlaubter Teilaktualisierung. Device-Wiederholung als Fakt, „höchstens ein Hook-Aufruf pro Einzelzugriff“ als Pflicht. |
| §5.3 Syndrom | `IL = 1`, `EA = 0`, `WnR` bei Atomics, `SET` nur mit FEAT_RAS (WP-ARCH-03). |
| §7.2 `Wfi`-Zeile | Trap aus EL0 nur ohne anstehenden Pegel. |
| §7.2 `WFE`-Absatz | Vollständige Weckliste, Event-Register-Quellen, Polling des Monitors als Pflicht. |
| Folgearbeiten | Verweis auf den jetzt vorliegenden ADR-0009-Nachtrag (§9). |

Die Entscheidung von ADR-0010 (Option C) bleibt unverändert. Keine Korrektur berührt die Schnittstellen-Skizze.

## 5. Vorschläge an den Orchestrator (nicht eingearbeitet)

1. **dbt.md §3.2.1** (`dbt-engineer`, WP-DBT-02): Reihenfolge-Regel nach §3.4 lockern oder `probe` mit Verweis auf
   diesen Bericht als bewusste, strengere Wahl kennzeichnen.
2. **dbt.md §10.5** (`dbt-engineer`): „Geweckt durch … Interrupts“ in „nicht maskierte Interrupts“ ändern. Exception-
   Rückkehr (`ERET`) als Quelle des Event-Registers ergänzen. Die Trap-Reihenfolge aus §3.2 in die
   `WFI`/`WFE`-Beschreibung aufnehmen.
3. **`crates/ritsu-dbt/src/mem.rs`** (`dbt-engineer`): `probe_by_read` darf nie MMIO erreichen (§3.3). Nach WP-MEM-02
   auf `GuestMemory::region_attrs` umstellen (ADR-0009 §9).
4. **testing.md** (`test-engineer`): Differenztest-Orakel für Data Aborts in Mehrfachzugriffen nach §3.4 (nicht
   fehlschlagende Ziele und Load-Zielregister nicht vergleichen).
5. **WP-ARCH-03** (Feature-Baseline): Entscheidung zu FEAT_RAS mit Folge für `ESR_EL1.SET`. FEAT_RAS ist ab Armv8.2
   verpflichtend, die M1-Baseline liegt darüber (**Annahme**, Prüfweg: ISA-Zensus WP-DBT-19).

## 6. Offene Punkte

* Ob XNU `LDXR`+`WFE` ohne `SEV` zum Warten nutzt, ist nicht geprüft (**Annahme**: ja, wie Linux
  `__cmpwait`). Die Korrektur in ADR-0010 §7.2 gilt unabhängig davon, weil sie aus der Architektur folgt.
* Die Seitenzahlen gelten für die PDF-Ausgabe M.d. Arm vergibt Regel-IDs stabil über Ausgaben, die Abschnittsnummern
  können sich ändern.
