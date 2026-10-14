# R-DBT-01: Review der Semantik-Schicht `Sem` (WP-DBT-01)

> Status: **Review-Bericht** (2026-10-14). Autor-Rolle: `architect`. Grundlage: `crates/ritsu-dbt/src/sem/`
> (`mod.rs`, `a64.rs`, `concrete.rs`, `prim.rs`), `crates/ritsu-dbt/src/ir/` (`mod.rs`, `builder.rs`, `tests.rs`),
> [dbt.md](../subsystems/dbt.md) §3.2/§3.2.1, §4–§9, §13–§15 und [ADR-0006](../adr/0006-dbt-tiered-design.md).
> Der Bericht ändert keinen Code. Er erfüllt die Akzeptanzbedingung „Review durch architect“ von WP-DBT-01 (dbt.md §17).

## Architektur-Review WP-DBT-01

### Urteil

**Trägt mit Änderungen.**

Die Grundform stimmt und sollte bleiben: Jede Beschreibung ist eine generische Funktion über einem Trait mit
assoziierten Wert-, Flag- und Abbruchtypen, Daten fließen nur über `Sem`-Operationen, PC-Bezüge sind relativ,
skalare Integer gehen bis 64 Bit, und statt Transaktionen gelten Reihenfolge-Regeln. Damit lassen sich T0, ein
Template-JIT ohne IR-Objekt und ein SSA-Builder für Cranelift aus **einer** Quelle ableiten (ADR-0006, E-3).
Die Konformanztests mit unabhängigen Orakeln und der Replay der IR auf `Concrete` sind ein gutes Fundament.

Fünf Lücken müssen geschlossen werden, **bevor** WP-DBT-02 die Zahl der Beschreibungen vervielfacht (P1 unten).
Später wird jede davon teurer, weil sie dann in hunderte Beschreibungen hineinwirkt:

1. Descriptions brauchen eine zweite Klasse von Konstanten: den **Übersetzungskontext** (`TbFlags`).
2. Für **Speicheroperationen** fehlen Form und Fault-Modell. Die Reihenfolge-Regel ist für Mehrfach-Stores
   (`STP`, `ST1 {…}`) in ihrer heutigen Fassung nicht erfüllbar.
3. Es gibt **keine datenabhängige Exception** (`raise_if`).
4. Die Regeln werden nur in Debug-Läufen von `Concrete` geprüft, also nur auf den ausgeführten Pfaden. Eine
   **statische Prüfung pro Beschreibung** fehlt.
5. Es gibt **keine Breitenumwandlung** (`zext`/`sext`/`trunc`).

Keine dieser Lücken stellt die Form selbst in Frage. Alle lassen sich additiv schließen.

### Geprüfte Entscheidungen des Implementierers

