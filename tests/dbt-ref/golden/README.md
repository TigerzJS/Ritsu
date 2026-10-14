# golden/ – aufgezeichnete Referenzergebnisse

| Datei | Orakel | Seed | Vektoren | Erzeugung |
|---|---|---|---|---|
| `m0.golden` | siehe `cargo run --manifest-path tests/dbt-ref/Cargo.toml -- info tests/dbt-ref/golden/m0.golden` (Zeile `oracle`) | `0x5249545355303031` | 118 047 | `make dbt-ref-record dbt-ref-update` |

* **Herkunft:** vollständig eigene, generierte Daten (Zufallsvektoren aus `tests/dbt-ref/src/generator.rs`, Ergebnisse von
  `ritsu-ref-runner`). Keine Apple-Dateien, keine fremden Testsuiten (ADR-0008). Lizenz wie das Repository.
* **Inhalt:** nur die Ergebnisse als Delta gegen den Anfangszustand; Wörter und Anfangszustände werden aus dem Seed neu
  erzeugt (Format: `tests/dbt-ref/README.md`). Der Kopf nennt Orakel, Host-CPU, Kernel, Commit, qemu- und
  clang-Version der Aufnahme.
* **Status:** `oracle = qemu` bedeutet *vorläufig*: erzeugt auf einem Rechner ohne arm64-Hardware. Die Hardware-Aufnahme
  liefert der CI-Job `dbt-reference` als Artefakt `dbt-ref-golden-<sha>`; sie wird mit
  `make dbt-ref-update DBT_REF_NEW=<datei>` eingecheckt und ersetzt dann diese Datei. Eine Hardware-Aufnahme wird nie
  durch eine qemu-Aufnahme ersetzt.
* **Größe halten:** < 2 MB je Datei. Mehr Vektoren oder Klassen: zusätzliche Datei statt Aufblähen.
