# Networking in LightOS 0.4.0

## What works
- PCI bus scan (`pci`)
- Intel e1000 (82540EM) driver: DMA rings, polled receive/transmit, MAC from the card
- Ethernet, ARP (request/reply + cache), IPv4, ICMP echo: **`ping`** works both ways
  (LightOS also answers pings sent to it)
- Static addressing: default `10.0.2.15 / 255.255.255.0`, gateway `10.0.2.2` (the VirtualBox NAT and QEMU
  user-mode defaults), change with `ifconfig <ip> [mask] [gateway]`

## What is still missing (next releases)
UDP + DHCP + DNS, TCP, HTTP, and `los update` over the network instead of the COM1 bridge.

## VirtualBox
The VM needs a NAT adapter of type **Intel PRO/1000 MT Desktop**:
```powershell
VBoxManage modifyvm LightOS --nic1 nat --nictype1 82540EM --cableconnected1 on
```
(`scripts\vbox-create.ps1` sets this for new VMs.) Then in LightOS:
```
los> ifconfig             # shows MAC, link up, 10.0.2.15
los> ping 10.0.2.2        # the NAT gateway answers
los> ping 8.8.8.8         # goes through the NAT to the internet
los> arp
```
## QEMU
`-nic user,model=e1000` (already in scripts\run.ps1) gives the same 10.0.2.x network.

## Troubleshooting
- Boot log says `network: no Intel e1000 network card`: wrong adapter type (must be 82540EM, not PCnet/virtio), or the adapter is disabled.
- `link DOWN`: cable not connected in the VM settings.
- `ping` times out but `arp` shows the gateway: ICMP is blocked by the host/NAT; try `ping 10.0.2.2` first.
- Send me the output of `pci` and `ifconfig` plus a screenshot of the boot log.