| Entscheidung | Bewertung | Begründung | Experiment, das sie kippen würde |
|---|---|---|---|
| **Relative PC** (`pc_rel(offset)`, `branch(…, offset, …)`, IR-Offsets zum Regionseinstieg) | **trägt** | Positionsunabhängiger Code ist die Voraussetzung für Option 1 aus dbt.md §14.3 (persistenter Cache ohne Relokation). Die Kosten in T1 sind ein `mov rax, [r15+PC]` plus `add`/`lea` je VA-abhängigem Wert, und die Sprungziele sind ohnehin Stubs. Nebeneffekt: Eine Übersetzung ist an keine VA gebunden. Damit wird es überhaupt erst möglich, Übersetzungen nach PA zu teilen (gleiche physische Seite an unterschiedlichen VAs). Wo das sicher ist, muss WP-DBT-12/-17 entscheiden (Chaining und mehrseitige T2-Regionen setzen eine stabile VA→PA-Zuordnung voraus). | Bench in WP-DBT-12: `ADRP`/`BL`-lastige Blöcke mit PC-relativer gegenüber absoluter Konstante. Zeigt der Vergleich mehr als 3 % Unterschied in T1, wird nicht persistierter T1-Code auf absolute VAs spezialisiert (nur im Backend, die Beschreibungen bleiben gleich). |
| **`Ty` nur bis 64 Bit**, Vektoren später als eigener assoziierter Typ | **trägt** | 128-Bit-Integer braucht die Integer-ISA nicht: `LDXP/STXP/CASP` sind Paare aus zwei 64-Bit-Werten, und `UMULH/SMULH` werden als `mulhi`-Op mit 64-Bit-Ergebnis modelliert. Getrennte Typen bilden die Registerklassen des Hosts ab (GPR gegenüber XMM). Davon profitieren T1 (keine Klassenwechsel) und T2 (Cranelift unterscheidet `i64` und `i128`/`f64`/`i8x16`). T0 rechnet für Integer-Code nicht mit `u128`. Wichtig für WP-DBT-10: Auch skalares FP gehört in die Vektor-Domäne (siehe P3-1), nicht in `V`. | Prototyp in WP-DBT-10: 20 repräsentative FP/NEON-Befehle (`FADD s/d`, `FMOV x↔d`, `UMOV`, `DUP`, `LD1`, `CNT`, `ADDV`) im Template-Backend absenken. Ergibt mehr als ein Klassenwechsel (GPR↔XMM) pro Befehl im Mittel, muss die Domänengrenze neu geschnitten werden. |
| **Reihenfolge-Regeln statt Transaktionen** (erst alles, was raisen kann, dann Writes; höchstens ein Kontrolltransfer, am Ende) | **trägt mit Änderung** | Sie kosten in keinem Backend etwas und folgen der Struktur des Arm-Pseudocodes (erst rechnen und lesen, dann schreiben). Durchgesehen wurden Loads/Stores mit Writeback, `LDP`, `CASP`, `SWP`, `LDRAA` mit Writeback, `BL/BLR`, `ERET`, `SVC/BRK`, `CBZ/TBZ`, `B.cond`, SP-Alignment-Check und `MSR`. Alle halten die Regel ein, mit einer Ausnahme: **Befehle mit mehreren Speicher-Schreibzugriffen** (`STP`, `ST1–ST4` mit mehreren Registern, `STNP`). Ein Store ist zugleich „kann raisen“ und „Architektur-Write“, deshalb verbietet die Regel wörtlich den zweiten Store. Ob Arm bei einem Fault im zweiten Teilzugriff eine teilweise Aktualisierung des Speichers erlaubt, ist hier nicht belegt (**Annahme**; Prüfweg: Arm ARM DDI 0487, Abschnitte zu Data Aborts und Single-Copy-Atomarität). Die Lösung mit `probe` (P1-2) ist in beiden Lesarten korrekt, weil „kein Teil geschrieben“ immer ein zulässiges Ergebnis ist. | Statische Regelprüfung (P1-4) über alle dekodierbaren Befehlsformen. Lässt sich eine Klasse nicht durch Umordnen oder `probe` regelkonform beschreiben, bekommt sie einen lokalen Puffer, und der Rest bleibt bei Regeln. |
| **`raise` gibt `Result<(), Abort>`** (T0: `ExecError`, T1/T2: `Infallible`) | **trägt** | Mit `?` lassen sich Beschreibungen einmal schreiben. In T1/T2 verschwindet der Fehlerpfad zur Compile-Zeit, in T0 bricht er die Beschreibung genau an der Fault-Stelle ab. Dasselbe Muster trägt `load/store -> Result<…, Abort>` (P1-2). Ergänzt werden muss nur die bedingte Form (P1-3). | Messung T0-MIPS (Ziel ≥ 30, dbt.md §15.2) mit Speicher-Ops. Kostet die `Result`-Weitergabe messbar Durchsatz (> 5 %), wird `Concrete` auf ein Fehler-Feld plus Sentinel umgestellt, ohne die Beschreibungen zu ändern. |
| `BranchKind` als reiner Hinweis | trägt | Hält Return-Stack/Jump-Cache aus der Semantik heraus. | – |
| Kanonische Form (Bits oberhalb der Breite sind 0) und Typ am verbrauchenden Op | trägt | T1 kann ohne Typ-Tags in `V` arbeiten. Der IR-Verifier und Debug-`Concrete` decken Typfehler auf. | – |

### Bewertung gegen die späteren Stufen

**T1-Template-JIT (Ziel < 1 µs/Block).** Durch die Monomorphisierung entsteht keine dynamische Dispatch-Ebene, ein
IR-Objekt gibt es nicht. Die Form trägt, wenn das Template-Backend folgende Punkte erfüllt:
* `V` ist ein kleines `Copy`-Enum `{Imm, Reg, Slot, Cc, …}`. Konstanten bleiben dadurch lazy und landen als
  x86-Immediate im Code, und `cond_holds` → `branch` wird ohne Materialisierung zu `jcc` fusioniert. Die Skizze in
  dbt.md §3.2 hatte `branch(Option<(Cond, F)>)`. Die Umsetzung nimmt `Option<V>`, also muss die Fusion über
  `V::Cc` laufen.
