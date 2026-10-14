# Host-OS – Appliance-Linux

> Status: **Entwurf v0.1** (WP-M0-PLAT). Zuständig: `platform-engineer`. Grundlage: [ADR-0002](../adr/0002-linux-host-vulkan.md)
> (Linux + Vulkan 1.3+). Das Image-Verzeichnis ist `host-os/` (Pointer: [host-os/README.md](../../host-os/README.md)).
> Plattform/Gerätemodell: [platform.md](platform.md). Alle Aussagen über Hardware- und Treiberstände sind
> **[Annahme]**, bis sie auf dem echten G5 MF geprüft wurden (siehe WP-HOST-01 und Hardwarematrix §12).

## 1. Ziel und Leitplanken

Der Nutzer steckt einen Stick an oder bootet eine Partition und landet **ohne sichtbares Linux** im macOS-Gast
(Hackintosh-Gefühl). Dazwischen liegt ein minimales Linux, das genau eine Aufgabe hat: Hardware initialisieren, `ritsu-vmm`
im Vollbild starten, Gäste-Ein-/Ausgabe verdrahten.

* **Klein:** Nur, was der vmm braucht. Keine Desktop-Umgebung, keine Paketverwaltung im Betrieb.
* **Reproduzierbar:** Jedes Image entsteht aus einer Konfiguration in `host-os/` und einem **fixierten Paketstand**.
* **Aktualisierbar und rückrollbar:** A/B (siehe §10).
* **Sicherheit:** Secure Boot ist hier möglich (kein OpenCore mehr, §8). Der vmm läuft ohne Root.
* **Der Host verteilt keine Apple-Dateien.** Boot-ROM und IPSW/Images liefert der Nutzer (First-Run, §9).

## 2. Basis-Distribution

| Kriterium | **Arch Linux** (mkosi/mkarchiso) | **Fedora** (kiwi / bootc) | **Buildroot** | **Yocto/OpenEmbedded** |
|---|---|---|---|---|
| Aktualität Kernel/Mesa/NVIDIA | sehr gut (rolling); Mesa/NVK und neue Kernel (Ada, Alder Lake) sofort | gut (halbjährlich), Kernel/Mesa aktuell | mittel; Versionen manuell pflegen | je nach Layer, meist hinterher |
| Proprietärer NVIDIA-Treiber | Paket vorhanden (`nvidia-open`, DKMS/Binär) | über RPM Fusion (extern) | kein Paket, selbst bauen | eigenes Layer nötig |
| Mesa (ANV, NVK), PipeWire, wlroots | alles als Paket | alles als Paket | möglich, aber Paket-Aufwand und Rust/LLVM-Build für NVK schwer | möglich, hoher Aufwand |
| Image-Bau | `mkosi` (Arch-Backend) bzw. `mkarchiso` (Live-ISO) | `kiwi` / `bootc` (Container-Image-basiert, ostree) | einfach, sehr schnell, klein | mächtig, steile Lernkurve |
| A/B-Update | `systemd-sysupdate` + `systemd-repart` (Partitionen) | ostree/bootc (Atomic) | RAUC o. ä. | RAUC/SWUpdate |
| Reproduzierbarkeit | über fixierte **Arch-Archive-Schnappschüsse** (Datum) möglich, aber manuell | gut (versionierte Releases) | sehr gut | sehr gut |
| Image-Größe/Bootzeit | mittel (150–300 MB komprimierte Basis ohne GPU-Stacks) | größer | klein, schnell | klein |
| Wartungsaufwand für kleines Team | gering–mittel (Rolling-Bruch durch Schnappschuss entschärft) | mittel | hoch (alles selbst) | hoch |
| Debugging am Entwicklerrechner | Entwickler kennen es; Pakete für perf, RenderDoc, gdb | ebenfalls | schwer | schwer |

**Empfehlung: Arch Linux als Paketbasis, Image-Bau mit `mkosi`**, fixiert auf einen datierten Arch-Archive-Schnappschuss
pro Release. Gründe:

1. Wir brauchen *gleichzeitig* sehr neue Treiberstände (Mesa/ANV, NVK, Ada-GPU, Kernel für Raptor-/Alder-Lake-Details) und ein
   proprietäres NVIDIA-Paket – das liefert Arch ohne Fremd-Repos.
