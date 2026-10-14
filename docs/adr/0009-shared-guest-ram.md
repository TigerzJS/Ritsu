# ADR-0009: Gast-RAM wird ein geteiltes memfd-Mapping (`GuestRam`) mit `&self`-Zugriff; `unsafe` nur in `ritsu_mem::host_map`

* **Status:** Proposed
* **Datum:** 2026-10-14
* **Entscheider:** Mensch (Freigabe), `architect` (Entwurf)
* **Betrifft:** `crates/ritsu-mem` (Eigentümer), `crates/ritsu-dbt`, `crates/ritsu-virtio`, `crates/ritsu-platform`,
  `crates/ritsu-vmm`, `crates/ritsu-gpu-host` (nur über eine Port-Schnittstelle, keine neue Crate-Kante), `fuzz/`

## Kontext

**Ist-Zustand (M0).** `ritsu_mem::GuestMemory` (`crates/ritsu-mem/src/lib.rs`) ist die eingefrorene Schnittstelle
zwischen CPU- und Geräteseite: `read(&self, …)`, `write(&mut self, …)` plus `u32`/`u64`-Helfer. Einzige
Implementierung ist `RamRegion` (ein `Vec<u8>`). Nutzer: `ritsu-dbt` (`interp::step`, `mmu::Translate` lesen
über `&impl/&dyn GuestMemory`), `ritsu-virtio` (`Queue::pop/push_used/should_notify`,
`VirtioDevice::queue_notify(mem: &mut dyn GuestMemory)`), dazu die Fuzz-Ziele `fuzz_t0_step` und
`fuzz_virtqueue_walk`.

**Problem.** `write(&mut self)` verlangt exklusiven Zugriff. Auf denselben Gast-RAM greifen aber gleichzeitig zu:
mehrere vCPU-Threads (T0, T1/T2-Code), Geräte-Threads (virtio-DMA) und später die GPU (Vulkan-Import). Die
Schnittstelle kennt außerdem weder Atomics noch Speicherordnung und liefert keine Host-Adressen.

**Anforderungen aus den Subsystem-Dokumenten**

| ID | Anforderung | Quelle |
|---|---|---|
| R-1 | Mehrere vCPUs greifen **gleichzeitig** zu. Ausgerichtete Zugriffe bis 64 Bit sind single-copy-atomar, dazu `CAS`, LSE-RMW, `CASP`/`LDXP`/`STXP` (16 Byte) und CAS-Emulation für `LDXR/STXR`. T0-Atomics müssen **dieselben Hardware-Atomics** sein, die T1 per `lock cmpxchg` auf dem Host-Mapping ausführt. Sonst ist ein Lock-Wort, das vCPU 0 in T0 und vCPU 1 in T1 bearbeitet, nicht atomar. | dbt.md §8.2–§8.4, §13; O-DBT-1 |
| R-2 | **Inline-TLB-Fastpath:** `addend = Host-Adresse − Gast-VA`. Dafür braucht es eine stabile Host-Basis pro RAM-Region für die gesamte VM-Laufzeit, Guard-Bereiche und bevorzugt 2-MiB-Huge-Pages. | dbt.md §7.3, §7.4 |
| R-3 | **SMC-Schutz:** Seiten mit Übersetzungen tragen im SoftMMU-TLB `NOTDIRTY`. Schreibzugriffe, die *nicht* über den TLB laufen (DMA, T0-Hilfspfade), müssen die Seite ebenfalls als beschrieben melden. Für `IC IALLU` wird ein „seit dem letzten IALLU beschrieben“-Bit pro Seite gebraucht. | dbt.md §7.6 (E-11) |
| R-4 | **virtio-Ringe:** Lesen von `avail.idx` mit Acquire, Schreiben von `used.idx` mit Release, eine volle Barriere vor dem Lesen von `avail.flags`/`used_event` (Store→Load). Ringinhalt und Puffer werden per Bulk-Kopie übertragen. | platform.md §8.1, §9.2; `ritsu-virtio/src/queue.rs` (Moduldoku *Memory ordering*) |
| R-5 | **Geräte-DMA** aus Geräte-Threads, nebenläufig zu den vCPUs. Block-I/O soll ohne Zwischenkopie möglich sein (`preadv/pwritev` direkt in Gast-RAM). | platform.md §8.1, §11 |
| R-6 | **GPU (G-10):** memfd-Backing des Gast-RAM, **Alias-Mappings**, die gast-physisch verstreute Seiten zu einem zusammenhängenden Host-VA-Bereich zusammenlegen (Import per `VK_EXT_external_memory_host`), und **Dirty-Logging** pro Seite (Strategie c). | gpu.md §6.2, G-10, WP-GPU-11 |
| R-7 | Gast-Eingaben sind feindlich: Jeder Zugriff ist bereichsgeprüft, bei Fehler gibt es keinen Teil-Write, kein Panic und keinen SIGBUS. | CLAUDE.md §3 Regel 2, Q-SEC-1 |
| R-8 | Die Abhängigkeitsregel bleibt: `ritsu-gpu-host` hängt **nicht** von `ritsu-mem` ab. | ARCHITECTURE.md §4 |