* `F` darf in EFLAGS leben. Das geht nur, weil `F`-Werte strukturell nie eine Befehlsgrenze überleben: Jede
  Beschreibung ist eine Funktion, und `F` wird nur über `set_nzcv` gespeichert. Diese Eigenschaft gehört als
  Regel in die Moduldoku. Über Befehlsgrenzen führt das Backend nur den Zustand „Gast-NZCV liegt in EFLAGS /
  Slot / `CpuState`“.
* Problem 1: `SUBS/CMP` wird heute als `add_with_carry(a, imm(!b), imm(1))` beschrieben, Register-`SUB` als
  `add_with_carry(a, not(b), 1)`. Ein Ein-Pass-Template muss daraus `sub`/`cmp` mit invertiertem CF
  zurückgewinnen (dbt.md §6.2). Beim Register-Fall hat es das `not` zu diesem Zeitpunkt schon emittiert. Das ist
  der häufigste Flag-Setzer. Abhilfe in P2-1.
* Problem 2: Weil `V: Copy` ist, kann das Backend Temporärregister nicht beim Drop freigeben. Es braucht deshalb
  einen Befehlsgrenzen-Hook (P2-2), der alle Temporärwerte einer Beschreibung freigibt.
* Die eigentliche Unbekannte ist der Durchsatz des Template-Backends selbst, nicht die Form. Empfehlung: In
  WP-DBT-12 zuerst einen Prototyp nur für die M0- und WP-DBT-02-Befehle bauen und messen (Median < 1 µs bei
  ~10 Befehlen/Block, also ~100 ns pro Befehl inklusive Decode). Bleibt er über 2 µs, zuerst Decode-Cache und
  Emitter prüfen, erst danach die Form.

**T2 über Cranelift.** `Op` bildet `Sem` 1:1 ab, die IR nutzt SSA mit Blockparametern, und `IntCC` ist identisch.
Das Lowering ist geradlinig. Was mit WP-DBT-02 ff. in der IR fehlt:
* Effektklassen pro Op (`Pure`, `ReadsState`, `WritesState`, `MayRaise`, `ReadsMem`, `WritesMem`, `EndsBlock`).
  Ohne sie können State Promotion, DCE und Lade-/Speicher-Elimination (dbt.md §4.4) nicht korrekt arbeiten.
* `Op::operands()` mit festen drei Slots reicht nicht. `cas_pair` hat Adresse plus 4 Werte, ein Store hat Adresse
  plus Wert plus Attribute.
* `Exit::Raise` hat keine Wert-Operanden. Das genügt, solange Daten-Aborts im Speicher-Op selbst entstehen
  (P1-2) und `raise_if` nur konstante Exceptions trägt (P1-3).
* Der Verifier arbeitet mit dichten `Vec<bool>`-Mengen pro Block, also O(Blöcke × Werte). Für T2-Regionen mit
  einigen tausend Werten ist das unkritisch, weil T2 asynchron kompiliert. Bitsets genügen, sobald es misst.

**Speicherzugriffe mit TLB-Fastpath und präzisen Faults (WP-DBT-02/06).** Die Form trägt, wenn der Speicherzugriff
eine **fusionierte** `Sem`-Op ist (Adresse in `V`, Attribute als Konstanten). Die Beschreibung berechnet nur die
Adresse. TBI-Kanonisierung, Wahl des `mmu_idx`, Alignment-Prüfung, TLB-Lookup, Slow-Path und MMIO bleiben im
Backend: in T0 über `Translate` plus `GuestMemory`, in T1 als Inline-Sequenz aus dbt.md §7.3 mit kaltem Stub.
Daten-Aborts samt `FAR`/`ESR` erzeugt das Backend aus Adresse und `MemAttrs`, nicht die Beschreibung. Die
Präzision folgt aus der Reihenfolge-Regel. Der kalte Stub schreibt die gecachten Register der *vorherigen* Befehle
zurück, setzt `PC = Block-VA + pc_offset` und verlässt den Block. Der laufende Befehl hat dann noch nichts
geschrieben.