2. `mkosi` erzeugt ein vollständiges GPT-Disk-Image mit `systemd-repart`, signierten UKIs und `systemd-sysupdate`-A/B-Layout aus
   *einer* Konfiguration; es hat ein Arch-Backend. `mkarchiso` bleibt als Option für ein Installer-Live-ISO.
3. Das Rolling-Risiko wird durch den **Schnappschuss** entschärft: Upgrades sind bewusste Commits (Datum anheben, testen, mergen).
4. Rückfall: Wenn Größe/Bootzeit zum Problem werden oder sehr strikte Reproduzierbarkeit nötig ist, ist ein späterer Umstieg auf
   Buildroot/Yocto für das *Release*-Image möglich, weil die Anwendungsschicht (`ritsu-vmm`, systemd-Units, Kernel-Config) portabel gehalten wird.
   Das wird in M7 neu bewertet (Offene Frage 1).

## 3. Kernel

Mainline-Linux, **LTS-Zweig** (Annahme: ≥ 6.12 LTS beim Start; Entscheidung bei WP-HOST-02), eigene `.config`, kein Distro-Kernel.

| Bereich | Einstellung / Begründung |
|---|---|
| Scheduler | `PREEMPT_DYNAMIC` (Standard `voluntary`), `HZ=1000` oder `NO_HZ_IDLE`; **Intel Thread Director / Hybrid-Scheduling** (`CONFIG_SCHED_MC_PRIO`, `INTEL_HFI_THERMAL`) an. |
| CPU | `intel_pstate` (HWP), `CPU_FREQ_GOV_*`, `INTEL_TCC`, `X86_PLATFORM_DRIVERS_*`; kein KVM nötig (der DBT ist kein Hypervisor). `CONFIG_KVM=n` spart Angriffsfläche. |
| Speicher | `TRANSPARENT_HUGEPAGE` (`madvise`) und HugeTLB für Gast-RAM (weniger TLB-Misses im SoftMMU-Backend); `USERFAULTFD` (Snapshot/Restore, später); `MEMCG`, cgroups v2. |
| JIT | W^X erlaubt (`mmap` mit `PROT_EXEC`), kein `noexec`-Zwang auf anonymem Speicher; `perf` mit JIT-Dump (`perf-<pid>.map`) für Profiling. |
| GPU/Anzeige | `DRM_I915` (Iris Xe, Alder Lake). `DRM_XE` ist für ADL nur experimentell [Annahme]; `DRM_NOUVEAU` mit GSP für NVK; NVIDIA-Modul separat (`nvidia-drm.modeset=1`, `fbdev=1`). `DRM_SIMPLEDRM` als Fallback beim frühen Boot, `FB`-Konsole minimal. |
| Speicherung | `NVME_CORE`, **`VMD`** (Intel Volume Management Device; falls BIOS auf RAID/Intel RST steht – besser: im BIOS auf AHCI), `EXT4`, `F2FS`/`BTRFS` (Datenpartition), `EROFS`/`SQUASHFS` (Root, schreibgeschützt), `DM_VERITY` (optional, Integrität des Root-Images), `FS_VERITY`. |
| WLAN/BT | `IWLWIFI`+`IWLMVM` (Intel AX201, CNVi; Firmware `linux-firmware`), `BT_INTEL`, `BT_HCIBTUSB`. |
| Audio | `SND_HDA_INTEL` + `SND_HDA_CODEC_REALTEK` (ALC256). Alder Lake kann den Codec per **SOF-DSP** (`SND_SOC_SOF_*`) ansteuern, je nach Mikrofon-/Firmware-Layout: **[Annahme] zu prüfen** (WP-HOST-05). |
| Eingabe | `I2C_HID_ACPI`, `HID_MULTITOUCH`, `MFD_INTEL_LPSS_PCI`/`PINCTRL_ALDERLAKE`/`I2C_DESIGNWARE_*` (Touchpad liegt an I2C hinter Intel-LPSS), `INPUT_EVDEV`, `HID_GENERIC`. |
| Netz | `NF_TABLES`, `TUN`, `BRIDGE`, `VETH` (Gast-NAT), `NET_SCH_FQ_CODEL`. |
| Sicherheit | `SECURITY_LOCKDOWN_LSM`, `MODULE_SIG` (signierte Module), `SECURITY_YAMA`, seccomp, `USER_NS` für Sandboxing. |
| Diagnose | `PERF_EVENTS`, `KPROBES` (aus in Release), `DEBUG_INFO_BTF`. |
| Nicht nötig | Bluetooth-Audio-Profile im Kernel (userland), Virtualisierung, Wireless-Treiber ≠ Intel, exotische Dateisysteme. |

