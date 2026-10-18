# Running LightOS in VirtualBox

LightOS boots with the legacy **BIOS** and talks to **IDE** disks (PIIX4). It has no SATA/AHCI driver yet,
so the VM must use an *IDE* controller.

## Automatic (Windows PowerShell)

```powershell
cd kernel; cargo bootimage --release; cd ..          # build the kernel image
powershell -ExecutionPolicy Bypass -File scripts\vbox-create.ps1 -Force
```
This converts the image to `build\live.vdi`, creates an empty `build\target.vdi` (1 GiB, change with `-TargetMiB`)
and a VM named `LightOS` with: Other/Unknown (64-bit), 256 MB RAM, I/O APIC off, no network, an IDE (PIIX4)
controller with **live.vdi = primary master** and **target.vdi = primary slave**.
`-Server` also maps COM1 to `127.0.0.1:7878` (start `python server\los_server.py serve` first).

## Manual (GUI)
1. `python tools\mkboot.py kernel\target\x86_64-lightos\release\bootimage-lightos.bin build\live.img`
   (prints **OK** if the installer can store a partition table; otherwise send me the output)
2. `VBoxManage convertfromraw build\live.img build\live.vdi --format VDI`
3. `VBoxManage createmedium disk --filename build\target.vdi --size 1024 --format VDI`
4. New VM: type *Other*, version *Other/Unknown (64-bit)*, 256 MB, **no hard disk**.
5. Settings -> System: I/O APIC off (EFI must stay off). Network: disabled is fine.
6. Settings -> Storage: delete the default controllers, add an **IDE** controller (PIIX4),
   attach `live.vdi` to *IDE Primary Master* and `target.vdi` to *IDE Primary Slave*.
7. Optional serial for los-server: Settings -> Serial Ports -> Port 1: COM1, Host Pipe/TCP **client**, `127.0.0.1:7878`.

## Installing
```
los> disks                 # disk 0 = live medium, disk 1 = empty target
los> install 1             # type YES when asked
los> shutdown
```
`install` writes the bootloader+kernel to the first 8 MiB of disk 1, creates an MBR partition table
(one FAT32 partition for the rest of the disk, or reuses the first FAT32 partition if the disk already has a table),
formats it, and copies the live volume's files (apps, package database) across.
Then run `scripts\vbox-finish.ps1` (or in the GUI: detach live.vdi, attach target.vdi as IDE Primary Master) and start the VM:
LightOS now boots from its own disk and `los install ...` persists on partition 1.

## Partitioning by hand
```
los> part list 1
los> part new 1 512        # 512 MiB FAT32 partition after the 8 MiB system area
los> part new 1 max        # all remaining space
los> part mkfs 1 1         # format it (asks for YES)
los> mount 1 1
los> part delete 1 2       # asks for YES
los> part wipe 1           # remove the table (asks for YES)
```
Partitions are 1 MiB aligned, primary only (max 4), type 0x0C. Disk 0 (the boot medium) is protected:
`install 0` is refused and its table is only edited if it does not touch the bootloader.

## Troubleshooting
- *"No ATA disks found"*: the VM uses SATA. Use an IDE controller (PIIX4).
- Black screen / reboot loop: take a screenshot; the kernel prints a red panic screen for most faults.
- `install` says the boot sector uses the partition-table area: run `tools\mkboot.py` and send me its output.
- `shutdown` powers the VM off via the VirtualBox ACPI port; if it only prints a message, use *Machine -> ACPI Shutdown*.
