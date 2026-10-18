#!/bin/sh
# Boot LightOS in QEMU with the data disk. SERVER=1 wires COM1 to los-server:
#   python3 server/los_server.py serve     (other terminal)
#   SERVER=1 scripts/run.sh
cd "$(dirname "$0")/.."
SERIAL=stdio; [ -n "$SERVER" ] && SERIAL=tcp:127.0.0.1:7878
exec qemu-system-x86_64 -drive format=raw,file=kernel/target/x86_64-lightos/release/bootimage-lightos.bin \
  -drive file=data.img,format=raw,if=ide,index=1 -nic user,model=e1000 -serial $SERIAL -m 128M "$@"
