# Architecture Decision Records

Jede Entscheidung, die mehr als ein Crate betrifft, schwer rückgängig zu machen ist oder eine öffentliche
Schnittstelle festlegt, bekommt einen ADR. Vorlage: [0000-template.md](0000-template.md). Neue ADRs mit dem Skill
`/adr` anlegen.

Regeln:

* ADRs sind **unveränderlich**, sobald sie `Accepted` sind. Änderungen laufen über einen neuen ADR mit
  `Supersedes`.
* `Proposed` heißt: Darauf darf noch nichts Irreversibles gebaut werden.
* Ein Mensch gibt ADRs frei. Agenten schlagen nur vor.

| ADR | Titel | Status |
|---|---|---|
| [0001](0001-full-system-translation.md) | macOS 27 wird per Full-System-Translation ausgeführt, nicht nativ | Accepted |
| [0002](0002-linux-host-vulkan.md) | Host-OS ist Linux, Host-Grafik-API ist Vulkan 1.3+ | Accepted |
| [0003](0003-languages.md) | Rust für den Host, C++20 für den Shader-Compiler, C/ObjC nur im Gast | Accepted |
| [0004](0004-guest-platform-vmapple.md) | Gast-Plattform ist die Apple-VM-Plattform (vmapple), kein echtes Apple-SoC | Accepted |
| [0005](0005-gpu-guest-path.md) | GPU-Gastpfad: Paravirt (P) oder eigener Treiber (G) | **Proposed**, wartet auf S-GPU-1/2 |
| [0006](0006-dbt-tiered-design.md) | DBT ist mehrstufig (Interpreter, Basis-JIT, Cranelift-JIT) mit persistentem Cache | Accepted |
| [0007](0007-shader-translation-on-host.md) | Shader werden auf dem Host von AIR nach SPIR-V übersetzt | Accepted |
| [0008](0008-clean-room.md) | Reverse Engineering nur im Clean-Room-Verfahren | Accepted |
| [0009](0009-shared-guest-ram.md) | Gast-RAM wird ein geteiltes memfd-Mapping (`GuestRam`) mit `&self`-Zugriff; `unsafe` nur in `ritsu_mem::host_map` | **Proposed** |
