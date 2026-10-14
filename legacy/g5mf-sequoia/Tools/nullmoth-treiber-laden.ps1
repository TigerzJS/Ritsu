# Laedt das NullMoth-NVIDIA-Treiberpaket 1.13.0 von GitHub und prueft die SHA-256.
#   powershell -ExecutionPolicy Bypass -File Tools\nullmoth-treiber-laden.ps1 -Ziel E:\NullMoth
param([string]$Ziel = (Join-Path (Split-Path -Parent $PSScriptRoot) 'Extras\NullMoth'))
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$Name = 'nullmoth-nvidia-1.13.0.tar.gz'
$Url = "https://github.com/nullmoth/nvidia-macos-driver/releases/download/v1.13.0/$Name"
$Sha = '994970b1bba53e3ae8d7f500ef27b25cae8bc6818abc112d35230a58e6882fdf'

New-Item -ItemType Directory -Force $Ziel | Out-Null
$dst = Join-Path $Ziel $Name
if ((Test-Path $dst) -and ((Get-FileHash -Algorithm SHA256 $dst).Hash -ieq $Sha)) {
    Write-Host "$Name ist schon vorhanden und geprueft."
    exit 0
}
Write-Host "Lade $Name (ca. 270 MB) ..."
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile "$dst.part"
$h = (Get-FileHash -Algorithm SHA256 "$dst.part").Hash
if ($h -ine $Sha) { Remove-Item "$dst.part"; throw "SHA-256 stimmt nicht ($h) - Datei verworfen" }
Move-Item -Force "$dst.part" $dst
Write-Host "$Name geladen, SHA-256 OK." -ForegroundColor Green
