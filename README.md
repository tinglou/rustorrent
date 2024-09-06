# rustorrent
> A BitTorrent library implemented in Rust

![Last commit](https://img.shields.io/github/last-commit/tinglou/rustorrent?style=flat-square)
![Coverage](https://img.shields.io/codecov/c/github/tinglou/rustorrent?style=flat-square)
![Status](https://img.shields.io/badge/status-in%20development-orange?style=flat-square)
![rust toolchain](https://img.shields.io/badge/rust-stable-blue?style=flat-square)

---

Rustorrent is intented to be a full featured BitTorrent implementation.  
It is in active development and is not usable yet. The API might change any time.

## Notable features
- Supports [io_uring](https://unixism.net/loti/what_is_io_uring.html) for file based IO (linux only)
- Uses intel [SHA extensions](https://software.intel.com/content/www/us/en/develop/articles/intel-sha-extensions.html)
- Full [utp](http://www.bittorrent.org/beps/bep_0029.html) implementation, no library used

## Implemented [BEPs](https://www.bittorrent.org/beps/bep_0000.html)
- [The BitTorrent Protocol Specification](https://www.bittorrent.org/beps/bep_0003.html)
- [Extension Protocol](https://www.bittorrent.org/beps/bep_0010.html)
- [Peer Exchange PEX](https://www.bittorrent.org/beps/bep_0011.html)
- [Multitracker Metadata Extension](https://www.bittorrent.org/beps/bep_0012.html)
- [UDP Tracker Protocol](https://www.bittorrent.org/beps/bep_0015.html)
- [Tracker Returns Compact Peer Lists](https://www.bittorrent.org/beps/bep_0023.html)
- [uTorrent transport protocol](https://www.bittorrent.org/beps/bep_0029.html)
- [IPv6 Tracker Extension](https://www.bittorrent.org/beps/bep_0007.html)

As noted, the library is not usable yet, though you might try it with:
```
$ cargo run scripts/Fedora-Workstation-Live-x86_64-33.torrent
```
