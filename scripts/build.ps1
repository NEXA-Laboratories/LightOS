# Build the kernel image and the data disk (Windows PowerShell).
$root = Split-Path -Parent $PSScriptRoot
Push-Location "$root\kernel"
cargo bootimage --release
Pop-Location
python "$root\tools\mkdisk.py" "$root\data.img" --root "$root\disk-root"
