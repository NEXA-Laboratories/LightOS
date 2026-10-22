# LightOS 0.5.0

![Version](https://img.shields.io/badge/version-0.5.0-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![Rust](https://img.shields.io/badge/rust-nightly-orange)

LightOS is a 64-bit operating system kernel written in Rust from scratch, with its own package manager
(**LOS**) and package server (**los-server**).

**New in 0.5.0 - user mode (ring 3)**
- Apps now run in **ring 3**: kernel memory is off-limits, a crashing or hostile app is killed (`[app] crashed: ...`) and the system keeps running,
  `Ctrl+C`/`Esc` stops an app stuck in a loop, syscall pointers are validated. See `docs/RING3.md`.
- New syscalls `GETCHAR` and `SLEEP`; demo apps `crash-elf`, `priv-elf`, `spin-elf` (packages) show the protection working.
- **Breaking:** the app region moved to 4 TiB; relink older apps (`--image-base=0x40000000000`).

**New in 0.4.3 - two editions + a much better shell**
- **Editions.** The installer now asks: **1) Console** (text shell only) or **2) Desktop** (text-mode desktop + shell).
  The choice is stored in `/LOS/EDITION.TXT`; change it any time with `edition console|desktop`.
  The Desktop edition is the `los-desktop` package (installed automatically by the installer).
- **LightOS Desktop (text mode):** menu bar with clock, tiles for **Files** (browse, open, edit, new file/folder, delete, run .ELF),
  **Terminal**, **Packages** (install/remove/update with one key), **System** (memory, disks, network, uptime) and **Power**.
  Esc leaves to the console shell; the `desktop` command opens it again. (A pixel-graphics desktop comes later.)
- **Shell:** history (Up/Down), Tab completion (commands and file paths), `cd`/`pwd` + relative paths and a `los:/DIR>` prompt,
  variables (`set NAME=value`, `$NAME`, `${NAME}`, `env`, `unset`; built-ins `$CWD $IP $VERSION $EDITION`),
  scripts (`source file`, lines starting with `#` are comments) and an optional boot script `/LOS/STARTUP.LOS`, `history`, `exit`.

**New in 0.4.2 (networking, part 3)**
- **TCP** and an **HTTP client**: `wget <http://host/path> [file]`
- **`los update` and `los install` now use the network** (HTTP to los-server at 10.0.2.2:8080, COM1 as fallback); `los server <ip[:port]>`

**New in 0.4.1 (networking, part 2)**
- UDP, **DHCP client** (automatic address at boot, `dhcp`), **DNS resolver** (`nslookup`, `ping <hostname>`)
- TCP, HTTP and `los update` over the network come next (0.4.2)

**New in 0.4.0 (networking, part 1)**
- PCI scan (`pci`), Intel **e1000 network driver**, Ethernet/ARP/IPv4/ICMP: `ifconfig`, `ping`, `arp` (see `docs/NETWORK.md`)
- VirtualBox VMs now get a NAT adapter (Intel PRO/1000 MT Desktop); UDP/DHCP/TCP/HTTP and `los update` over the network follow in 0.4.x

**New in 0.3.1**
- `edit <file>`: a real full-screen text editor (arrows, F2 save, Esc quit), provided by the `los-edit` package
- wider tables (the `disks` table no longer cuts off lines)

**New in 0.3.0**
- Runs in **VirtualBox** (BIOS + IDE) as well as QEMU: `scripts\vbox-create.ps1`, see `docs/VIRTUALBOX.md`
- **System installer**: `install <disk>` copies LightOS (bootloader, kernel, files, packages) to another disk
- **Partition system**: MBR editor (`disks`, `part list|new|delete|wipe|mkfs`), FAT32 formatter, `mount`/`unmount`
- Up to 4 IDE disks, automatic mount of the first FAT32 volume (partition or raw)
- Includes everything from 0.2.0: boot animation, FAT32, persistent packages, ELF apps