## 4. GPU-Stack

### 4.1 Treiberwahl für die RTX 4050 Laptop (AD107, Ada)

| Option | Vorteile | Nachteile / Risiko |
|---|---|---|
| **NVIDIA proprietär mit Open-Kernel-Modulen (`nvidia-open`)** | Vollständiger Vulkan-1.3+-Funktionsumfang (Descriptor Buffer, Mesh Shader, Ray Query, Timeline-Semaphore) und ausgereifte Leistung/Energieverwaltung; GSP-Firmware wird vom Treiber selbst geladen. Für Turing und neuer sind die offenen Module der vorgesehene Weg [Annahme: für Ada Standard]. | Geschlossener Userspace (nicht patchbar), Wayland-Besonderheiten (explizite Synchronisation, GBM/EGLStreams-Historie), Version an Kernel gekoppelt (DKMS-Pflege), Lizenz/Verteilung klären (**Redistribution des Binärtreibers im Image prüfen**). |
| **NVK (Mesa) auf nouveau + GSP** | Freier Stack, bessere Debugbarkeit, mit Mesa-Update mitgeliefert, kein DKMS. | Funktionsumfang/Leistung hinken je Mesa-Stand hinterher; **Ray Tracing und Mesh Shader-Stand ist zu prüfen** [Annahme: lückenhaft]; Reclocking/Energie weniger ausgereift. |

**Entscheidung (vorläufig):** Release-Standard = **proprietärer Treiber mit offenen Kernel-Modulen**; **NVK als zweites Backend** für CI-nahe Tests,
Debugging und als Rückfall. Der GPU-Host (`ritsu-gpu-host`) meldet die Fähigkeitsmatrix je Treiber, so dass beide Stacks ehrlich nutzbar sind
(ARCHITECTURE.md §5.2). Messung statt Annahme: WP-HOST-03 vergleicht beide mit `vulkaninfo` und den Features aus `gpu.md`.

### 4.2 Iris Xe (Xe-LP, Alder Lake) – Mesa ANV

* Treiber: Mesa `ANV` (Vulkan), Kernel `i915`. Vulkan 1.3 ist verfügbar [Annahme]; Mesh Shader/Ray Query auf Xe-LP fehlen (Hardware) → werden vom
  Backend als nicht verfügbar gemeldet bzw. emuliert (siehe `gpu.md`).
* Die Iris Xe hängt am **internen Display** (eDP) – sie ist *Scanout-GPU*, auch wenn die RTX 4050 rendert.

### 4.3 PRIME-Render-Offload auf einem Notebook ohne MUX

Die Aufgabe: Render auf der **dGPU (NVIDIA)**, Scanout auf der **iGPU (Intel)**, denn das Panel hängt nur an der Iris Xe.

* Pfad: dGPU rendert in ein Bild, exportiert es als **dma-buf** (linear oder mit Modifier), die iGPU importiert (Vulkan
  `VK_EXT_external_memory_dma_buf` / `VK_EXT_image_drm_format_modifier`) und stellt es dar oder kopiert es in ein Scanout-Bild.
  Kosten: eine zusätzliche Kopie über PCIe (1080p60 ≈ 0,5 GB/s, kein Engpass; 4K höher).
