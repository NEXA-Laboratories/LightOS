# Create a VirtualBox VM for LightOS (Windows PowerShell).
#   live medium  -> IDE primary master  (what you boot)
#   empty target -> IDE primary slave   (what 'install 1' writes to)
# Usage: scripts\vbox-create.ps1 [-Name LightOS] [-TargetMiB 1024] [-Server] [-Force]
param([string]$Name = "LightOS", [int]$TargetMiB = 1024, [switch]$Server, [switch]$Force)
$root = Split-Path -Parent $PSScriptRoot
$vbm = (Get-Command VBoxManage -ErrorAction SilentlyContinue).Source
if (-not $vbm) { $vbm = "C:\Program Files\Oracle\VirtualBox\VBoxManage.exe" }
if (-not (Test-Path $vbm)) { throw "VBoxManage not found. Install VirtualBox or add it to PATH." }
$bin = "$root\kernel\target\x86_64-lightos\release\bootimage-lightos.bin"
if (-not (Test-Path $bin)) { throw "Build first:  cd kernel; cargo bootimage --release" }
$build = "$root\build"
New-Item -ItemType Directory -Force $build | Out-Null

# Run VBoxManage and stop on a non-zero exit code (stderr text alone is not an error).
function Vbm { & $vbm @args; if ($LASTEXITCODE -ne 0) { throw "VBoxManage failed: $args" } }

python "$root\tools\mkboot.py" $bin "$build\live.img"
if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 3) { throw "tools\mkboot.py failed" }

if ($Force) {
    # It is fine if the VM does not exist yet.
    & $vbm unregistervm $Name --delete 2>&1 | Out-Null
    Remove-Item "$build\live.vdi", "$build\target.vdi" -ErrorAction SilentlyContinue
}
Vbm convertfromraw "$build\live.img" "$build\live.vdi" --format VDI
Vbm createmedium disk --filename "$build\target.vdi" --size $TargetMiB --format VDI
Vbm createvm --name $Name --ostype Other_64 --register
Vbm modifyvm $Name --memory 256 --vram 16 --cpus 1 --ioapic off --nic1 nat --nictype1 82540EM --cableconnected1 on --boot1 disk --boot2 none --boot3 none --boot4 none
if ($Server) {
    Vbm modifyvm $Name --uart1 0x3F8 4 --uartmode1 tcpclient 127.0.0.1:7878
}
Vbm storagectl $Name --name "IDE" --add ide --controller PIIX4
Vbm storageattach $Name --storagectl "IDE" --port 0 --device 0 --type hdd --medium "$build\live.vdi"
Vbm storageattach $Name --storagectl "IDE" --port 0 --device 1 --type hdd --medium "$build\target.vdi"

Write-Host ""
Write-Host "VM '$Name' created. Start it from the VirtualBox window, then at the los> prompt:"
Write-Host "    disks"
Write-Host "    install 1"
Write-Host "When it says 'Installation complete':  shutdown,  then run  scripts\vbox-finish.ps1"
