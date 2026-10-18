# Boot LightOS in QEMU with the data disk. Add -Server to wire COM1 to los-server
# (start it first: python server\los_server.py serve)
param([switch]$Server)
$root = Split-Path -Parent $PSScriptRoot
$qemu = (Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue).Source
if (-not $qemu) { $qemu = "C:\Program Files\qemu\qemu-system-x86_64.exe" }
$serial = if ($Server) { "tcp:127.0.0.1:7878" } else { "stdio" }
& $qemu -drive "format=raw,file=$root\kernel\target\x86_64-lightos\release\bootimage-lightos.bin" `
        -drive "file=$root\data.img,format=raw,if=ide,index=1" -nic user,model=e1000 -serial $serial -m 128M
