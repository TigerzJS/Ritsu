# tests/dbt-ref – Differenztests des DBT gegen eine AArch64-Referenz (WP-DBT-04)

Dieses Paket prüft **T0** (`ritsu_dbt::interp`) gegen eine unabhängige Referenz: echte arm64-Hardware (primär,
GitHub-Runner `ubuntu-24.04-arm`) oder `qemu-aarch64` (sekundär, lokal). Die Strategie steht in
[docs/process/testing.md](../../docs/process/testing.md) §4; hier steht, **wie** die Infrastruktur arbeitet.

Eigenständiges Cargo-Paket mit eigenem `[workspace]` (wie `fuzz/` und `bench/`): Wurzel-`Cargo.toml` und Crate-Graph
bleiben unberührt, es gibt keine externen Abhängigkeiten, nur `ritsu-core`, `ritsu-mem` und `ritsu-dbt` per Pfad.

```
 Generator (Rust, Seed) ──► vectors.bin ──► ritsu-ref-runner (C, static, ohne libc)
        │                                     │  läuft auf arm64-Hardware oder unter qemu-aarch64
        │                                     ▼
        │                                  results.raw ──► pack ──► m0.golden (eingecheckt, ~1,3 MB)
        │                                                              │
        └────────────► T0 (ritsu_dbt::interp) ────────── vergleichen ◄─┘   cargo test / make dbt-ref-check
```

## Befehle

```bash
make dbt-ref-check          # T0 gegen die eingecheckten Golden Files (Teil von make test); ~6 s Debug-Build
make dbt-ref-smoke          # 3,5k frische Vektoren: Runner unter qemu (oder nativ) gegen T0; Teil von make check
make dbt-ref-fresh          # wie smoke, aber ~118k Vektoren und frischer Seed (RITSU_TEST_SEED=… zum Wiederholen)
make dbt-ref-gen            # nur Vektoren erzeugen            -> build/dbt-ref/vectors.bin
make dbt-ref-record         # Vektoren auf der Referenz laufen lassen -> build/dbt-ref/record.golden
make dbt-ref-update         # record.golden als tests/dbt-ref/golden/m0.golden einchecken (Hardware wird nie durch qemu ersetzt)
cargo test --manifest-path tests/dbt-ref/Cargo.toml -- --ignored    # dasselbe wie dbt-ref-fresh als Test (kleiner)
```

Voraussetzungen für alles außer `dbt-ref-check`: `clang` (18) und `ld.lld`, und `qemu-aarch64` (`apt-get install
qemu-user`, getestet mit 8.2.2) bzw. ein arm64-Linux-Rechner. Fehlen die Werkzeuge, meldet `dbt-ref-smoke` „skip: …“
(mit `DBT_REF_REQUIRED=1`, wie in der CI, ist es ein Fehler). Das Skript `tools/dbt_ref.sh` treibt die Pipeline;
`DBT_REF_ORACLE=qemu` erzwingt qemu auch auf arm64.

### Einen Fehlschlag wiederholen

Jeder Bericht endet mit den Wiederholungsbefehlen. Beispiel:

```
RITSU_TEST_SEED=0xb3a73ce2ff2 cargo run --manifest-path tests/dbt-ref/Cargo.toml -- show --units 1000 --class movz --index 4956
RITSU_TEST_SEED=0xb3a73ce2ff2 DBT_REF_UNITS=1000 make dbt-ref-fresh
```

`show` druckt Befehlswörter (mit `llvm-mc`-Disassembly, falls installiert), Anfangsregister und das T0-Ergebnis.
Der Seed eines Vektors ist `hash(Master-Seed, Klassenname, Index)`: neue Klassen oder andere Stückzahlen
verschieben die Vektoren der übrigen Klassen nicht (testing.md §4.5). `RITSU_TEST_SEED` setzt den Master-Seed
(Dezimal oder `0x…`); ohne Angabe gilt der feste Seed `0x5249545355303031`, mit dem die Golden Files entstehen.

## Ausführungsmodell

Ein Vektor ist **Anfangszustand** (X0–X30, SP, NZCV, Seed des Datenfensters) plus **Befehlswörter**. Die Wörter
liegen ab `code_base = 0x10_0000_0100`; dahinter stehen 8 Sentinels `BRK #0xf001`. Der Block endet, wenn

* `PC` die Sentinels erreicht (normales Ende, Signal `SIGTRAP`, `pc >= code_end`), oder
* ein Befehl der Wörter ein Signal auslöst (`UDF` → `SIGILL`, `BRK` → `SIGTRAP` am Befehl, Sprung auf nicht
  abbildbare oder nicht ausführbare Adresse → `SIGSEGV`, unausgerichteter Sprung → `SIGBUS`).