* Mit Wayland-Compositor erledigt der Compositor den Multi-GPU-Teil (wlroots Multi-GPU, `WLR_DRM_DEVICES`); im Direktpfad (§5) macht es `ritsu-gpu-host` selbst.
* **Ports:** Ob HDMI/USB-C-DP des G5 MF an der dGPU oder iGPU hängen, ist **[Annahme: dGPU, zu prüfen]** (WP-HOST-04). Es bestimmt, welche Ausgabe ohne Kopie läuft.
* Stromsparen: Im Leerlauf (Gast ohne GPU-Last) muss die dGPU in D3cold/`RTD3` gehen (Akkulaufzeit). Eigenes Messziel in WP-HOST-10.

## 5. Anzeige: Kiosk-Compositor vs. direktes KMS

| Kriterium | **Kiosk-Wayland-Compositor (z. B. cage, wlroots)** | **Direkt DRM/KMS (`VK_KHR_display` / eigener Atomic-Modeset)** |
|---|---|---|
| Aufwand | gering: Wayland-Client mit Vulkan-WSI | hoch: Modesetting, Hotplug, EDID, Skalierung, Mehr-Monitor selbst |
| Latenz | +1 Compositor-Frame möglich (Direct-Scanout hilft) | niedrigste (eigene Frame-Planung, Q-PERF-4 < 8 ms) |
| Multi-GPU/PRIME | Compositor-Aufgabe, NVIDIA-Pfad empfindlich (Version/Explicit Sync) | in unserer Hand, aber eigener Code |
| Eingabe | libinput im Compositor; Fokus/Pointer-Capture über Wayland-Protokolle | eigene libinput-Integration |
| VRR/HDR/Skalierung | vom Compositor abhängig | volle Kontrolle |
| Fehlersuche | Compositor als zusätzliche Fehlerquelle | weniger bewegliche Teile zur Laufzeit |
| Fremdkode | cage + wlroots + libseat | nur Kernel-APIs (+ libinput, libseat) |

**Empfehlung:** Phase 1 (M0–M4) **cage** mit `ritsu-vmm` als einzigem Fenster (schnell zum ersten Bild, Wayland ist auf NVIDIA mit neueren Treibern tragfähig).
Phase 2 ab M5 ein **direktes KMS-Präsentationsmodul** hinter einer `Presenter`-Schnittstelle in `ritsu-gpu-host` (Zuständigkeit gpu-engineer) für minimale Latenz und
VRR. Der Wechsel darf keine Auswirkung auf die Plattform haben; deshalb steht die Präsentation hinter einem Trait.

## 6. Eingabe, Audio, Netzwerk

### 6.1 Eingabe
`libinput` (über den Compositor, später direkt) → normalisierte Ereignisse → vmm-Eingabegerät (USB-HID, Apple-Gerät oder virtio-input; siehe platform.md §9.4).
Touchpad des G5 MF (I2C-HID): Linux liefert Multitouch-Kontakte; Zeigerbeschleunigung/Tap-to-click kommen aus libinput und werden **vor** der Gast-Übergabe angewendet,
solange der Gast kein echtes Trackpad-Gerät hat. Tastenbelegung: Host reicht Scancodes/HID-Usages durch, ohne Layout-Übersetzung (macOS interpretiert selbst).
Spezialtasten (Helligkeit, Lautstärke, Fn): Der Host behandelt Helligkeit des internen Panels selbst (Backlight-Sysfs), Lautstärke über PipeWire, und gibt sie nur auf Wunsch an den Gast.

### 6.2 Audio
PipeWire (mit WirePlumber) als Host-Audioserver, `ritsu-vmm` ist ein PipeWire-Client für virtio-sound. Ausgang: Lautsprecher/Kopfhörer (ALC256), HDMI-Audio.
Latenz-Ziel: < 40 ms Ende-zu-Ende zunächst, Puffergröße einstellbar. Mikrofon erst später (Datenschutzdialog im First-Run).

### 6.3 Netzwerk
* Host: **NetworkManager** oder `iwd`+`systemd-networkd`. Für ein Appliance-Image ohne Desktop ist `iwd` + `systemd-networkd` kleiner; NetworkManager hat bessere
  Hotspot-/Captive-Portal-/VPN-Werkzeuge. **Vorschlag:** `iwd` + `systemd-networkd` + `systemd-resolved`, WLAN-Einrichtung im First-Run (Offene Frage 4).
