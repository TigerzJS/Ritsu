# host-os

Appliance-Linux-Image, das direkt in `ritsu-vmm` bootet (ADR-0002). Dieses Verzeichnis enthält noch **keinen Image-Code**;
der Entwurf (Basisdistribution, Kernel, GPU-Stack, Anzeige, Bootkette, A/B-Updates, WP-HOST-01…13) steht in
[docs/subsystems/host-os.md](../docs/subsystems/host-os.md).

Geplanter Inhalt (WP-HOST-01 ff.): `mkosi`-Konfiguration mit fixiertem Arch-Archive-Schnappschuss, Kernel-`.config`,
systemd-Units (`ritsu-session.service`), `systemd-repart`-/`sysupdate`-Definitionen, First-Run-Assistent.

Nicht hier: Apple-Binärdateien (Boot-ROM, IPSW, Images) – die liefert der Nutzer, sie werden nie ins Repo oder ins Image aufgenommen.