Gemessen wird der **ganze** Zustand: X0–X30, SP, NZCV, PC, Signal und das Datenfenster (256 Bytes bei
`mem_base = 0x20_0000_0000`, Inhalt = splitmix64-Strom aus dem Speicher-Seed; geändert → vollständig im Ergebnis).

Alle generierten Sprünge gehen **vorwärts** auf den Anfang eines späteren Elements oder auf einen der ersten beiden
Sentinels (nie ins Innere eines mehrwortigen Elements): jeder Vektor terminiert. Indirekte Sprünge (`BR`/`BLR`/`RET`)
bekommen ihr Ziel per vorangestelltem `MOVZ`/`MOVK` oder (nur als erstes Element) per Anfangsregister.

### Der Runner (`runner/ritsu-ref-runner.c`)

Statisches aarch64-Linux-Programm ohne libc (nur rohe Syscalls), gebaut mit `clang-18 --target=aarch64-linux-gnu
-static -nostdlib -fuse-ld=lld` (`runner/build.sh`). Pro Vektor: Datenfenster füllen, Codeseite beschreiben
(`mprotect` RW → Cache-Wartung per `DC CVAU`/`IC IVAU`/`DSB`/`ISB` → `mprotect` RX) und anspringen. Die Seite beginnt mit
einem 256-Byte-Prolog, der X0–X30, SP und NZCV lädt und in die Testwörter hineinläuft (deshalb liegen sie bei
`code_base`, `PC`-abhängige Ergebnisse wie `BL` sind damit auf Referenz und in T0 gleich). Das Ende kommt als
Signal; der Handler (Alternativstack) liest den Zustand aus dem Signalkontext und lenkt den `rt_sigreturn` auf
`resume_point`, der die Callee-saved-Register des Runners wiederherstellt. `-t N` bricht nach N Sekunden ab.

### Dateiformate (Little Endian)

**Vektordatei** (`vectors.bin`):

```
Kopf (64 B)  "RITSUDV1" u32 version=1 u32 flags=0 u64 seed u64 code_base u64 mem_base
             u32 count u32 mem_len u32 gen_version u32 units u8[8] reserviert
Eintrag      u16 nwords  u16 class_id  u32 0  u64 mem_seed  u64 x[31]  u64 sp  u64 nzcv(Bits 31:28)  u32 words[nwords]
```

**Rohergebnis** (`results.raw`, vom Runner geschrieben):

```
Kopf (64 B)  "RITSURR1" u32 version=1 u32 runner_version u32 count u32 mem_len u64 code_base u64 mem_base u64 seed u8[16]
Eintrag      u16 signal  u16 flags(Bit0: Datenfenster geändert)  i32 si_code  u64 pc  u64 x[31]  u64 sp  u64 nzcv
             [mem_len Bytes Datenfenster, wenn Bit0]
```

**Golden File** (`golden/*.golden`): Kopf mit Metadaten (`oracle=hardware|qemu`, `host_cpu`, `host_kernel`, `commit`,
`qemu`, `cc`, …), Seed/Units/Generatorversion und je Klasse `(id, Anzahl, Fingerabdruck der Vektoren)`; danach je
Vektor ein **Delta gegen den Anfangszustand** (Signal, PC relativ zu `code_base`, geänderte Register als LEB128,
NZCV, Datenfenster). Anfangszustände und Wörter stehen **nicht** in der Datei: sie werden aus dem Seed neu erzeugt
und über die Fingerabdrücke abgesichert. Ändert sich der Generator, ohne dass `GEN_VERSION` steigt, scheitert der Test mit
einer klaren Meldung statt gegen falsche Vektoren zu vergleichen. Das genaue Layout steht im Kopfkommentar von
`src/golden.rs`. Größe: ~11 Bytes je Vektor (118 047 Vektoren = 1,3 MB; `gzip -9`: 0,5 MB).

## Orakel und Herkunft der Golden Files

Reihenfolge bei Widerspruch: **Hardware > Arm-Pseudocode > QEMU** (testing.md §4.4). `golden/m0.golden` ist heute eine
**vorläufige Aufnahme mit qemu** (`oracle = qemu`): in der Entwicklungsumgebung gibt es keine arm64-Hardware. Der Job
`dbt-reference` (`ubuntu-24.04-arm`) nimmt dieselben Vektoren auf echter Hardware auf, prüft T0 dagegen
(`make dbt-ref-verify-new`), vergleicht mit der eingecheckten Datei (`make dbt-ref-compare`) und lädt `record.golden`
als Artefakt hoch. Dieses Artefakt ersetzt die qemu-Aufnahme per `make dbt-ref-update DBT_REF_NEW=…`; danach
verweigert `dbt-ref-update`, die Hardware-Aufnahme durch qemu zu überschreiben. Zusätzlich lässt der Workflow
`dbt-ref.yml` nachts frische Seeds auf Hardware laufen und vergleicht sie auf x86 mit T0.