## Quick start: VirtualBox (Windows PowerShell)
```powershell
cd kernel
cargo bootimage --release
cd ..
powershell -ExecutionPolicy Bypass -File scripts\vbox-create.ps1 -Force
# start the VM "LightOS" in VirtualBox, then at los>:
#   disks
#   install 1        (type YES)
#   shutdown
powershell -ExecutionPolicy Bypass -File scripts\vbox-finish.ps1
```
Start the VM again: it boots from its own disk.

## Quick start: QEMU
```powershell
python tools\mkdisk.py data.img --root disk-root        # optional FAT32 data disk with sample files
powershell -ExecutionPolicy Bypass -File scripts\run.ps1
```
To try the installer in QEMU, create a blank disk (`qemu-img create -f raw target.img 1G`) and attach it as
the second IDE disk instead of data.img (`-drive file=target.img,format=raw,if=ide,index=1`).

## Shell commands
| Area | Commands |
|---|---|
| Shell | `cd pwd set unset env history source exit` (see above), `edition`, `desktop` |
| System | `help clear echo ver uptime mem vtop reboot shutdown` |
| Processes | `ps spawn run kill reap syscalls bp panic` |
| Files | `ls cat write mkdir rm df exec edit` (edit needs `los install los-edit`) |
| Network | `pci`, `ifconfig [ip mask gw]`, `dhcp`, `nslookup <host>`, `ping <host|ip> [count]`, `arp`, `wget <url> [file]` |
| Disks | `disks`, `part list\|new\|delete\|wipe\|mkfs`, `mount [disk [part]]`, `unmount` |
| Installer | `install <disk>` |
| Packages | `los install\|remove\|list\|search\|update\|available\|info\|verify\|autoremove\|run` |

## Disk layout of an installed system
```
LBA 0          boot sector (bootloader stage 1) + MBR partition table (bytes 446..509)
LBA 1..16383   bootloader + kernel (8 MiB system area; partitions never start below it)
LBA 16384..    partition 1: FAT32  (/LOS/DB.TXT package database, /BIN apps, your files)
```
Disk numbers = IDE position: 0 primary master (boot medium), 1 primary slave, 2 secondary master, 3 secondary slave.

## Layout of the source
```
kernel/src/   main animation logo | vga serial gdt interrupts | memory allocator | process syscall elf |
              ata mbr fat32 installer | keyboard shell ui | los/ (package manager)
liblightos/   userspace library      examples/hello-app/   example app
server/       los_server.py + tests  repo/                 package repository
tools/        gen_logo.py mkdisk.py mkelf.py mkboot.py     scripts/   PowerShell + sh helpers
docs/         PROTOCOL.md VIRTUALBOX.md NETWORK.md RING3.md
```

## Known limits of 0.5.0
- **Not tested on real hardware or in VirtualBox by me**; the kernel code is written blind, so expect to
  report a few compiler errors / crashes (that is how 0.1.0 and 0.2.0 got working).
- IDE/PIO only: no SATA/AHCI, no NVMe, no USB storage. Use an IDE controller in VirtualBox.
- BIOS boot only (no UEFI). MBR with up to 4 primary partitions; FAT32 only, 8.3 file names.
- The installer needs the bootloader to leave bytes 446..509 of sector 0 free; `tools\mkboot.py` checks this
  before you start. The target must be disk 1-3 (disk 0 is the live medium).
- The "Desktop" edition is a text-mode (80x25) desktop, not pixel graphics yet. Apps run in ring 3 but share one address space and run one at a time. Network: IPv4 only, one TCP connection at a time, HTTP only (no HTTPS); no USB.

## Roadmap
 v0.6.0 AHCI/SATA, USB, per-app address spaces + multitasking · v0.7.0 pixel-graphics desktop, threads, SMP · v1.0.0 stable ABI, docs, package signing

MIT License. LightOS Team (NEXA Laboratories).
