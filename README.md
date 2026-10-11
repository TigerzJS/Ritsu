# Hackintosh: Gigabyte G5 MF (i5-12500H + RTX 4050 Laptop)

OpenCore-EFI für **macOS 15 Sequoia** auf dem Gigabyte G5 MF, gebaut mit
**[1401](https://github.com/nullmoth/1401)** von NullMoth Systems. Dazu kommt der neue
**[NullMoth NVIDIA-Treiber](https://github.com/nullmoth/nvidia-macos-driver)** (v1.13.0), der Turing-und-neuere
GeForce-Karten (also auch die RTX 4050) unter macOS mit Metal 3 betreibt.

| | |
|---|---|
| Gerät | Gigabyte G5 MF (Alder Lake-H, Intel Iris Xe + NVIDIA GeForce RTX 4050 Laptop, 10DE-28A1) |
| macOS | 15 Sequoia (der NullMoth-Treiber ist nur für 15 gebaut, **kein** macOS 26 Tahoe) |
| OpenCore | 1.0.9 (DEBUG-Build, schreibt Boot-Logs auf den Stick) |
| SMBIOS | MacBookPro16,2 (das wählt 1401 für Alder-Lake-Laptops) |
| NVIDIA-Treiber | NullMoth 1.13.0 + 1401-Mac-App 1.13.0 |

> **Bitte zuerst lesen: was realistisch ist.**
>
> * Der NullMoth-Treiber ist **experimentell**. Physisch validiert ist nur die RTX 5060 (Desktop). Für RTX-4050-,
>   RTX-4060- und RTX-3050-Laptops gibt es Nutzerberichte mit funktionierendem Metal-Desktop, aber keine Garantie.
> * Der G5 MF hat **keinen MUX-Schalter**: Der eingebaute Bildschirm hängt an der **Intel Iris Xe**. Für die Iris Xe
>   (12. Generation) gibt es **keinen macOS-Treiber**. Folge:
>   * Der **Laptop-Bildschirm zeigt macOS ohne Beschleunigung** (Firmware-Framebuffer, keine Helligkeitsregelung).
>   * Die **RTX 4050 läuft als zweite Metal-GPU** (wie bei den MacBook Pros mit zwei GPUs). Apps, die die schnelle GPU
>     wollen, rechnen auf der RTX, und macOS kopiert die Bilder auf den Bildschirm. Das funktioniert ab NullMoth 1.12.
>   * **Volle Beschleunigung gibt es auf einem externen Monitor** an einem Anschluss, der mit der NVIDIA-GPU verdrahtet
>     ist. Beim G5 sind das normalerweise **HDMI** und **Mini-DisplayPort**. Bitte selbst testen.
> * Diese EFI wurde **ohne ACPI-Dump von deinem Gerät** gebaut (siehe [Wie diese EFI entstanden ist](#wie-diese-efi-entstanden-ist)).
>   Die SSDTs sind so geschrieben, dass sie bei abweichenden ACPI-Namen ohne Schaden nicht greifen. Wenn etwas nicht
>   läuft, bau die EFI mit der 1401-Windows-App auf deinem Laptop neu (Plan B unten).

---

## Was funktioniert (erwartet)

| Komponente | Lösung | Status |
|---|---|---|
| CPU (4 P- + 8 E-Kerne) | Cpuid-Spoof, `ProvideCurrentCpuInfo`, CpuTopologyRebuild, SSDT-PLUG-ALT | ✅ |
| RTX 4050 (Metal 3) | NullMoth-Treiber, `acpi-wake-type` = 1 hält die dGPU wach | ⚠️ experimentell, zweite GPU / externer Monitor |
| Interner Bildschirm (Iris Xe) | Firmware-Framebuffer (IONDRVSupport bleibt an) | ⚠️ ohne Beschleunigung, keine Helligkeit |
| WLAN Intel AX201/AX211 | itlwm + **HeliPort** (`Extras/HeliPort-1.5.0.dmg`) | ✅ (kein AirDrop) |
| Bluetooth Intel | IntelBluetoothFirmware + IntelBTPatcher + BlueToolFixup | ✅ |
| LAN Realtek RTL8111 | RealtekRTL8111 | ✅ |
| Audio Realtek | AppleALC, `alcid=14` | ⚠️ Layout ggf. anpassen (siehe unten) |
| Tastatur / Touchpad | VoodooPS2Controller / VoodooI2C + VoodooI2CHID (Polling) | ✅ |
| Akku / Sensoren | VirtualSMC, SMCBatteryManager, ECEnabler | ✅ |
| USB | USBToolBox + UTBDefault + XHCI-unsupported | ✅ (später mit der 1401-Mac-App mappen) |
| NVMe | NVMeFix | ✅ (kein Samsung PM981/PM991/Micron 2200S) |
| HDMI-/DP-Audio der NVIDIA | – | ❌ (bei NullMoth noch offen) |
| Ruhezustand, AirDrop, Sidecar, DRM (Netflix/Apple TV+) | – | ❌ / unzuverlässig |

---

## Inhalt dieses Repos

```
EFI/                          OpenCore-EFI, kommt auf den USB-Stick
  OC/config.plist             Startzustand (Installer + erster Start, ohne NVIDIA-Treiber)
Extras/
  config-nach-treiber.plist   config für NACH der Treiberinstallation (nur für die manuelle Installation)
  NullMoth/1401-Mac-1.13.0.zip   NullMoth-Installer für macOS (installiert den Treiber, ändert die config)
  NullMoth/LICENSE-NullMoth.txt  Lizenz (PolyForm Noncommercial 1.0.0)
  HeliPort-1.5.0.dmg          WLAN-Programm für itlwm
  ACPI-Quellen/*.dsl          Quelltext aller SSDTs
  hardware-annahme-G5MF.json  Hardware-Report, mit dem 1401 gebaut hat
Tools/
  smbios-erzeugen.ps1 / .sh   eigene Seriennummer/MLB/UUID/ROM eintragen (PFLICHT)
  usb-stick-erstellen.ps1 / .sh  Stick befüllen: EFI + macOS-Recovery + NullMoth-Treiber
  nullmoth-treiber-laden.ps1 / .sh  lädt nullmoth-nvidia-1.13.0.tar.gz (SHA-256 geprüft)
  macserial/, macrecovery/    Werkzeuge aus OpenCorePkg 1.0.9
```

Das Treiberpaket (270 MB) liegt nicht im Repo, weil GitHub nur Dateien bis 100 MB erlaubt. Die Skripte laden es
von NullMoths GitHub-Release und prüfen die SHA-256
`994970b1bba53e3ae8d7f500ef27b25cae8bc6818abc112d35230a58e6882fdf`.

---

## Anleitung

### 1. Vorbereiten (unter Windows)

* USB-Stick mit **mindestens 16 GB**, der gelöscht werden darf.
* **Python 3** für den Recovery-Download (Microsoft Store → „Python 3.12“).
* **Backup** deiner Daten. Plane, wo macOS hinkommt: am einfachsten eine **eigene NVMe-SSD** (falls dein G5 MF einen
  freien zweiten M.2-Slot hat). Neben Windows auf derselben SSD geht auch, dafür vorher in der Datenträgerverwaltung Platz freimachen
  (mindestens 100 GB).
* In **Windows** BitLocker deaktivieren oder den Wiederherstellungsschlüssel notieren (BIOS-Änderungen können ihn abfragen).

### 2. USB-Stick erstellen

1. Repo herunterladen (Code → Download ZIP) und entpacken.
2. Stick als **GPT + FAT32** formatieren, z. B. mit [Rufus](https://rufus.ie): Startart „Nicht startfähig“,
   Partitionsschema GPT, Dateisystem FAT32.
3. PowerShell im entpackten Ordner öffnen und ausführen (E: = dein Stick):

   ```powershell
   powershell -ExecutionPolicy Bypass -File Tools\usb-stick-erstellen.ps1 E:
   ```

   Das Skript
   * erzeugt beim ersten Lauf **deine eigene Seriennummer** (`Tools\smbios-erzeugen.ps1`). Die Werte stehen danach in
     `EFI\OC\config.plist`. Bitte **nicht** wieder ins öffentliche Repo hochladen.
   * kopiert `EFI` auf den Stick,
   * lädt die **macOS-15-Recovery** direkt von Apple nach `com.apple.recovery.boot`,
   * legt den **NullMoth-Treiber**, die **1401-Mac-App** und `config-nach-treiber.plist` nach `NullMoth\`.

   Wenn der Recovery-Download bei 1 MB abbricht, blockiert meist der Web-Schutz des Virenscanners Apples Server.
   Dann den Web-Schutz kurz ausschalten oder einen Handy-Hotspot benutzen.

### 3. BIOS einstellen

BIOS beim Start mit **F2** öffnen, Boot-Menü mit **F12**. Je nach BIOS-Version fehlen manche Punkte, das ist ok.

| Einstellung | Wert |
|---|---|
| Secure Boot | **Disabled** |
| Fast Boot | Disabled |
| CSM / Legacy | Disabled (reines UEFI) |
| SATA/NVMe-Modus | **AHCI**, nicht Intel RST/VMD |
| Intel VMD Controller | Disabled |
| VT-d | egal (`DisableIoMapper` ist aktiv) |
| Above 4G Decoding / Resizable BAR | Enabled, falls vorhanden |
| Graphics Mode / Display Mode | **Discrete/dGPU**, falls dein BIOS das anbietet (dann läuft auch der Laptop-Bildschirm über die RTX) |

> Wird Windows nach der Umstellung auf AHCI nicht mehr starten, vorher in Windows einmal im abgesicherten Modus starten
> (`bcdedit /set {current} safeboot minimal`, BIOS umstellen, starten, `bcdedit /deletevalue {current} safeboot`).

### 4. macOS installieren

1. Stick einstecken, mit **F12** vom USB-Stick (UEFI) starten.
2. Im OpenCore-Menü **„dmg“ / „macOS Base System“** wählen. Der Start ist absichtlich ausführlich (`-v`, weißer Text).
3. Sprache wählen → **Festplattendienstprogramm** → oben „Darstellung → Alle Geräte einblenden“.
   * Eigene SSD für macOS: die SSD wählen → Löschen → **APFS**, Schema **GUID-Partitionstabelle**.
   * Neben Windows: **nur** den freien Bereich/die vorbereitete Partition als APFS formatieren. **Nicht** die ganze
     Windows-SSD löschen!
4. **macOS Sequoia erneut installieren** → Ziel wählen. Es lädt jetzt etwa 13 GB von Apple.
5. Der Laptop startet mehrmals neu. Jedes Mal mit **F12 vom Stick** starten und im OpenCore-Menü **„macOS Installer“**
   bzw. später den Namen deines Laufwerks wählen, bis der Einrichtungsassistent kommt.
6. Einrichtungsassistent durchklicken. WLAN geht hier noch nicht (kommt mit HeliPort). LAN-Kabel funktioniert.

### 5. NVIDIA-Treiber installieren (in macOS)

**Empfohlen: mit der 1401-Mac-App** (passt die OpenCore-config auf dem Stick automatisch an):

1. Den Stick, von dem du gestartet hast, **eingesteckt lassen**.
2. Auf dem Stick `NullMoth/1401-Mac-1.13.0.zip` öffnen → **1401.app** starten. Bei einer Gatekeeper-Warnung:
   Systemeinstellungen → Datenschutz & Sicherheit → „Trotzdem öffnen“.
3. Den Schritten folgen. Die App
   * findet die OpenCore-Partition (den Stick) und zeigt jede Änderung, bevor sie sie macht,
   * erkennt, dass der Laptop-Bildschirm an der Intel-GPU hängt, und lässt IONDRVSupport an,
   * stellt die BAR-Werte für den Treiber ein (`ResizeGpuBars 13`, `ResizeAppleGpuBars -1`),
   * installiert den Treiber und legt im Boot-Menü **„1401: Remove NVIDIA driver“** als Rückweg an,
   * kann die **USB-Ports mappen** (empfohlen).
4. Wenn macOS fragt: Systemeinstellungen → Datenschutz & Sicherheit → **Erweiterungen erlauben** → neu starten → nochmal
   neu starten.
5. Der erste Start mit Treiber kann bis zu einer Minute bei „PCI configuration end“ stehen bleiben. Die RTX meldet
   sich etwa eine Minute nach dem Login als zweite GPU.

**Prüfen:**

```bash
kmutil showloaded --list-only | grep nullmoth      # 4 Zeilen
system_profiler SPDisplaysDataType | head -30      # GeForce RTX 4050 Laptop GPU, Metal: unterstützt
```

<details>
<summary>Alternativ: manuell ohne 1401-App</summary>

```bash
cd /Volumes/<STICK>/NullMoth
tar -xzf nullmoth-nvidia-1.13.0.tar.gz && cd pkgroot
shasum -a 256 -c SHA256SUMS     # jede Zeile muss OK sagen
sudo ./install.sh
```

Danach auf dem Stick `/Volumes/<STICK>/EFI/OC/config.plist` durch `/Volumes/<STICK>/NullMoth/config-nach-treiber.plist`
ersetzen (umbenennen in `config.plist`). Beide Dateien hat das Skript mit derselben Seriennummer befüllt.
Dann `sudo shutdown -r now`.

Deinstallieren: `sudo ./uninstall.sh && sudo shutdown -r now`, danach wieder die normale `config.plist` nehmen.
</details>

### 6. Nach der Installation

* **WLAN:** `Extras/HeliPort-1.5.0.dmg` öffnen, HeliPort in Programme ziehen, starten, in den Anmeldeobjekten eintragen.
* **Ohne Stick starten** (OpenCore auf die interne SSD kopieren): erst, wenn alles stabil läuft. NullMoth rät, den Stick
  bis dahin dranzulassen, weil die 1401-App genau diese config verwaltet. Dann mit `sudo diskutil mount disk0s1`
  (EFI der macOS-SSD) die `EFI`-Ordner vom Stick dorthin kopieren. Ein vorhandenes `EFI/Microsoft` **nicht** löschen.
  Danach den Stick in der 1401-App neu auswählen.
* **Ausführlichen Start abschalten:** in `config.plist` unter `NVRAM → Add → 7C436110-… → boot-args` `-v debug=0x100
  keepsyms=1` entfernen.
* **Ruhezustand** abschalten (er ist auf Hackintosh-Laptops oft kaputt): `sudo pmset -a hibernatemode 0 standby 0 autopoweroff 0`.

---

## Probleme lösen

| Problem | Lösung |
|---|---|
| Stick erscheint nicht unter F12 | Secure Boot aus? Stick GPT + FAT32? `EFI/BOOT/BOOTx64.efi` vorhanden? |
| Hängt bei `[EB|#LOG:EXITBS:START]` | In `config.plist`: `Booter → Quirks → SetupVirtualMap` auf `false`, sonst `DevirtualiseMmio` auf `false` testen |
| Hängt bei „PCI configuration end“ (Installer) | Bis zu 2 Minuten warten. Sonst `npci=0x2000` zu den boot-args hinzufügen |
| Verbotsschild / `EB.MM.AKM` | Wenn möglich NVMe-SSD und Foto des `-v`-Bildschirms in den NullMoth-Discord |
| Kein Ton | `alcid=14` in den boot-args durch einen anderen Wert ersetzen. ALC256: 5, 11, 13, 21, 56, 57, 66, 67, 69, 70, 76, 88, 95, 97, 99; ALC255: 3, 11, 13, 17, 21, 22, 27, 28, 29, 31, 66, 99 |
| Touchpad tot | `-vi2c-force-polling` steht in den boot-args. Sonst `SSDT-GPI0.aml` und `SSDT-XOSI.aml` in `ACPI → Add` testweise ausschalten |
| Einige USB-Ports gehen nicht | Mit der 1401-Mac-App (oder USBToolBox unter Windows) USB-Map erstellen |
| Nach dem Treiber schwarzer Bildschirm | Im OpenCore-Menü **„1401: Remove NVIDIA driver“** wählen (Leertaste zeigt versteckte Einträge). Das stellt den Zustand vor der Installation wieder her |
| RTX taucht nicht auf | Eine Minute nach dem Login warten. Dann 1401 → Crash report → **Send logs to NullMoth** und die Report-ID im NullMoth-Discord nennen |
| Firefox stürzt beim Start ab | `ipc_control_port_options=0` ist schon gesetzt, Treiber/1401 auf den neuesten Stand bringen |

NullMoth sammelt alle bekannten Probleme hier: <https://nullmothsystems.com/help>

### Plan B: EFI von 1401 direkt auf deinem Laptop bauen lassen

Diese EFI wurde ohne Daten von deinem Gerät gebaut. Die **1401-Windows-App** liest dagegen die echte Hardware und die
echten ACPI-Tabellen deines G5 MF und erzeugt daraus eine genau passende EFI. Wenn mit diesem Repo etwas nicht startet:

1. `1401-Windows-<version>.zip` von <https://github.com/nullmoth/1401/releases> laden und `1401.exe` starten.
2. **Check this PC** → **Build the Mac setup** → **Create the macOS stick**.
3. Ab Schritt 3 dieser Anleitung weitermachen.

---

## Wie diese EFI entstanden ist

1. **1401** (NullMoth, Python-Engine `p1401` auf Basis von OpCore-Simplify) wurde unter Linux mit einem Hardware-Report
   für den G5 MF (`Extras/hardware-annahme-G5MF.json`: i5-12500H, Iris Xe 8086-46A6, RTX 4050 Laptop 10DE-28A1,
   Intel AX201, Realtek RTL8111, Realtek ALC256, I2C-HID-Touchpad) ausgeführt. Ein ACPI-Dump vom G5 MF war nicht
   verfügbar. Deshalb diente der DSDT eines Gigabyte AERO 16 (gleiche Alder/Raptor-Lake-Plattform, gleicher
   Gigabyte-BIOS-Stamm) als Vorlage für die ACPI-Analyse. 1401 hat die Kexts gewählt, die Quirks gesetzt, seine
   NullMoth-Regeln angewendet (boot-args, SIP `0x0A43`, `acpi-wake-type`, Installer-BAR) und die config mit
   `ocvalidate` geprüft.
2. Danach angepasst, nach den Regeln aus NullMoth 1.11 bis 1.13 für Laptops ohne MUX:
   * Die Iris Xe wird **nicht** mehr abgeschaltet (`disable-gpu` entfernt), damit der interne Bildschirm macOS zeigt.
     IONDRVSupport wird nicht blockiert.
   * **AMFIPass 1.4.1** ergänzt (ohne ihn lädt WindowServer den Metal-Treiber nicht), dazu `ipc_control_port_options=0`.
   * Die vom Vorlage-DSDT abgeleitete GPI0-Umbenennung greift nur noch gezielt an `\_SB.GPI0._STA` statt global.
   * `SSDT-RTCAWAC` mit `CondRefOf`, damit sie bei fehlendem `STAS` nichts kaputt macht.
   * OpenShell ergänzt, Seriennummern durch Platzhalter ersetzt (Repo ist öffentlich).
3. Beide configs bestehen `ocvalidate` (OpenCore 1.0.9) ohne Fehler.

---

## Lizenzen

* **NullMoth NVIDIA-Treiber und 1401:** © NullMoth Systems, [PolyForm Noncommercial 1.0.0](Extras/NullMoth/LICENSE-NullMoth.txt).
  Kostenlos, aber **nicht kommerziell** nutzbar. Unterstützen: <https://buymeacoffee.com/nullmoth>
* **OpenCore** (acidanthera): BSD-3-Clause. **Lilu, VirtualSMC, WhateverGreen, AppleALC, NVMeFix, RestrictEvents**
  (acidanthera): BSD-3-Clause. **AMFIPass:** aus den OpenCore-Legacy-Patcher-Payloads (dortania). **itlwm, IntelBluetoothFirmware, HeliPort** (OpenIntelWireless): GPL-2.0 /
  BSD-3. **VoodooI2C, VoodooPS2** (VoodooI2C/acidanthera): GPL-2.0. **RealtekRTL8111** (Mieze): eigene Lizenz.
  **USBToolBox** (dhinakg): MIT. **ECEnabler, BrightnessKeys, CpuTopologyRebuild, XHCI-unsupported:** jeweilige
  Upstream-Lizenzen.
* macOS darf laut Apple-Lizenz nur auf Apple-Hardware laufen. Nutzung auf eigenes Risiko.