Unabhängige Erwartungen im Paket: (1) die **handgerechneten Vektoren** (`src/handmade.rs`, 47 Fälle mit Ergebnis aus dem
Arm-ARM-Pseudocode, nicht aus T0 abgeleitet): T0 *und* jede Aufnahme müssen sie reproduzieren; (2) die **Encoder**
(`src/enc.rs`) werden gegen Wörter von `llvm-mc-18` geprüft; (3) splitmix64/FNV gegen veröffentlichte Referenzwerte.

## Bekannte Grenzen

* **qemu ist nur sekundär.** `qemu-aarch64` (TCG, CPU `max`) setzt die Semantik selbst um; es teilt Fehlerbilder nicht
  mit T0, kann aber eigene haben. Nicht abgebildet bzw. abweichend: NaN-Payloads/FPCR-Details und Flags einzelner
  SIMD-Befehle, Speicherordnung (TCG im Benutzermodus ist stärker als Arm: Litmus-Tests mit qemu sind wertlos), exklusive
  Monitore (`STXR`-Erfolgsbedingungen), Cache-/Selbstmodifikation (qemu erkennt Schreiben auf Code von selbst,
  fehlende `IC IVAU` fallen nicht auf), ID-Register/`MIDR_EL1`, `DC ZVA`-Blockgröße, `si_code`/Fehleradressen der
  Signale (werden deshalb nie verglichen), PAC-Schlüssel. Weicht qemu von der Hardware ab, gilt die Hardware.
* **Signale statt Exceptions:** Die Referenz ist ein Linux-Benutzerprogramm (EL0). Systemregister, MMU, EL1 und
  Exception-Syndrome sind so nicht prüfbar (testing.md §4.3, §4.6).
* **Der Runner prüft keine HWCAPs.** Befehle, die die Referenz-CPU nicht hat, würden als `SIGILL` erscheinen.
  Die heutigen Klassen sind ARMv8.0; für Erweiterungen (LSE, PAC, FP16, …) braucht der Generator ein Feature-Flag.
* **T0-Modellierung:** T0 hat bei ausgeschalteter MMU keine Seitenrechte. Der Harness (`src/t0.rs`) bildet die des
  Runners nach (nur die Coderegion ist ausführbar). Der Prolog-Bereich vor `code_base` ist in T0 null; generierte
  Vektoren lesen ihn nicht.
* **Masken:** `masks.toml` (testing.md §4.2) gibt es noch nicht, weil die heutigen Klassen keine
  implementierungsdefinierten Ergebnisse haben. Mit der ersten FP-/NEON-Klasse nachziehen.
* **Nicht abgedeckt:** Speicherzugriffe (Datenfenster, Registerzeiger und Vergleich sind vorbereitet, aber keine
  Klasse schreibt oder liest), FP/NEON (Format hat noch keine V-Register), Mehrthread.

## Neue Befehlsklasse hinzufügen (WP-DBT-02 und folgende)

1. Encoder in `src/enc.rs` ergänzen und mit `llvm-mc-18` gegenprüfen (Tabelle `LLVM` im Test).
2. In `src/generator.rs` eine Funktion `fn(&mut Ctx)` schreiben (Elemente `Item::Plain`, `Branch`, `Indirect`;
   Operanden über `rand_value`, Zusicherungen über `seed_reg`/`can_seed`) und eine Zeile in `CLASSES` anhängen
   (**neue, nie wiederverwendete `id`**; Namen und ids bestehender Zeilen nie ändern).
3. Handgerechnete Fälle in `src/handmade.rs` **anhängen** (nur am Ende; Indizes sind Teil des Golden-Fingerabdrucks).
4. Ändert sich ein bestehender Vektor, `GEN_VERSION` erhöhen. Neue Klassen allein brauchen das nicht.
5. `make dbt-ref-smoke`, dann `make dbt-ref-record dbt-ref-update` (und im PR auf die Hardware-Aufnahme des Jobs
   `dbt-reference` warten): `cargo test` verlangt Golden-Daten für jede Klasse der Tabelle.

## Klassen heute (118 047 Vektoren bei `--units 1000`)

`make`-unabhängig: `cargo run --manifest-path tests/dbt-ref/Cargo.toml -- classes` listet sie. M0-Klassen: `movz`,
`movn`, `movk`, `mov_const_seq`, `add_imm`/`adds_imm`/`sub_imm`/`subs_imm` (inkl. SP, XZR, `CMP`/`CMN`, Shift 12),
`b`, `bl`, `br`, `blr`, `ret`, `cbz`, `cbnz`, `b_cond`, `cmp_b_cond`, `nop`, dazu `udf`, `brk`, die Mischklassen
`mix_dp` (2–6 abhängige Datenverarbeitungsbefehle) und `mix_flow` (3–9 Elemente mit Vorwärtssprüngen) sowie `handmade`.
