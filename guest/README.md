# guest/ – Gast-Komponenten (nur ADR-0005 Variante G)

> **Dieses Verzeichnis ist leer und bleibt es, solange [ADR-0005](../docs/adr/0005-gpu-guest-path.md) (Status *Proposed*) nicht
> mit Variante G (eigener Gast-Treiber) oder dem Hybrid entschieden ist.** Auf `Proposed`-ADRs baut niemand Irreversibles
> (CLAUDE.md §3 Regel 6). Spike S-GPU-2 darf hier einen Prototyp ablegen.

## Was hier liegen würde

Nur wenn Variante G (oder der Hybrid) gewählt wird:

| Komponente | Zweck |
|---|---|
| Gast-Kext (DriverKit-Extension oder Kernel-Extension) | Übergibt den Befehlsstrom aus [`ritsu-gpu-proto`](../crates/ritsu-gpu-proto) über das Gast-Transportgerät (virtio) an den Host |
| Metal-Treiber-Bundle (`MTLDevice`-Plug-in) | Stellt `MTLCreateSystemDefaultDevice()` im Gast bereit und übersetzt Metal-Aufrufe in den eigenen Befehlsstrom |
| Installationsskripte | Signieren, Installieren, Aktivieren im Gast |

Variante P (Apples eigener Paravirt-Treiber im Gast) braucht **nichts** aus diesem Verzeichnis.

## Bau-Voraussetzungen

* **macOS-SDK** (Xcode oder Command Line Tools) und damit ein **Mac** (oder eine macOS-27-VM). Das SDK ist Apple-Eigentum und liegt **nicht** im Repo.
* Ziel-Architektur **`arm64e`** (Pointer Authentication), wie der Gast-Kernel: `-arch arm64e`. Kernel-Erweiterungen müssen zum PAC-ABI des Kernels passen;
  ein Bau mit `arm64` genügt nicht für Kernel-Code.
* Xcode-Toolchain auf einem Mac. Eine **Cross-Toolchain unter Linux** (Clang mit `-target arm64e-apple-macos` plus ein extrahiertes SDK) ist theoretisch möglich,
  aber rechtlich und praktisch heikel (SDK-Lizenz, private Header) und ausdrücklich **nicht** Teil des Plans. Wer das untersuchen will, öffnet dazu zuerst eine Frage an den Menschen.
* Nur öffentliche SDK-Header und dokumentierte Verfahren; private Metal-Treiber-Schnittstellen laufen nach ADR-0008 über eine Spezifikation in `docs/specs/`,
  geschrieben von `reverse-engineer`, nicht vom Autor des Treibers.

## SIP und AMFI im Gast

Ein eigener Kext und ein nicht von Apple signiertes Metal-Plug-in laufen nur, wenn der Gast **gelockerte Sicherheit** hat:

* **SIP** (System Integrity Protection) muss für das Laden nicht notarisierter Kernel-/Systemerweiterungen reduziert werden
  (`csrutil` in der Recovery des Gastes; konkrete Flags: Gegenstand von S-GPU-2).
* **AMFI** (Apple Mobile File Integrity) erzwingt Code-Signaturen; auf Apple-Silicon-Gästen kann sie über Boot-Argumente bzw. Boot-Policy gelockert werden.
  Auf `vmapple` läuft das über die Boot-Policy im aux-Speicher ([platform.md](../docs/subsystems/platform.md) §10.4) – Details **[Spec nötig]**.
* Reduzierte Sicherheit bedeutet: kein Zugriff auf einige Apple-Dienste, mögliche Einschränkungen bei Updates (neue macOS-Builds können den Treiber ungültig machen,
  Risiko R-5), und der Nutzer muss das bewusst akzeptieren (First-Run-Hinweis in `host-os.md` §9).

## Code-Signatur

* Entwicklung: **Ad-hoc-Signatur** (`codesign -s -`) reicht im Gast mit gelockertem AMFI.
* Verteilung: Eine Developer-ID-Signatur und Notarisierung wären nur mit Apple-Entwicklerkonto möglich; **Entscheidung offen** (Mensch). Kernel-Extensions erfordern ohnehin Nutzerfreigabe im Gast.
* Das Metal-Treiber-Bundle und der Kext werden als **Artefakte** gebaut und über das Host-Image bzw. den Import (nicht über das Repo) in den Gast gebracht.

## Regeln

1. **Nichts in diesem Verzeichnis wird in der Linux-CI gebaut.** `make check`, `cargo` und CI-Jobs ignorieren `guest/`; ein Build braucht macOS (CLAUDE.md §4). Prüfungen hier sind „nicht ausführbar in der Cloud“ und werden so gemeldet.
2. Keine Apple-Binärdateien, kein SDK-Inhalt, keine disassemblierten Fragmente (ADR-0008).
3. Jede Änderung hier geht vor dem Commit an `code-reviewer` (CLAUDE.md §2).
4. Protokolldefinitionen leben in `ritsu-gpu-proto` (Rust). Der Gast-Code darf sie nur über eine generierte C-Header-Datei verwenden, nicht per Handkopie.
