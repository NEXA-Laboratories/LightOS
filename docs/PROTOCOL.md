# LOS serial protocol v1

The kernel has no network stack in 0.1.0 (TCP/IP is planned for 0.2.0), so `los update`
and `los install` talk to `los-server` over **COM1**. QEMU connects COM1 to the server's
bridge port with `-serial tcp:127.0.0.1:7878`. One line per message, `\n` terminated.
Lines that do not start with `LOS/1 ` are treated as kernel log output and printed by the server.

| Kernel sends        | Server replies |
|---------------------|----------------|
| `LOS/1 PING`        | `PONG` |
| `LOS/1 LIST`        | `PKG|name|version|author|description|verified(0/1)|deps(comma)|hash(16 hex)|size` per package, then `END` |
| `LOS/1 SEARCH <q>`  | same as LIST, filtered |
| `LOS/1 GET <name>`  | `DATA|<name>|<payload>` with `\\`, `\n`, `\r`, `\t` escaped, or `ERR|<message>` |

Integrity: `hash` is FNV-1a 64 over the raw payload bytes. The kernel recomputes it after
`GET` and refuses to install on mismatch. Payloads are plain ASCII, max 32 KiB.

If the server does not answer, the kernel falls back to its built-in repository.

## HTTP API

| Method & path | Notes |
|---|---|
| `GET /health` | status |
| `GET /api/v1/packages` | all packages (JSON) |
| `GET /api/v1/packages/<name>` | one package |
| `GET /api/v1/packages/<name>/payload` | raw payload |
| `GET /api/v1/search?q=` | name/description search |
| `POST /api/v1/submit` | body = `package.toml`, a line `---payload---`, then the payload. Runs the automatic security check; accepted packages are **unverified** until a maintainer reviews them |
| `POST /api/v1/verify/<name>` | maintainer only, header `X-Admin-Token` |
| `DELETE /api/v1/packages/<name>` | maintainer only; refused while other packages depend on it |

## Automatic security check (on submit)
name format, semantic version (must increase on update), license (MIT / Apache-2.0),
description/author sanity, every dependency must exist, no dependency cycles,
non-empty ASCII payload <= 32 KiB. Warns on `unsafe` and on payloads not mentioning `no_std`.