Abhängigkeit: Für mehrere vCPU-Threads braucht `Concrete` eine Speicherschnittstelle mit `&self` und atomaren
Operationen. Dieselbe Schnittstelle braucht T1 als Host-Basisadresse für den Fastpath. Beides wird im
ADR-Entwurf [ADR-0009](../adr/0009-shared-guest-ram.md) festgelegt (O-DBT-1). T0-Atomics müssen
**Hardware-Atomics auf demselben Speicher** sein, den T1 per `lock cmpxchg` benutzt. Ein Rust-Mutex in T0 wäre
nicht atomar zu T1-Code auf einer anderen vCPU.

**NEON/128 Bit (WP-DBT-10).** Die Form trägt. Ein zusätzlicher assoziierter Typ `type Q: Copy` (128-Bit-Vektor)
bricht nur die drei Backends, nicht die Beschreibungen. Zu entscheiden: siehe P3-1.

**PAC (WP-DBT-11).** Die Form trägt. PAC sind eigene Ops (`pac_sign`, `pac_auth`, `pac_strip`, `pac_ga`) mit
Schlüssel als Decode-Zeit-Konstante. Ob ein Schlüssel freigegeben ist (`SCTLR_EL1.EnIA…`) und wo das PAC-Feld liegt
(TBI), sind Übersetzungskontext (P1-1). `RETAA` ist `pac_auth` plus `branch_indirect`, `LDRAA` ist `pac_auth`
plus `load` plus Writeback. Beides hält die Reihenfolge-Regel ein. Mit FPAC (vorerst nicht gemeldet, dbt.md §9.2)
käme ein datenabhängiger Fault hinzu, also `raise_if` oder ein `Result` von `pac_auth`.

**Exklusive/LSE (WP-DBT-09/14).** Die Form trägt mit eigenen Ops: `load_exclusive`,
`store_exclusive -> status`, `clear_exclusive`, `cas`, `cas_pair -> (V, V)`, `atomic_rmw(op)`, `fence(kind)`. Die
Ordnung (`Plain/Acquire/AcquirePc/Release`) gehört in `MemAttrs`. 128-Bit-Paare werden zu zwei `V`-Werten, ohne
`I128`. Monitor und CAS-Emulation leben im Backend. Die Beschreibung von `STXR` ist
`status = store_exclusive(…)?; set_gpr(ws, status)` und damit regelkonform.

### Änderungsvorschläge (priorisiert)

**P1: vor bzw. als erster Schritt von WP-DBT-02 (blockierend für weitere Befehlsklassen)**

1. **Übersetzungskontext als zweite Konstantenklasse.** Die Regel wird erweitert: Rust-Kontrollfluss ist erlaubt
   über Decode-Zeit-Konstanten (`Inst`) **und** über den Übersetzungskontext `Ctx`, eine Sicht auf `TbFlags`
   (EL, PAN, TBI0/1/TBID, `SCTLR_EL1.{SA,SA0,UCI,EnIA…}`, FP-Freigabe `CPACR_EL1.FPEN`, später FPCR-Modus). Neue
   Trait-Methode: `fn ctx(&self) -> Ctx`. T0 berechnet `Ctx` aus dem `CpuState` mit **derselben** Funktion, aus
   der T1/T2 `TbFlags` bilden, und cacht ihn bis zum nächsten kontextändernden Write. T1/T2 nehmen ihn aus dem
   Block-Schlüssel. Invariante (steht schon in dbt.md §10.1): Jeder Write auf ein Feld, das in `Ctx` eingeht,
   beendet den Block. Ohne diesen Punkt muss jede Rechteprüfung (EL0-Zugriff auf `MRS`, `IC IVAU` mit `UCI`,
   PAN) entweder als Laufzeit-`select` geschrieben werden, was T1 verschlechtert, oder pro Backend dupliziert
   werden, was ADR-0006 verletzt.
