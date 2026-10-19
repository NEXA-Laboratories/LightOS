# User mode (ring 3) in LightOS 0.5.0

Apps (static ELF64 files) now run in **ring 3**, the unprivileged CPU mode. The kernel stays in ring 0.

## How it works
- **GDT**: new ring-3 code and data segments; the TSS has `rsp0`, a 256 KiB kernel stack the CPU switches to
  whenever ring-3 code is interrupted or makes a system call.
- **Memory**: an app is loaded at `0x0000_0400_0000_0000` (4 TiB; 16 MiB for code/data, then a 64 KiB stack).
  Only those pages (and the page-table levels above them) carry the USER flag. **Kernel memory has no USER flag**,
  so any access from an app is a page fault.
- **Entering**: `iretq` with ring-3 CS/SS, RIP = ELF entry, RSP = stack top, IF = 1, IOPL = 0, all general
  registers cleared (no kernel values leak).
- **System calls**: `int 0x80` (the IDT gate is DPL 3). Pointers passed to the kernel are checked against the
  app's own mapped ranges, so an app cannot make the kernel read or write kernel memory for it.
- **Faults**: page fault, general protection fault (e.g. `hlt`, `in`, `out`, `cli`), invalid opcode, divide error
  from ring 3 print `[app] crashed: ...` and kill only the app; you are back at the prompt.
- **Ctrl+C / Esc** (checked on every timer tick) stops an app stuck in a loop.
- Each run starts with zeroed pages, so one app cannot read the previous app's data.

## System calls (rax = number, rdi/rsi/rdx = args, result in rax)
1 WRITE(fd,ptr,len) · 2 GETPID · 3 EXIT(code) · 4 YIELD · 5 UPTIME · **6 GETCHAR** (blocks, 3 = Ctrl+C) ·
**7 SLEEP(ms)** · 10 LOS_INSTALL · 11 LOS_REMOVE · 12 LOS_LIST

## Try it
```
los update
los install hello-elf crash-elf priv-elf spin-elf
los run hello-elf     # prints a line, exits with code 0
los run crash-elf     # [app] crashed: page fault (address 0x200000 ...) - and the shell is still alive
los run priv-elf      # [app] crashed: general protection fault
los run spin-elf      # loops; press Ctrl+C or Esc -> [app] stopped
```
Also `exec /BIN/HELLO.ELF` etc. from a data disk built with `tools/mkdisk.py` (rebuild it: the ELFs moved to the new base).

## Building your own apps
`examples/hello-app` + `liblightos`; link at `--image-base=0x40000000000` (already set in its `.cargo/config.toml`).

## Not there yet
- One shared address space (apps are isolated from the *kernel*, but not from each other; only one app runs at a time).
- No preemptive multitasking between apps, no per-app page tables, no file or network syscalls yet.
- Apps built before 0.5.0 (linked at 1 TiB) must be relinked.
