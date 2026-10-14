# Clean-Room-Spezifikationen

Nur die Rolle `reverse-engineer` (oder ein Mensch in der Analystenrolle) schreibt hier. Verfahren:
[clean-room.md](../process/clean-room.md), [ADR-0008](../adr/0008-clean-room.md).

Unterverzeichnisse nach Bereich: `platform/` (Apple-VM-Geräte, Boot), `gpu-paravirt/` (Paravirt-GPU-Protokoll),
`metallib/` (Container, AIR-Konventionen), `metal-driver/` (Schnittstellen für Variante G).

## Pflichtkopf jeder Spezifikation

```markdown
# Spec: <Gerät/Protokoll>
* Bereich: platform | gpu-paravirt | metallib | metal-driver
* macOS-Build(s): z. B. 27.0 (Build-Nummer)
* Methode: öffentliche Doku | Black-Box-Trace | statische Analyse (mit Begründung)
* Autor-Rolle: reverse-engineer
* Konfidenz: pro Abschnitt markiert als bestätigt / wahrscheinlich / Vermutung
```