2. **Speicher-Ops und Fault-Modell festlegen**, bevor die ersten Loads/Stores beschrieben werden:
   `load(ty, addr, MemAttrs) -> Result<V, Abort>`, `store(ty, addr, v, MemAttrs) -> Result<(), Abort>`, dazu
   `probe(addr, size, AccessKind) -> Result<(), Abort>`. `MemAttrs` enthält nur Konstanten: Größe, Vorzeichen,
   `unpriv` (`LDTR/STTR`), Ordnung, Alignment-Pflicht, ESR-relevante Befehlsangaben (WnR, Größe,
   Transferregister; ob ISV für Aborts nach EL1 gefüllt werden muss, ist **Annahme**, Prüfweg Arm ARM
   `ESR_ELx.ISS` für EC 0x24/0x25). Ergänzung der Reihenfolge-Regel: *„Ein Befehl mit mehr als einem
   Speicher-Write ruft zuerst `probe` für alle Ziele auf. Danach darf nur noch der erste Store einen
   Übersetzungs-Fault auslösen, und auch der nur über `probe`-gedeckte Adressen.“* Für MMIO-Ziele von
   Mehrfach-Stores meldet das Backend einen Bus-Fehler als SError statt synchron (Arm erlaubt externe Aborts
   asynchron, **Annahme**, gleicher Prüfweg). Einzelstores und Loads brauchen kein `probe`.
3. **`raise_if(cond: V, e: Exception) -> Result<(), Abort>`.** Die Exception bleibt eine Konstante
   (`Undefined{word}`, `Svc{imm}`, `Brk{imm}`, `Hvc{imm}`, `Smc{imm}`, `SpAlignment`, `SysRegTrap{…}`,
   `FpAccess`). Je Variante wird die *bevorzugte Rücksprungadresse* dokumentiert: `SVC`/`HVC`/`SMC` liefern
   `PC+4`, Faults den eigenen `PC`. `raise_if` zählt als „kann raisen“ und steht damit vor dem ersten Write, ist
   aber kein Kontrolltransfer. Erster Nutzer ist `CheckSPAlignment()` in WP-DBT-02.
4. **Statische Regelprüfung über alle Beschreibungen.** `Builder` oder ein eigener IR-Check prüft pro
   `pc_offset`: kein `SetGpr/SetNzcv/Store` vor einem `MayRaise`-Op derselben Instruktion (mit der Ausnahme aus
   P1-2), höchstens ein Kontrolltransfer, danach keine Ops mehr in erreichbaren Blöcken. Ein Test senkt für
   **jede dekodierbare Befehlsform** (Zufallswörter in großer Zahl plus die Randwörter jeder Klasse, später der
   2³²-Lauf aus WP-DBT-05) über `Builder` ab und wendet den Check an. Da datenabhängige Wahl nur über `select`
   läuft, enthält die IR eines Befehls alle Ops jedes Laufzeitpfads. Der Check ist daher pro Befehlsform
   vollständig. Die Debug-Asserts in `Concrete` sehen dagegen nur die ausgeführten Pfade. Heute akzeptiert
   `Builder` z. B. zwei Branches in einer Beschreibung stillschweigend.
5. **Breitenumwandlung** `zext(from, to, v)`, `sext(from, to, v)`, `trunc(from, to, v)` als eigene Ops. Ohne sie
   lassen sich `ADD (extended register)`, `LDRSW`, `SXTW`, `UBFM`-Spezialfälle und 32-Bit-Ergebnisse in
   64-Bit-Adressrechnung nicht typkorrekt beschreiben (heute erzwingt die Typregel, dass ein `I32` nicht als `I64`
   weiterverwendet wird).

**P2: vor WP-DBT-12 (T1)**

1. **Subtraktion als eigene Op:** `sub_with_carry(ty, a, b, cin) -> (V, F)` ≡ `AddWithCarry(a, NOT b, cin)`, mit
   Konformanztest gegen genau diese Gleichung. `SUBS/CMP` (`cin = 1`) und `SBC/SBCS` werden damit für T1 direkt zu
   `sub/cmp/sbb` mit bekannter CF-Inversion (dbt.md §6.2). Das lazy `F` in T1 merkt sich „Subtraktionsart“.
2. **Befehlsgrenzen-Hooks** in einem Treiber-Trait (z. B. `trait Lower: Sem { fn begin_insn(&mut self, pc_offset:
   i64); fn end_insn(&mut self); }`). Die Blockbildung (Ende an Branch, Seitengrenze, 64 Befehlen,
   `EndsBlock`-Ops) wird dadurch **einmal** für T1 und T2 geschrieben. T1 gibt in `end_insn` die Temporärwerte
   frei. `Builder::set_pc_offset` geht darin auf.