* WLAN: Intel AX201 (CNVi) per `iwlwifi`; Firmware-Datei im Image (`linux-firmware`).
* Gast: `virtio-net` an `tap`. Standard **NAT** (nftables-Masquerade, eigenes `10.x`-Subnetz, DHCP/DNS-Weiterleitung vom Host), optional **Bridge** (nur bei Ethernet, da WLAN-Clients
  keine Bridge mit fremden MACs erlauben). Gast-MAC deterministisch aus der Maschinen-UUID.
* Nur erlaubte Ports hinein (Portweiterleitung wie in der QEMU-Doku: z. B. Host 2222 → Gast 22 per Konfiguration).

## 7. Speicherlayout

```
GPT auf der NVMe des Nutzers (Beispiel, freier Platz neben Windows):
  ESP / XBOOTLDR   (vorhandene Windows-ESP bleibt unberührt)   -> eigener XBOOTLDR 1 GiB: systemd-boot, UKI-A, UKI-B
  ritsu-root-A     EROFS/SquashFS, schreibgeschützt (dm-verity optional)    ~2 GiB
  ritsu-root-B     Zwilling für A/B-Update                                  ~2 GiB
  ritsu-data       ext4/f2fs: Gast-Images, aux, Übersetzungs-Cache, Konfiguration, Logs   Rest (≥ 128 GiB empfohlen)
```

* **Disk-Image als Datei vs. rohe Partition:** v1 = **Datei** (sparse raw) auf `ritsu-data`: Sicherung, Reflink-Snapshots, Backup durch Kopieren, flexible Größe.
  Rohe Partition/LV nur als Option für maximalen Durchsatz (gemessen, siehe PLAT-10). Der vmm öffnet Images selbst, der Gast liefert nie Pfade.
* **Dual-Boot mit Windows:** Windows-ESP wird nicht verändert (nur ein Eintrag in systemd-boot, der den Windows-Boot-Manager chainloadet); Ritsu nutzt einen **eigenen XBOOTLDR**,
  weil Windows-ESPs oft nur 100–260 MiB groß sind (zwei UKIs passen nicht). **BitLocker:** vor Partitionsänderungen aussetzen/Wiederherstellungsschlüssel sichern – der Wizard
  warnt, ändert Windows-Partitionen nur nach ausdrücklicher Zustimmung und nur durch Verkleinern (kein Formatieren).
* **Verschlüsselung:** Datenpartition optional mit LUKS2 (TPM-gebunden oder Passphrase); der Gast-FileVault ist unabhängig.

## 8. Bootkette

* **Bootloader:** `systemd-boot` und **Unified Kernel Images (UKI)** (Kernel + initrd + Kommandozeile in einer signierten EFI-Datei). Boot-Counting + Automatic Boot Assessment liefert das Rollback der A/B-Updates (§10).
* **Secure Boot ist jetzt möglich:** Beim Hackintosh musste OpenCore Secure Boot aushebeln; hier ist nur Linux-Standard nötig. Zwei Wege: (a) Projekt-Schlüssel + `shim`-Signatur (Microsoft-signierter
  shim, MOK-Einschreibung durch den Nutzer – komplex, aber ohne BIOS-Eingriff), (b) Nutzer trägt den Ritsu-Schlüssel selbst in die Firmware ein (`sbctl`, Setup-Mode). **Empfehlung:** (b) als Standard (einfacher, volle Kontrolle), (a) als späteres Komfort-WP.
* **Initrd:** minimal (systemd-initrd), Aufgaben: Datenpartition einhängen/entschlüsseln, A/B-Root auswählen, verity prüfen.
* **Start des vmm:** systemd-Ziel `ritsu.target` → Dienst `ritsu-session.service` (cage + `ritsu-vmm`) unter einem unprivilegierten Benutzer `ritsu`, `seatd`/`logind` für DRM-/Eingabegeräte.
  Ziel: Einschalten → vmm in **< 10 s** (Q-PERF-2 betrifft den Gast-Boot danach).
* **TPM:** optional Messung der UKI in PCR 11/7; Bindung des LUKS-Schlüssels. Keine Pflicht.

## 9. First-Run-Assistent

