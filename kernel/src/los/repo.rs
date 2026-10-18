//! Built-in package repository (used offline / before `los update`).
use super::{hash, Origin, PackageMeta};
use alloc::{string::ToString, vec::Vec};

pub struct Builtin {
    pub name: &'static str,
    pub version: &'static str,
    pub author: &'static str,
    pub description: &'static str,
    pub verified: bool,
    pub deps: &'static [&'static str],
    pub payload: &'static str,
}

impl Builtin {
    pub fn to_meta(&self) -> PackageMeta {
        PackageMeta {
            name: self.name.to_string(),
            version: self.version.to_string(),
            author: self.author.to_string(),
            description: self.description.to_string(),
            verified: self.verified,
            deps: self.deps.iter().map(|d| d.to_string()).collect::<Vec<_>>(),
            hash: hash::fnv1a64(self.payload.as_bytes()),
            origin: Origin::Builtin,
        }
    }
}

pub const BUILTIN: &[Builtin] = &[
    Builtin {
        name: "los-core",
        version: "0.1.0",
        author: "LightOS Team",
        description: "Core utilities",
        verified: true,
        deps: &[],
        payload: "los-core 0.1.0\ncore utilities: echo, uptime, mem, ps\n",
    },
    Builtin {
        name: "los-shell",
        version: "0.1.0",
        author: "LightOS Team",
        description: "Command shell",
        verified: true,
        deps: &["los-core"],
        payload: "los-shell 0.1.0\ninteractive command shell\n",
    },
    Builtin {
        name: "los-edit",
        version: "0.1.0",
        author: "LightOS Team",
        description: "Text editor",
        verified: true,
        deps: &["los-core"],
        payload: "los-edit 0.1.0\nsimple text editor\n",
    },
    Builtin {
        name: "los-net",
        version: "0.1.0",
        author: "LightOS Team",
        description: "Network utilities",
        verified: true,
        deps: &["los-core"],
        payload: "los-net 0.1.0\nnetwork utilities (driver support arrives in 0.2.0)\n",
    },
    Builtin {
        name: "demo-app",
        version: "1.0.0",
        author: "Community",
        description: "Demo application",
        verified: false,
        deps: &["los-core"],
        payload: "demo-app 1.0.0\ndemonstration application\n",
    },
    Builtin {
        name: "hello-elf",
        version: "0.3.0",
        author: "LightOS Team",
        description: "Hello world ELF64 demo app",
        verified: true,
        deps: &["los-core"],
        payload: super::hello_elf::PAYLOAD,
    },
];