**Fakten mit Quelle**

* Im Rust-Speichermodell ist ein Data Race UB, also ein nicht synchronisierter, konfliktierender Zugriff, von dem
  mindestens einer nicht atomar ist. Auch *teilweise überlappende atomare Zugriffe unterschiedlicher Größe* ohne
  Synchronisation sind verboten („every pair of non-synchronized atomic accesses must be either disjoint, access
  the exact same memory (including using the same access size), or both be reads“).
  Quelle: [`std::sync::atomic`, Modul-Doku](https://doc.rust-lang.org/std/sync/atomic/index.html). Daraus folgt:
  Eine Rust-Referenz (`&[u8]`/`&mut [u8]`) auf Gast-RAM, den ein anderer Thread gleichzeitig beschreibt, ist UB.
  Auch reine Rust-Atomics genügen für T0 nicht, weil der Gast gemischte Größen auf derselben Adresse erzeugen
  kann (Byte-`LDADDB` neben 64-Bit-`STR`).
* rust-vmm `vm-memory` 0.18.0 (Apache-2.0 OR BSD-3-Clause) bietet `GuestMemoryMmap`, `FileOffset`,
  `VolatileSlice`, einen Bitmap-Mechanismus und `Bytes::load/store<T: AtomicAccess>(…, Ordering)`, aber **kein**
  Compare-Exchange. Abhängigkeiten laut docs.rs: `arc-swap`, `bitflags`, `libc`, `rangemap`, `thiserror`,
  `vmm-sys-util` (teils optional). Quelle: [docs.rs/vm-memory](https://docs.rs/vm-memory/latest/vm_memory/),
  [`Bytes`](https://docs.rs/vm-memory/latest/vm_memory/bytes/trait.Bytes.html).
* Rust `std` bietet weder `memfd_create` noch `mmap`. Dafür braucht es `libc` oder `rustix`. Beides wäre eine
  neue externe Abhängigkeit (CLAUDE.md §3 Regel 7).

**Annahmen**

* **Annahme A-M1:** `VK_EXT_external_memory_host` importiert Zeiger in ein `MAP_SHARED`-memfd-Mapping
  (shmem) auf ANV (Iris Xe) und auf dem NVIDIA-Treiber, mit `minImportedHostPointerAlignment` ≤ 16 KiB.
  *Prüfweg:* Spike in WP-GPU-11 auf dem G5 MF. Rückfall bei Fehlschlag: `udmabuf` (dma-buf aus memfd-Seiten,
  verlangt nach Kernel-Quelle `drivers/dma-buf/udmabuf.c` ein memfd mit `F_SEAL_SHRINK`) plus
  `VK_EXT_external_memory_dma_buf`. Beide Wege setzen memfd voraus.
* **Annahme A-M2:** Transparent Huge Pages für shmem (`/sys/kernel/mm/transparent_hugepage/shmem_enabled` =
  `advise` plus `MADV_HUGEPAGE`) liefern 2-MiB-Seiten für das Haupt-Mapping. Zusätzliche Alias-Mappings
  einzelner 16-KiB-Stücke derselben Datei bleiben trotzdem möglich (dort mit 4-KiB-PTEs). *Prüfweg:*
  `AnonHugePages`/`ShmemPmdMapped` in `/proc/<pid>/smaps` im Test. hugetlbfs (`MFD_HUGETLB`) ist bewusst **nicht**
  gewählt: Seine Mappings sind an 2 MiB gebunden, 16-KiB-Aliase wären damit unmöglich.
* **Annahme A-M3:** Zugriffe aus `core::arch::asm!`-Blöcken auf Speicher, den Rust-Code nie über Referenzen oder
  Rust-Atomics berührt, sind außerhalb des abstrakten Rust-Speichermodells. Sie verhalten sich wie Zugriffe eines
  fremden Prozesses oder einer FFI-Funktion, und ihre Semantik ist die von x86-64. Gemischte Größen sind dann
  hardwaredefiniert. *Prüfweg:* Review durch `code-reviewer` (Schwerpunkt unsafe) gegen die Regeln der Rust
  Reference für Inline-Assembly. Fällt die Annahme, ist der Rückfall `ptr::read_volatile`/`write_volatile` plus
  `lock`-Präfix über `asm!` nur für RMW. Das ist die Praxis von rust-vmm, formal aber ebenfalls nicht definiert.
* **Annahme A-M4:** Für Bulk-Kopien wird `rep movsb` (FSRM auf Alder Lake) eingesetzt. Dessen Stores können
  untereinander umgeordnet sein, deshalb schließt jede Bulk-Kopie, auf die ein Release-Store folgt, mit `sfence`
  ab, bis Intel SDM Vol. 3 (*Memory Ordering*, Fast-String-Operationen) das Gegenteil belegt. *Prüfweg:*
  SDM-Abschnitt und ein Ordnungs-Stresstest in WP-MEM-02.

## Optionen

1. **Option A: Nur die Signatur ändern.** `GuestMemory::write(&self)`, `Send + Sync`, `RamRegion` mit innerer
   Synchronisation (`Mutex` oder `Box<[AtomicU8]>`), dazu geordnete Load/Store-Methoden. Keine Host-Zeiger, kein
   `unsafe`.
   * Vorteile: sehr klein, kein `unsafe`, keine neue Abhängigkeit, für virtio sofort ausreichend.
   * Nachteile: verfehlt R-1 (T0-Atomics über einen Lock sind nicht atomar zu T1-`lock cmpxchg`), R-2 (kein
     `addend`), R-5 (kein Zero-Copy-I/O) und R-6 (kein memfd, keine Aliase). Für den JIT entstünde später eine
     zweite, parallele Schnittstelle, und zwei Wege zum selben Speicher laufen auseinander.
   * Kosten: ~1 Tag. Risiko: niedrig jetzt, hoch später (Doppel-Schnittstelle).
2. **Option B: Eigenes `GuestRam` (memfd + `mmap`) in `ritsu-mem` mit sicherer `&self`-API, `unsafe`-Kern im
   Modul `host_map`, Trait `GuestMemory` v2 als objektsichere Fassade.** Details unter *Entscheidung*.
   * Vorteile: deckt R-1 bis R-8 mit **einem** Eigentümer des Gast-RAM ab. Die `unsafe`-Fläche ist klein und an
     einem Ort. Typen und Fehler bleiben die eigenen (`GuestPhysAddr`, `MemError`). Alias-Mappings, Seitenflags
     (SMC/Dirty) und Host-Basis für den JIT entstehen aus demselben Mapping.
   * Nachteile: eigener `unsafe`-Code (Schätzung 400–700 Zeilen inklusive Tests) mit Review-Aufwand, und eine
     neue Abhängigkeit für Syscalls (`libc`).
   * Kosten: WP-MEM-01 plus WP-MEM-02 (siehe Migration), etwa 2 Agent-Sitzungen plus unsafe-Review. Risiko: mittel
     (Soundness-Fragen A-M3/A-M4, Import-Frage A-M1).
3. **Option C: rust-vmm `vm-memory` übernehmen** (`GuestMemoryMmap` mit `FileOffset` auf ein memfd,
   `Bytes::load/store` mit `Ordering`, `VolatileSlice` für Bulk und I/O, Bitmap für Dirty-Tracking). Ergänzt
   würde eigener Code für CAS/RMW, 16-Byte-CAS, Alias-Mappings, Seitenflags für SMC und JIT-Host-Basis.
   * Vorteile: Region-Verwaltung und volatile Kopien sind praxiserprobt (Firecracker, Cloud Hypervisor), mit
     fertiger Dirty-Bitmap und I/O-Hilfen.
   * Nachteile: Die schweren Teile (CAS/RMW mit gemischten Größen, Alias, SMC-Flags, Guard-Bereiche, `addend`)
     deckt das Crate nicht ab. Wir bräuchten trotzdem eigenen `unsafe`-Code über `get_host_address`. Dazu kämen
     zwei Adress- und Fehlermodelle (`GuestAddress` neben `GuestPhysAddr`), die bis in `ritsu-virtio` lecken oder
     gewrappt werden müssen. Weitere Punkte: 0.x-Versionen mit häufigen Breaking Changes, fünf bis sechs
     zusätzliche transitive Abhängigkeiten, und die Lizenzfrage O-1 des Projekts ist noch offen.
   * Kosten: ähnlich wie B plus ein Adapter, laufender Update-Aufwand. Risiko: mittel (fremde API-Evolution,
     unklare Abdeckung von R-1).

Verworfen ohne nähere Ausarbeitung: ein globaler `RwLock` um den RAM (verfehlt R-1/R-2 wie A, dazu Konkurrenz auf
dem Lock) und Seitenschutz per `mprotect` für SMC/Dirty (Host-Signale im JIT, teuer, kollidiert mit dem
Software-TLB-Konzept aus dbt.md §7.6).

## Entscheidung

**Option B.** Ab Freigabe gilt:

### 1. Ein Eigentümer: `ritsu_mem::GuestRam`

* `GuestRam` besitzt den gesamten gast-physischen RAM einer VM. Das ist **ein memfd** (`MFD_CLOEXEC`,
  `MFD_ALLOW_SEALING`), per `ftruncate` auf Endgröße gebracht und danach mit `F_SEAL_SHRINK | F_SEAL_GROW`
  versiegelt. Niemand kann die Datei mehr verkleinern, damit gibt es kein SIGBUS. Die RAM-Regionen (Layout aus
  `MachineConfig`/Spec `memory-map`) werden mit `MAP_SHARED | MAP_FIXED` in **eine** vorher reservierte
  `PROT_NONE`-Reservierung gelegt, mit Guard-Bereichen (≥ 64 KiB) davor, dahinter und zwischen den Regionen, dazu
  `MADV_HUGEPAGE` (A-M2).
* Die Regionen stehen nach dem Bau fest (kein Hotplug). Lookups GPA → Region sind damit lock-frei.
* Geteilt wird über `Arc<GuestRam>`. Das Haupt-Mapping lebt, bis der letzte `Arc` fällt.

### 2. Sichere Zugriffs-API (alle Methoden `&self`, bereichsgeprüft, kein Teil-Write bei Fehler)

Skizze (verbindlich sind die Eigenschaften, die exakten Namen legt WP-MEM-01/-02 fest):

```rust
pub trait GuestMemory: Send + Sync {                       // objektsicher: virtio nutzt &dyn GuestMemory
    fn read(&self, gpa: GuestPhysAddr, buf: &mut [u8]) -> Result<(), MemError>;   // Bulk, keine Ordnung
    fn write(&self, gpa: GuestPhysAddr, data: &[u8]) -> Result<(), MemError>;     // Bulk, meldet Schreiben (§4)
    fn load(&self, gpa: GuestPhysAddr, w: Width, ord: MemOrder) -> Result<u64, MemError>;
    fn store(&self, gpa: GuestPhysAddr, w: Width, v: u64, ord: MemOrder) -> Result<(), MemError>;
    fn compare_exchange(&self, gpa: GuestPhysAddr, w: Width, current: u64, new: u64)
        -> Result<Result<u64, u64>, MemError>;                                   // SeqCst (lock cmpxchg)
    fn compare_exchange_u128(&self, gpa: GuestPhysAddr, current: u128, new: u128)
        -> Result<Result<u128, u128>, MemError>;                                 // lock cmpxchg16b
    fn fetch_rmw(&self, gpa: GuestPhysAddr, w: Width, op: RmwOp, operand: u64) -> Result<u64, MemError>;
    fn fence(&self, ord: MemOrder);
    // bereitgestellt: read_u16/u32/u64, write_u16/u32/u64 (Relaxed), wie bisher
}
// Width = W8 | W16 | W32 | W64;  MemOrder = Relaxed | Acquire | Release | SeqCst;
// RmwOp = Swap | Add | And | Or | Xor | AndNot | SMax | SMin | UMax | UMin   (LSE-Satz, dbt.md §8.4)
```

* `Width` + `u64` statt generischer Methoden, damit der Trait objektsicher bleibt (`&dyn GuestMemory` in
  `VirtioDevice::queue_notify`).
* Atomare Ops verlangen natürliche Ausrichtung. Andernfalls gibt es den neuen Fehler `MemError::Misaligned`
  (`MemError` ist bereits `#[non_exhaustive]`). Ein Zugriff muss vollständig in **einer** Region liegen.
* Unzulässige Kombinationen (z. B. `load` mit `Release`) behandelt die Implementierung als `SeqCst` und meldet sie
  per `debug_assert!`, ohne Panic im Release-Build.
* Semantik von `MemOrder` gegenüber dem Gast: Ein `load(…, Acquire)` sieht alle Gast-Writes, die der Gast vor
  seinem ordnenden Write auf diese Adresse ausgeführt hat (Gast-`DMB`/`STLR`, auf x86-TSO übersetzt nach
  dbt.md §8.2). `store(…, Release)` macht alle vorherigen Writes dieses Threads (inklusive `write`-Bulk) vorher
  sichtbar. `fence(SeqCst)` ist eine volle Barriere (`mfence`).
* **Implementierungen:** `GuestRam` (Produktion) und `RamRegion` (Referenz- und Test-Backend, intern ein `Mutex`
  über dem Puffer, damit trivial linearisierbar und ohne `unsafe`). Test- und Fault-Injection-Backends bleiben
  so möglich, etwa der dünn besetzte Speicher des Linux-User-Harness (dbt.md §16.3).
* `mmu::Translate::translate(&mut self, mem: &dyn GuestMemory, …)` bleibt unverändert. Deskriptor-Reads beim
  Walk laufen über `load(…, W64, Relaxed)` (single-copy-atomar, WP-DBT-06).

### 3. Host-Adressen für JIT und Kernel-I/O (sicher zu erhalten, `unsafe` zu benutzen)

* `GuestRam::host_range(gpa, len) -> Result<HostRange, MemError>` liefert `{ ptr: NonNull<u8>, len }`, bereits
  gegen eine Region geprüft. `HostRange` ist `Copy + Send + Sync`, enthält **keine** Referenz und ist gültig,
  solange ein `Arc<GuestRam>` lebt. Der Inhaber muss dieses `Arc` halten (der JIT im vCPU-Kontext, das Gerät in
  seinem Zustand).
* Der JIT (WP-DBT-12) bildet daraus `addend`. TLB-Einträge entstehen nur für RAM-Seiten (dbt.md §7.4), jeder
  Fastpath-Zugriff liegt daher in einer `HostRange`.
* Zero-Copy-Block-I/O: `GuestRam::pread_into(fd, gpa, len, file_off)` / `pwrite_from(…)` (und eine
  `iovec`-Variante für Deskriptorketten) rufen `preadv/pwritev` mit **Rohzeigern** auf. Ein `IoSliceMut`
  (= `&mut [u8]`) wird nie gebildet. Danach melden sie den Bereich als beschrieben (§4).
* Wer an `GuestRam` vorbei schreibt (Kernel-I/O außerhalb dieser Helfer, GPU über einen importierten Alias), muss
  `GuestRam::note_external_write(gpa, len)` aufrufen, sonst greifen SMC-Absicherung und Dirty-Log nicht. Die
  GPU-Seite tut das für Strategie (a) nach Completion für die von ihr beschriebenen Bereiche (gpu.md §6.2,
  Shader-Reflexion).

### 4. Seitenzustand für SMC und Dirty-Logging (eine Tabelle, zwei Nutzer)

* `GuestRam` führt pro 16-KiB-Gastseite ein `AtomicU8` mit den Bits `CODE` (Seite hat Übersetzungen, gesetzt vom
  DBT), `CODE_WRITTEN` (seit dem letzten `IC IALLU` beschrieben), `WATCH` (Dirty-Logging scharf) und `DIRTY`
  (seit der letzten Ernte beschrieben). Bei 64 GiB RAM sind das 4 Mi Einträge, also 4 MiB.
* Jeder Write über die sichere API (`write`, `store`, `compare_exchange` bei Erfolg, `fetch_rmw`, `pread_into`)
  macht **erst** den Write, **dann** einen Relaxed-Load der Flags. Ist `CODE` oder `WATCH` gesetzt, folgt ein
  `fetch_or` mit Release auf `CODE_WRITTEN`/`DIRTY`, und für `CODE` wird `WriteObserver::code_written(page)`
  aufgerufen. Die Reihenfolge „erst Write, dann Markierung“ garantiert: Wer `DIRTY` erntet (Acquire) und dann
  liest, sieht den Write.
* Der JIT-Fastpath umgeht die API. Der DBT setzt beim TLB-Füllen für Seiten mit `CODE` oder `WATCH` das
  `NOTDIRTY`/`WATCH`-Flag in `cmp_write` (dbt.md §7.3). Der Slow-Path schreibt dann über die API, die markiert.
  **Scharf geschaltete `WATCH`-Seiten bleiben dauerhaft im Slow-Path.** Ernten (`harvest_dirty`, atomarer Swap
  der `DIRTY`-Bits) braucht daher keinen TLB-Shootdown. Das passt zu gpu.md Strategie (c), die nur für Buffer mit
  seltenen CPU-Writes gedacht ist.
* **Scharfschalten** (`arm_write_watch(range)`) setzt `WATCH` und ruft `WriteObserver::revoke_fast_writes(range)`
  auf. Diese Methode kehrt erst zurück, wenn keine vCPU die Seiten mehr ohne Slow-Path beschreiben kann
  (synchroner Shootdown, dbt.md §13.3). Erst danach ist das Dirty-Log für den Bereich vollständig.
  Entschärfen darf lazy sein: Ein veralteter `WATCH`-Eintrag führt nur harmlos in den Slow-Path.
* Für `CODE` entscheidet der DBT selbst, ob er beim Setzen sofort einen Shootdown macht. Mechanismus 2 aus E-11
  ist eine Absicherung, die Korrektheit kommt aus `IC IVAU` (dbt.md §7.6).
* `WriteObserver` ist ein Trait in `ritsu-mem` (`Send + Sync`), implementiert von `ritsu-dbt` und verdrahtet in
  `ritsu-vmm`. Damit entsteht keine neue Crate-Kante.

### 5. Alias-Mappings für die GPU (G-10)

* `GuestRam::alias(pages: &[GuestPhysAddr]) -> Result<AliasMapping, MemError>` legt die Seiten (16-KiB-ausgerichtet,
  RAM, keine Duplikate nötig) in eine neue Reservierung. Jede Seite wird per `mmap(MAP_SHARED | MAP_FIXED, memfd,
  offset)` eingeblendet. `AliasMapping` besitzt den VA-Bereich, gibt ihn bei `Drop` frei und liefert
  `host_range()`. Es hält intern ein `Arc<GuestRam>`.
* Der Nutzer muss das `AliasMapping` länger leben lassen als jede externe Nutzung (Vulkan: die importierte
  `VkDeviceMemory`). Das ist eine `unsafe`-Vertragsbedingung **auf der GPU-Seite**.
* `ritsu-gpu-host` sieht `ritsu-mem` nicht (R-8). Es definiert stattdessen einen eigenen Port-Trait (Arbeitstitel
  `GuestMemoryPort`: `alias`, `read`/`write` für Upload-Kopien, `arm_write_watch`/`harvest_dirty`,
  `note_external_write`), den `ritsu-vmm` mit einem Adapter über `GuestRam` implementiert. Die genaue Form legt
  WP-GPU-11 fest (gpu-engineer mit architect), gebunden an die Invarianten dieses ADR.

### 6. `unsafe`-Grenze

* `ritsu-mem` bekommt `#![deny(unsafe_code)]` auf Crate-Ebene. **Einzige Ausnahme:** das Modul `host_map`
  (`#[allow(unsafe_code)] mod host_map;`). `host_map` ist nicht `pub`. Nach außen gehen nur sichere Typen
  (`GuestRam`, `HostRange`, `AliasMapping`).
* Inhalt von `host_map`: memfd-Erzeugung und Versiegelung, Reservierung und Guards, Region- und Alias-Mappings
  (inklusive `munmap` im `Drop`), `madvise`, die Zugriffsprimitive (Bulk-Kopie in und aus dem Gast, Load/Store
  8–64 Bit, `lock cmpxchg` 8–64 Bit, `lock cmpxchg16b`, RMW), `preadv/pwritev` mit Rohzeigern.
* **Invarianten** (stehen als Moduldoku in `host_map`, jede `// SAFETY:` verweist darauf):
  * **I-1** Es entsteht **nie** eine Rust-Referenz (`&T`, `&mut T`, `&[u8]`, `IoSlice*`) auf Gast-RAM oder ein
    Alias-Mapping.
  * **I-2** Rust-Code greift auf Gast-RAM nur über die Primitive von `host_map` zu. Diese sind in
    `core::arch::asm!` implementiert (A-M3), also keine Rust-Atomics und kein `copy_nonoverlapping` auf Gast-RAM.
  * **I-3** Primitive nehmen nur einen `Checked`-Bereich entgegen. Dieser Typ ist privat in `host_map` und nur
    über die Bereichsprüfung der Regionstabelle konstruierbar (überlaufgeprüft, vollständig innerhalb einer
    Region). Damit sind die Primitive selbst sichere Funktionen.
  * **I-4** Haupt-Mapping und memfd leben, solange ein `Arc<GuestRam>` existiert. Ein `AliasMapping` lebt bis zu
    seinem `Drop` und hält das `Arc`. Kein Code unmappt Teile des Haupt-Mappings zur Laufzeit.
  * **I-5** Die memfd-Größe ist nach dem Bau versiegelt (`F_SEAL_SHRINK | F_SEAL_GROW`), kein SIGBUS durch
    Kürzen.
  * **I-6** Atomare Primitive setzen natürliche Ausrichtung voraus (geprüft in `Checked`). Damit gibt es keine
    Split-Locks (dbt.md §8.2).
  * **I-7** Auf eine Bulk-Kopie in den Gast, der ein Release-Store folgen kann, folgt `sfence` (A-M4).
* Neue Abhängigkeit (Entscheidung des Orchestrators, CLAUDE.md §3 Regel 7): **`libc`**, nur in `ritsu-mem` und
  nur in `host_map`. Begründung: `memfd_create`, `mmap`, `fcntl(F_ADD_SEALS)`, `madvise`, `preadv` mit Rohzeigern
  (`iovec` hat `*mut c_void`, also keine Referenz, I-1). Erwogene Alternative: `rustix`. Es ist sicherer
  typisiert, seine I/O-Funktionen nehmen aber Slices, was I-1 verletzt.

### 7. Speicherordnung virtio (Pflicht für `ritsu-virtio`)

* `pop`: `avail.idx` mit `load(W16, Acquire)`, danach Ring und Deskriptoren mit `read`.
* `push_used`: zuerst den Eintrag mit `write`, dann `used.idx` mit `store(W16, Release)`.
* `should_notify`: `fence(SeqCst)` zwischen `used.idx`-Store und dem Lesen von `avail.flags` (Store→Load, sonst
  verlorene Interrupts). Die genaue Fundstelle in virtio 1.2 §2.7.7/§2.7.10 ist **Annahme** und wird in der
  Migration nachgetragen.
* Die Moduldoku-Passage „requirement on the backend“ in `queue.rs` entfällt, die Pflicht liegt jetzt im Aufrufer.

### 8. Migrationsplan

| Schritt | WP (Vorschlag) | Agent | Inhalt | Bricht |
|---|---|---|---|---|
| 1 | **WP-MEM-01** | dbt-engineer | `GuestMemory` v2 (`&self`, `Send + Sync`, `load/store/compare_exchange/…`, `Width`, `MemOrder`, `RmwOp`, `MemError::Misaligned`). `RamRegion` auf `Mutex` umgestellt. „Frozen contract“-Doku in `lib.rs` verweist auf ADR-0009. | Nur Implementierer des Traits (heute nur `RamRegion`). Aufrufer bleiben quellkompatibel: `&mut M`/`&mut dyn GuestMemory` koerzieren auf `&self`-Methoden, auch in `ritsu-virtio` und `fuzz/`. |
| 2 | **WP-MEM-02** ★ | dbt-engineer, Review code-reviewer (unsafe) | `GuestRam` mit `host_map` (§1–§6), Seitentabelle, `WriteObserver`, `alias`, `pread_into/pwrite_from`, `host_range`. Tests: Mehr-Thread-CAS-Zähler (8 Threads × 10⁶), gemischte Größen auf derselben Adresse, Grenz- und Überlauffälle ohne Teil-Write, Alias-Kohärenz (Write über das Haupt-Mapping, Read über den Alias), Seal-Test (`ftruncate` scheitert), Ordnungs-Stresstest (Message Passing mit Release/Acquire), Fuzz-Ziel für Bereichsprüfung. | nichts (additiv) |
| 3 | **PLAT-xx** | platform-engineer | `ritsu-virtio`: Ordnung nach §7. Signaturen `push_used(&mut self, mem: &M)` (statt `&mut M`) und `queue_notify(…, mem: &dyn GuestMemory)` auf `&`. Zwei-Thread-Test mit `GuestRam` (Treiber-Thread gegen Geräte-Thread). | `VirtioDevice`-Implementierer (heute nur Testgeräte) |
| 4 | test-engineer | test-engineer | `fuzz_t0_step`, `fuzz_virtqueue_walk`: `&mut` → `&`, optional `GuestRam` statt `RamRegion`. | – |
| 5 | in WP-DBT-02/-06/-09/-12/-13 | dbt-engineer | T0-Stores über `store`/`write`, Walker-Reads atomar, T0-Atomics über `compare_exchange`/`fetch_rmw`, JIT-`addend` aus `host_range`, `WriteObserver` (SMC, `revoke_fast_writes`). | – |
| 6 | PLAT-15 | platform-engineer | `ritsu-vmm` baut `Arc<GuestRam>` aus dem Layout und verteilt es an vCPU- und Geräte-Threads. | – |
| 7 | WP-GPU-11 | gpu-engineer + architect | `GuestMemoryPort` in `ritsu-gpu-host`, Adapter in `ritsu-vmm`, A-M1-Spike. | – |

Reihenfolge: 1 → (2 ∥ 3 ∥ 4) → 5/6/7 nach Bedarf. Schritt 1 ist klein und entblockt WP-DBT-09 sowie PLAT-10.

## Konsequenzen

* **Positiv:** Ein Eigentümer und eine Wahrheit für Gast-RAM. T0- und T1-Atomics sind dieselben
  Hardware-Operationen. Die Speicherordnung für Geräte steht in der Schnittstelle statt in einer Moduldoku. Die
  GPU bekommt Aliase und ein Dirty-Log ohne neue Crate-Kante. Zero-Copy-Block-I/O wird möglich. Die gesamte
  `unsafe`-Fläche liegt in einem Modul mit benannten Invarianten.
* **Negativ / Kosten:**
  * Eigener `unsafe`-Code mit Inline-Assembly, der Spezial-Review braucht.
  * Neue Abhängigkeit `libc`.
  * Rust-Zugriffe auf Gast-RAM sind für LLVM undurchsichtig: keine Vektorisierung, keine Zusammenlegung. Für
    Bulk-Kopien bleibt `rep movsb`.
  * Jeder Write über die sichere API kostet einen zusätzlichen Relaxed-Byte-Load (Seitenflags).
  * `GuestRam` braucht Linux-Syscalls. Tests dafür laufen nur unter Linux, Miri kann sie nicht prüfen. Miri bleibt
    für `RamRegion` und die Logik außerhalb von `host_map`.
* **Folgearbeiten (Work Packages):** WP-MEM-01, WP-MEM-02 ★, PLAT-xx (virtio-Ordnung), Fuzz-Anpassung, Punkte in
  WP-DBT-02/-06/-09/-12/-13, PLAT-15, WP-GPU-11 (siehe Migrationsplan). Nach Freigabe: dbt.md §7.4 und O-DBT-1,
  platform.md §8.1 und §15 Frage 3 sowie gpu.md G-10 auf diesen ADR verweisen lassen.

## Überprüfung

Die Entscheidung ist falsch oder muss angepasst werden, wenn eine der folgenden Prüfungen scheitert:

1. **GPU-Import (A-M1):** Lässt sich ein memfd-Alias auf ANV **und** NVIDIA weder per
   `VK_EXT_external_memory_host` noch per `udmabuf` plus dma-buf-Import importieren, verliert memfd seinen
   GPU-Grund. Dann fällt Strategie (a), `alias` wird überflüssig, und die Entscheidung memfd gegen anonymes
   `mmap` wird neu bewertet. Messung: Spike in WP-GPU-11 auf dem G5 MF (nicht ausführbar in CI).
2. **Kosten des sicheren Pfads:** Microbench in `bench/`: T0-Load/Store über `GuestMemory` v2 gegen direkten
   Host-Zeigerzugriff. virtio-blk-Durchsatz mit `pread_into` gegen Kopie über `write`. Liegt der T0-Overhead über
   15 % der Befehlszeit oder bleibt Bulk unter 70 % von `memcpy`, bekommt T0 einen eigenen Fastpath über
   `host_range` (wie T1) bzw. wird der Bulk-Mechanismus in `host_map` getauscht. Die Schnittstelle bleibt.
3. **Soundness-Review (A-M3/A-M4):** Verwirft `code-reviewer` die asm-Begründung oder belegt der Ordnungs-Stresstest
   eine Umordnung nach Bulk-Kopie trotz `sfence`, wird I-2 bzw. I-7 ersetzt. Ein neuer ADR ist nur nötig, wenn
   sich dadurch die öffentliche Schnittstelle ändert.
4. **Huge Pages (A-M2):** Zeigt `smaps` im Appliance-Image keine PMD-Mappings für das memfd, und messen wir
   dadurch mehr als 10 % Host-dTLB-Misses im Fastpath, wird hugetlbfs erwogen. Dann gibt es keine 16-KiB-Aliase
   mehr, und das ist ein Zielkonflikt mit Punkt 1. Er wird per neuem ADR entschieden.
5. **Dirty-Log-Kosten:** Ist `arm_write_watch` (synchroner Shootdown) in Metal-Workloads häufiger als etwa 100/s
   pro vCPU oder dominieren Slow-Path-Writes auf `WATCH`-Seiten die Frame-Zeit, wird auf das QEMU-Modell
   umgestellt (nach dem ersten Write wieder Fastpath, Shootdown beim Ernten).
6. **Option C erneut prüfen**, falls WP-MEM-02 nach zwei Sitzungen am `unsafe`-Review hängt (CLAUDE.md §2.3):
   Dann `vm-memory` für Region-Verwaltung und Bulk übernehmen und nur CAS/Alias/Flags selbst bauen.