Ein kleines Programm `ritsu-setup` (TUI oder minimale GUI im selben Compositor, Entscheidung offen; WP-HOST-08). Schritte:

1. **Hardwareprüfung:** CPU-Features (AVX2, BMI2, FMA; **kein AVX-512 erwartet**), GPU-Stack (`vulkaninfo`-Zusammenfassung), RAM, Datenträger. Klarer Bericht, was fehlt.
2. **Speicherplatz einrichten** (Datenpartition anlegen/auswählen, §7), Warnungen für Windows/BitLocker.
3. **Netzwerk:** WLAN verbinden (nötig für Updates, nicht für Boot).
4. **Apple-Komponenten beschaffen (vom Nutzer):**
   * **Boot-ROM** (`AVPBooter…`): Datei vom USB-Stick/aus einem Mac importieren; Ritsu verteilt sie nie. Prüfsumme anzeigen (wir veröffentlichen **Hashes**, nie Inhalte).
   * **IPSW oder fertiges Image:** (a) Wenn S-BOOT-2 erfolgreich ist: IPSW laden/importieren, Host-seitiger Restore. (b) Rückfall: **auf dem eigenen Mac erzeugtes Disk- und aux-Image samt Maschinen-UUID** importieren (platform.md §10.3).
5. **Erster Start**, Cache-Aufwärmen (Übersetzungs-Cache wird beim ersten Boot gefüllt – Hinweis auf lange Startzeit).
6. **Rechtlicher Hinweis:** macOS-Lizenz erlaubt macOS nur auf Apple-Hardware. Risiko beim Nutzer (ARCHITECTURE.md R-1). Keine Rechtsberatung. Der Text wird vom Menschen freigegeben (Offene Frage 6).

## 10. Updates (A/B)

* `systemd-sysupdate`: signierte Update-Artefakte (Root-Image + UKI) werden in die inaktive Seite geschrieben, UKI mit Boot-Zähler; erst nach erfolgreichem Start (`systemd-bless-boot`) wird die neue Seite dauerhaft.
  Schlägt der Start n-mal fehl, bootet die alte Seite.
* **Updatekanal:** HTTPS mit Signaturprüfung (Projekt-Schlüssel; Schlüsselrotation planen), Manifest mit Mindestversion.
* **Daten bleiben:** Gast-Images, aux, Einstellungen und der Übersetzungs-Cache liegen auf `ritsu-data` und werden nicht angefasst. Das Cache-Format ist versioniert; ein inkompatibles Update verwirft den Cache, nicht die Images.
* **Gast-macOS-Updates** sind eine andere Ebene (innerhalb des Gastes; siehe Kompatibilitätsmatrix `docs/compat/`, R-5).

## 11. Übersetzungs-Cache, Energie und Hybrid-CPU

* **Speicherort:** `/var/lib/ritsu/tcache/` auf `ritsu-data`, Schlüssel (macOS-Build, Seiten-Hash). Quote einstellbar (Vorschlag 20–40 GiB), LRU-Verdrängung.
  **Der Cache enthält ausführbaren Host-Code**: Jeder Eintrag bekommt eine Prüfsumme **und** ein Siegel mit hostlokalem Schlüssel (Datei 0600, in einem Keyring), damit ein manipuliertes Cache-Verzeichnis
  keinen Code einschleust. Beim Laden nur mit gültigem Siegel, sonst neu übersetzen. Die Cache-Dateien werden `mmap`-geladen, nicht blind ausgeführt.
* **Hybrid-CPU (i5-12500H: 4 P-Kerne mit SMT = 8 Threads, 8 E-Kerne):**
  * Topologie aus `/sys/devices/system/cpu/cpu*/topology` und `/sys/devices/cpu_core/cpus` bzw. `/sys/devices/cpu_atom/cpus` ermitteln (nicht fest verdrahten).
  * **vCPU-Threads:** je ein Thread pro **physischem P-Kern** (SMT-Geschwister frei lassen), `sched_setaffinity`; Standard-vCPU-Zahl = Zahl der P-Kerne.
  * **JIT-Compiler-/AOT-Threads, I/O, Shader-Compiler:** E-Kerne (niedrigere Priorität, `SCHED_BATCH`/nice), damit sie die vCPUs nicht stören.
  * Optional `isolcpus`/`nohz_full` für P-Kerne (misst man: WP-HOST-10), CPU-`performance`-EPP bei Netzteil, `balance_performance` im Akkubetrieb.
