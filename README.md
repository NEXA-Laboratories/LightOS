# LightOS - Modern operating system

![Version](https://img.shields.io/badge/version-0.1.0-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![Rust](https://img.shields.io/badge/rust-nightly-orange)

**LightOS** is a modern 64-bit operating system kernel written in Rust from scratch. The project demonstrates the power of Rust system programming and includes its own LOS package manager.

## 🚀 Main features

### Core
- ✅ **Memory Management** - Physical and virtual memory, heap allocator
- ✅ **Process Management** - Multitasking, process scheduler
- ✅ **Interrupt Handling** - IDT, CPU exception handling
- ✅ **System Calls** - Interface for userspace applications
- ✅ **VGA Driver** - Color text output
- ✅ **Serial Port** - Debugging output via COM1

### LOS Package Manager
- 📦 **Installing packages** - `los install <package>`
- 🗑️ **Removing packages** - `los remove <package>`
- 📋 **List of installed** - `los list`
- 🔍 **Package Search** - `los search <query>`
- 🔄 **Repository update** - `los update`
- ✓ **Package Verification** - Security check before installation
- 🔗 **Dependency Management** - Automatic installation

### Security
- ✓ Package verification before installation
- ✓ Isolated application execution
- ✓ Checking dependencies
 Warnings about unchecked packages

## 📁 Project structure

```
lightos/
├── src/
,── main.rs # The entry point of the core
,── vga_buffer.rs # VGA driver
│   ├── serial.rs         # Serial port
,── interrupts.rs # Interrupt handling
│   ├── gdt.rs            # Global Descriptor Table
,── memory.rs # Memory management
│   ├── allocator.rs      # Heap allocator
,── process.rs # Process management
,── syscall.rs # System calls
,── task/ # Asynchronous tasks
│   └── package.rs        # LOS Package Manager
├── Cargo.toml
├── x86_64-lightos.json   # Target specification
└── README.md
```

## 🛠️ Build and launch

### Requirements
- Rust nightly (`rustup default nightly`)
- `bootimage' crate: `cargo install bootimage'
- QEMU for testing

### Compilation
```bash
# Build the core
cargo build --release

# Create a boot image
of cargo botimage

# Launch in QEMU
cargo run
```

### Launching in QEMU
```bash
qemu-system-x86_64 -drive format=raw,file=target/x86_64-lightos/release/bootimage-lightos.bin
```

## 📦 LOS Package Manager

### Teams

#### Package Installation
```rust
los install los-shell
```

#### Package Removal
```rust
los remove demo-app
```

#### List of installed packages
```rust
los list
```
Conclusion:
``
╔════════════════════════════════════════════════════════╗
║              Installed Packages                       ║
╠════════════════════════════════════════════════════════╣
║ ✓ los-core-0.1.0    by LightOS Team
║ ✓ los-shell-0.1.0   by LightOS Team
╚════════════════════════════════════════════════════════╝
```

#### Package Search
```rust
los search editor
```

#### Updating the repository
```rust
los update
```

### Available packages

| Package | Version | Description | Verified |
|-------|--------|----------|----------|
| `los-core` | 0.1.0 | Core utilities | ✓ |
| `los-shell` | 0.1.0 | Command shell | ✓ |
| `los-edit` | 0.1.0 | Text editor | ✓ |
| `los-net` | 0.1.0 | Network utilities | ✓ |
| `demo-app` | 1.0.0 | Demo application | ✗ |

## 💻 Application Development

### Creating an application for LightOS

1. **Create a project**
```bash
cargo new --bin my-app
```

2. **Configure Cargo.toml**
```toml
[dependencies]
liblightos = "0.1.0" # LightOS Standard Library

[profile.release]
panic = "abort"
```

3. **Write the code**
```rust
#![no_std]
#![no_main]

use liblightos::*;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println!("Hello from LightOS app!");
    
    // System calls
let pid = sys_getpid();
println!("My PID: {}", pid);
    
    sys_exit(0);
}
```

4. **Assemble**
```bash
cargo build --release --target x86_64-lightos
```

5. **Create a package manifest** (`package.toml`)
``toml
[package]
name = "my-app"
version = "1.0.0"
description = "My awesome application"
author = "Your Name"
license = "MIT"

[dependencies]
los-core = "0.1.0"
```

6. **Publish**
```bash
los submit my-app
```

### System calls

#### Basic challenges
``rust
// Text
output sys_write(1, "Hello".as_ptr(), 5);

