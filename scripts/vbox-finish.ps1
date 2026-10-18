# After 'install 1' finished and the VM is powered off: boot from the installed disk.
# Detaches the live medium and attaches the target disk as IDE primary master.
param([string]$Name = "LightOS")
$root = Split-Path -Parent $PSScriptRoot
$vbm = (Get-Command VBoxManage -ErrorAction SilentlyContinue).Source
if (-not $vbm) { $vbm = "C:\Program Files\Oracle\VirtualBox\VBoxManage.exe" }
$build = "$root\build"
& $vbm storageattach $Name --storagectl "IDE" --port 0 --device 0 --medium none
& $vbm storageattach $Name --storagectl "IDE" --port 0 --device 1 --medium none
& $vbm storageattach $Name --storagectl "IDE" --port 0 --device 0 --type hdd --medium "$build\target.vdi"
Write-Host "Done: '$Name' now boots from the installed disk (target.vdi)."