* **Energie/Thermik:** `thermald`, Intel DPTF/`intel_pmc`; Lüfterkurven kommen von der Firmware (der G5 MF hat keinen frei dokumentierten EC-Zugriff – **[Annahme]**). Ziel: kein Thermal-Throttling in den ersten 10 Minuten Volllast messen.
  Akku: Anzeige durch den Host, Gast bekommt (später) virtuelle Batterie-Informationen (Spec nötig).
* **Suspend/Resume:** Host-S0ix/S3 nur, wenn der Gast vorher anhält (Pause + `CNTVOFF`-Korrektur, platform.md §6). Standard in v1: nicht unterstützt (klare Meldung).

## 12. Hardwarematrix – Gigabyte G5 MF

| Komponente | Hardware | Linux-Treiber / Stack | Erwarteter Stand | Prüfen in |
|---|---|---|---|---|
| CPU | Core i5-12500H (4P+8E, AVX2, kein AVX-512) | `intel_pstate`, ITD-Scheduler | gut | WP-HOST-02 |
| iGPU | Iris Xe (Alder Lake GT2, am eDP) | `i915` + Mesa ANV | gut [Annahme] | WP-HOST-03 |
| dGPU | RTX 4050 Laptop (AD107), kein MUX | `nvidia-open` / NVK(nouveau+GSP) | gut/mittel [Annahme] | WP-HOST-03/04 |
| Display | internes Panel an iGPU; Ports (HDMI, USB-C DP) vermutlich an dGPU [Annahme] | KMS, PRIME | zu prüfen | WP-HOST-04 |
| WLAN/BT | Intel AX201 (CNVi) | `iwlwifi`, `btintel` | gut | WP-HOST-05 |
| Audio | Realtek ALC256 | `snd_hda_intel` (evtl. SOF) | gut; Mikrofon zu prüfen | WP-HOST-05 |
| Touchpad | I2C-HID | `i2c_hid_acpi`, `hid-multitouch`, Intel-LPSS | gut | WP-HOST-05 |
| Tastatur | PS/2-Emulation (i8042) | `atkbd` | gut; Zusatztasten zu prüfen | WP-HOST-05 |
| Speicher | NVMe (evtl. hinter Intel VMD) | `nvme`, `vmd` | gut; BIOS-Einstellung dokumentieren | WP-HOST-02 |
| Ethernet | typischerweise Realtek-GbE [Annahme] | `r8169` | gut | WP-HOST-05 |
| Kamera/Fingerabdruck/Sensoren | – | uvc / nicht benötigt | nicht im Umfang | – |
| Firmware | UEFI, Secure Boot konfigurierbar | `sbctl` | zu prüfen (Setup-Mode) | WP-HOST-06 |

Offen: genaue Chip-IDs (Ethernet, Touchpad-Controller) und BIOS-Version; Messung per `lspci`/`dmesg` auf dem Gerät. Unter `legacy/g5mf-sequoia/` liegt der alte Hackintosh (eingefroren, nicht verändern). Er darf als Hinweisquelle gelesen werden (z. B. welche Kexts für Ethernet/Audio nötig waren, was auf die Chips schließen lässt).

## 13. Work Packages