// Get PID
let pid = sys_getpid();

// End the sys_exit process
(0);
``

#### LOS-specific challenges
``rust
// Install the package programmatically
sys_los_install("package-name".as_ptr(), 12);

// Delete the package
sys_los_remove("package-name".as_ptr(), 12);

// Package list
sys_los_list();
``

## 🎯 Demonstration for the conference

### Ready-made scenarios

####1. Booting the system
```
╔════════════════════════════════════════════╗
║        LightOS v0.1.0 - Booting...       ║
╚════════════════════════════════════════════╝
[OK] GDT initialized
[OK] IDT initialized
[OK] PICs initialized
[OK] Interrupts enabled
[OK] Heap initialized
[OK] Process manager initialized
[OK] System calls initialized

╔════════════════════════════════════════════╗
║     LightOS Ready - Welcome to Shell      ║
╚════════════════════════════════════════════╝
```

####2. Process Management
```rust
> ps
╔════════════════════════════════════════════╗
║            Process List                   ║
╠════════════════════════════════════════════╣
║ PID │ Name           │ State    │ Priority║
╠════════════════════════════════════════════╣
║   1 │ init           │ Running  │        5║
║   2 │ los-shell      │ Ready    │        5║
╚════════════════════════════════════════════╝
```

####3. Package Manager
```rust
> los install los-edit
📦 Installing dependency: los-core
✓  Successfully installed los-core
📦 Installing los-edit-0.1.0
   Author: LightOS Team
   Description: Simple text editor
✓  Successfully installed los-edit

> los list
╔════════════════════════════════════════════════════════╗
║              Installed Packages                       ║
╠════════════════════════════════════════════════════════╣
║ ✓ los-core-0.1.0    by LightOS Team
║ ✓ los-edit-0.1.0    by LightOS Team
╚════════════════════════════════════════════════════════╝
```

## 🔧 Architecture

### Layers of the system

```
┌─────────────────────────────────┐
│      User Applications          │
├─────────────────────────────────┤
│     liblightos (stdlib)         │
├─────────────────────────────────┤
│      System Calls API           │
├─────────────────────────────────┤
│  Process Manager | VFS | Network│
├─────────────────────────────────┤
│   Memory Manager | Scheduler    │
├─────────────────────────────────┤
│    Interrupt Handlers | Drivers │
├─────────────────────────────────┤
│         Hardware (x86_64)       │
└─────────────────────────────────┘
```

### Main components

1. **Boot** - Bootloader via crate bootloader
2. **Memory** - Paging, heap allocation
3. **Interrupts** - IDT, exception handling
4. **Processes** - Round-robin scheduler
5. **Syscalls** - Userspace API
6. **Drivers** - VGA, Serial, Keyboard
7. **Package Manager** - LOS ecosystem

## 📊 Performance

- **Download Time**: ~100ms
- **Memory Usage**: ~4MB (core)
- **Context switch**: ~1 microns
- **Process support**: up to 1024

## 🗺️ Roadmap

### v0.2.0
- [ ] FAT32 file system
- [ ] ELF loader for userspace
- [ ] Network stack (TCP/IP)
- [ ] USB support

### v0.3.0
- [ ] GUI framework
- [ ] Multithreading (threads)
- [ ] SMP (multi-core)
- [ ] Advanced scheduler

### v1.0.0
- [ ] Stable ABI
- [ ] Full API Documentation
- [ ] Official Package Repository
- [ ] Package signing & verification

## Publishing packages

### Verification process

1. **Package Sending**
- Automatic security check
   - Dependency analysis
   - Code quality control

2. **Review**
- Manual code
review - Functionality testing
   - Checking documentation

3. **Approval**
   - Getting the "Verified" status
   - Publication in the repository
   - Notification of the author

### Package Requirements

✓ Works in a no_std environment  
✓ Correct error handling  
✓ Documented code  
✓ All dependencies are specified  
✓ License (MIT/Apache-2.0)  
✓ Secure code (no unsafe where possible)

## 📝 License

MIT License - see the LICENSE file

## , The authors

- **LightOS Team (NEXA Laboratories)** - Main Development
- **Community** - Third-party packages

## 🔗 Links

- [API Documentation](https://docs.lightos.org )
- [Package Repository](https://packages.lightos.org )
- [GitHub](https://github.com/NEXA-Laboratories/LightOS )
- [Discord Community](https://discord.gg/AQXVHsXmEe )

---

**LightOS** - The future begins here! 🚀
