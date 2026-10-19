# Networking in LightOS 0.4.2

## What works
- PCI bus scan (`pci`)
- Intel e1000 (82540EM) driver: DMA rings, polled receive/transmit, MAC from the card
- Ethernet, ARP (request/reply + cache), IPv4, ICMP echo: **`ping`** works both ways
  (LightOS also answers pings sent to it)
- **UDP** sockets (listen / send / receive queue)
- **DHCP client**: runs at boot and with `dhcp`; applies address, mask, gateway and DNS server
- **DNS resolver** (A records): `nslookup <host>`, and `ping` accepts host names
- **TCP client** (handshake, in-order receive, stop-and-wait send with retransmit, FIN close) - one connection at a time
- **HTTP/1.1 client** (GET, Content-Length and chunked bodies; no TLS/HTTPS, no redirects): `wget <http://host/path> [file]`
- **`los update` / `los install` over HTTP**: LightOS talks to `los-server` at `10.0.2.2:8080` (= the host as seen from
  VirtualBox NAT and QEMU user networking), and falls back to the COM1 bridge if the network or server is unavailable.
  Change the address with `los server <ip[:port]>`.
- Static addressing as fallback: `10.0.2.15 / 255.255.255.0`, gateway `10.0.2.2`, DNS `10.0.2.3`
  (the VirtualBox NAT and QEMU defaults); change with `ifconfig <ip> [mask] [gateway]`

## What is still missing (next releases)
HTTPS/TLS, redirects, a listening TCP socket (servers), IPv6.

## Using los-server over the network
```powershell
python server\los_server.py serve        # on the host, before or after starting the VM
```
```
los> los update           # "Repository updated: 6 package(s) from los-server (network 10.0.2.2:8080)"
los> los install hello-elf
los> wget http://example.com
```
The server window logs `[http] GET /api/v1/index.txt` for every request, which proves the traffic goes over the network.

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
los> dhcp                 # ask the DHCP server again
los> nslookup example.com
los> ping example.com
```
## QEMU
`-nic user,model=e1000` (already in scripts\run.ps1) gives the same 10.0.2.x network.

## Troubleshooting
- Boot log says `network: no Intel e1000 network card`: wrong adapter type (must be 82540EM, not PCnet/virtio), or the adapter is disabled.
- `link DOWN`: cable not connected in the VM settings.
- `ping` times out but `arp` shows the gateway: ICMP is blocked by the host/NAT; try `ping 10.0.2.2` first.
- Send me the output of `pci` and `ifconfig` plus a screenshot of the boot log.