| WP | Titel | Dep | Risiko | Abnahme |
|---|---|---|---|---|
| **HOST-01** | Image-Bau-Pipeline: `mkosi`-Konfiguration, Arch-Schnappschuss, bootbares Image mit Shell | – | niedrig | Image bootet in QEMU/OVMF und auf dem G5 MF bis zur Shell; Build reproduzierbar (Hash gleich bei gleichem Schnappschuss) |
| **HOST-02** | Kernel-Konfiguration + Hybrid-Scheduling | HOST-01 | ★ | `.config` im Repo; alle Geräte der Matrix erkannt; ITD-Scheduler aktiv (Messung: Thread auf P-Kern) |
| **HOST-03** | GPU-Stack: Vulkan auf beiden GPUs (nvidia-open **und** NVK) | HOST-02 | ★★ | `vulkaninfo` für ANV und NVIDIA, Feature-Tabelle (Descriptor Buffer, Timeline, Mesh Shader, Ray Query) in `docs/research/` |
| **HOST-04** | **Spike PRIME-Präsentation:** Render auf dGPU, Anzeige am internen Panel (cage) | HOST-03 | ★★★ | Vulkan-Testbild (`vkcube`-artig, Eigenbau in `bench/`) mit stabilen 60 fps auf dem Panel, Messung der Kopierkosten |
| **HOST-05** | Eingabe, Audio, Netz-Treiber der Matrix (WLAN, Touchpad, Audio, Ethernet) | HOST-02 | ★ | Alle Geräte funktionieren in der Host-Shell; Kurzprotokoll |
| **HOST-06** | UKI + `systemd-boot` + Secure Boot (`sbctl`) | HOST-01 | ★★ | Bootet mit Secure Boot an; manipuliertes UKI wird abgelehnt |
| **HOST-07** | Partitionierung/Installer, XBOOTLDR, Windows-Dual-Boot | HOST-06 | ★★ | Auf Testgerät mit Windows: beide Systeme booten; Windows-ESP unverändert (Hash) |
| **HOST-08** | First-Run-Assistent `ritsu-setup` (Import von ROM/Images, Hardwarebericht) | HOST-07, PLAT-18 | ★ | Durchlauf auf Testgerät, Fehlerfälle (falsche Datei, zu wenig Platz) behandelt |
| **HOST-09** | A/B-Updates mit `systemd-sysupdate` und Boot-Counting | HOST-06/07 | ★★ | Update + absichtlich kaputtes Update → automatischer Rollback |
| **HOST-10** | Energie und CPU-Pinning-Dienst (Topologie, Affinität, EPP, dGPU-Ruhe) | HOST-02, PLAT-15 | ★ | vCPUs auf P-Kernen (`taskset`-Check), Akkulaufzeit-Leerlaufmessung |
| **HOST-11** | Direkt-KMS-Präsentation (gemeinsam mit gpu-engineer) | HOST-04 | ★★ | Latenz < 8 ms im Messaufbau, VRR optional |
| **HOST-12** | CI-Image-Build + Smoke-Test in QEMU/OVMF | HOST-01 | niedrig | Pipeline erzeugt Image bei jedem Release-Tag |
| **HOST-13** | Cache-Siegel und Quotenverwaltung für den Übersetzungs-Cache | DBT-Cache-Format | ★ | Manipulierter Cache-Eintrag wird abgelehnt (Test) |

## 14. Offene Fragen

1. **Basis dauerhaft Arch/mkosi oder später Buildroot/Yocto für Releases?** (Neubewertung in M7.)
2. **Verteilung des proprietären NVIDIA-Treibers** im Image: Lizenzfragen (Redistribution) und DKMS-Pflege klären; alternativ Download beim First-Run.
3. **dGPU-Stack-Standard:** proprietär oder NVK – entscheidet die Messung in HOST-03/04, und die Features, die `gpu.md` verlangt.
4. **Netzwerkstack:** `iwd`+`networkd` oder NetworkManager.
5. **Secure Boot:** Nutzer-Schlüssel (`sbctl`) reicht für v1, oder wird ein `shim`-Pfad erwartet?
6. **Rechtstexte und Umgang mit Apple-Komponenten** (First-Run): vom Menschen freizugeben.
7. **Suspend/Resume** in v1 nicht unterstützt – akzeptabel?
8. **Grafischer First-Run** oder reine Text-UI?
9. **Anschlüsse des G5 MF** (HDMI/USB-C an dGPU oder iGPU) – nur am Gerät messbar.
10. **Kernel:** LTS-Version und Xe- vs. i915-Treiber für Alder Lake.
11. Verfügbarkeit eines **Testgeräts** für Hardware-WPs (die Cloud-Umgebung hat weder GPU noch UEFI-Hardware): HOST-03..11 sind dort **nicht ausführbar**.