3. **`BranchAddr`/TBI als Beschreibungs-Hilfsfunktion** (`fn branch_addr<S: Sem>(s, target) -> V` in `a64.rs`,
   gesteuert über `Ctx`), statt es in drei Backends zu implementieren. Die Doku von `branch_indirect` („The
   backends implement it, so the fix lands once“) widerspricht sich selbst: drei Backends sind nicht „once“.
4. **Flag-Ops vervollständigen:** `flags_logic(ty, result) -> F` (`ANDS/TST`: C = V = 0), `flag_c(F) -> V(I1)`
   (`ADC/SBC`), `flags_from_imm(nzcv) -> F` und `select_flags(cond, F, F) -> F` (`CCMP/CCMN`),
   `flags_to_v`/`v_to_flags` (`MRS/MSR NZCV`). In der Moduldoku festhalten: „`F`-Werte überleben keine
   Befehlsgrenze“.
5. **IR nachziehen:** Effektklassen pro `Op`, variable Operandenzahl, `Exit::Sync{reason}`/`EndsBlock`,
   `CallHelper{id, args, effects}` (dbt.md §4.2). Dazu eine `end_block(reason)`-Op in `Sem` für `ISB` und
   kontextändernde `MSR` (WP-DBT-07).

**P3: mit WP-DBT-09/10/11**

1. **Vektor-Domäne:** `type Q: Copy` (T0: `u128`, T1: XMM oder Slot, T2: `Value` vom Typ `V128`), Ops mit einem
   Anordnungs-Deskriptor `VecTy { lane, lanes }`, damit nicht pro Anordnung eine Op entsteht. **Skalares FP
   operiert auf `Q`** (Arm: `Sn/Dn` sind die unteren Lanes von `Vn`, ein Write nullt den Rest; x86: `addss/addsd`
   auf XMM). Explizite Domänenwechsel `q_lane_to_v`, `v_to_q_lane` (`FMOV x↔d`, `UMOV`, `INS`, `DUP`). Der
   FPCR-Modus (`RMode`, `FZ`, `FZ16`, `DN`, `AH`) kommt in `Ctx`, `MSR FPCR` beendet dann den Block, und
   kumulierte FPSR-Flags laufen über eine eigene Op. FP-Exception-Traps werden nicht gemeldet (`FPCR.*E` RAZ/WI),
   damit es keinen datenabhängigen FP-Raise gibt.
2. **Atomare Ops** wie oben unter „Exklusive/LSE“. Die Speicherschnittstelle stellt ADR-0009 bereit.
3. **PAC-Ops** wie oben. Ob das PAC-Feld (abhängig von TxSZ) über `Ctx` oder zur Laufzeit aus `TCR_EL1` bestimmt
   wird, entscheidet WP-DBT-11 per Messung.
4. Niedrig: Varianten von `Inst` als benannte Structs (`AddSubImm` usw.), damit die Beschreibungen nicht an
   `clippy::too_many_arguments` stoßen. Compile-Zeit und Codegröße der dreifachen Monomorphisierung ab etwa
   300 Beschreibungen messen.

### Neue Risiken

* **Formale Soundness von T0 auf geteiltem Gast-RAM:** Rust verbietet unsynchronisierte, teilweise überlappende
  atomare Zugriffe unterschiedlicher Größe ([`std::sync::atomic`](https://doc.rust-lang.org/std/sync/atomic/index.html),
  Abschnitt *Memory model for atomic accesses*). Genau solche Zugriffe kann der Gast erzeugen (Byte-`LDADDB`
  neben 64-Bit-`STR`). Behandlung in ADR-0009 (Zugriffe nur über `host_map`-Primitive).
* **Op-Inflation:** Die Schätzung in dbt.md (~150 Ops) wird mit NEON deutlich überschritten, wenn nicht über
  `VecTy` parametrisiert wird. Jede Op kostet Konformanztests in drei Backends.

### Folge für Dokumente (Vorschlag an den Orchestrator)

* dbt.md §3.2.1: Regeltext um `Ctx` (P1-1), `probe` (P1-2), `raise_if` (P1-3) und die statische Prüfung (P1-4)
  ergänzen. In §17 bei WP-DBT-01 „architect-Review offen“ durch einen Verweis auf diesen Bericht ersetzen.
  Pflege: `dbt-engineer` im Rahmen von WP-DBT-02.
