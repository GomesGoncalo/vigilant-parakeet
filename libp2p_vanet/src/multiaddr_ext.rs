use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;
use mac_address::MacAddress;

/// Encode a MAC address as a `/memory/<u64>` multiaddr.
///
/// MAC bytes are packed into a u64 (little-endian, upper 2 bytes zero) so
/// the address is expressible without registering a custom protocol code.
pub fn mac_to_multiaddr(mac: MacAddress) -> Multiaddr {
    Multiaddr::empty().with(Protocol::Memory(mac_to_u64(mac)))
}

/// Extract a MAC address from a `/memory/<u64>` multiaddr, if present.
pub fn multiaddr_to_mac(addr: &Multiaddr) -> Option<MacAddress> {
    for proto in addr.iter() {
        if let Protocol::Memory(val) = proto {
            return Some(u64_to_mac(val));
        }
    }
    None
}

fn mac_to_u64(mac: MacAddress) -> u64 {
    let b = mac.bytes();
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], 0, 0])
}

fn u64_to_mac(val: u64) -> MacAddress {
    let bytes = val.to_le_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]].into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mac: MacAddress = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff].into();
        let addr = mac_to_multiaddr(mac);
        let recovered = multiaddr_to_mac(&addr).expect("should decode");
        assert_eq!(mac, recovered);
    }

    #[test]
    fn zero_mac() {
        let mac: MacAddress = [0u8; 6].into();
        let addr = mac_to_multiaddr(mac);
        let recovered = multiaddr_to_mac(&addr).expect("should decode");
        assert_eq!(mac, recovered);
    }

    #[test]
    fn broadcast_mac() {
        let mac: MacAddress = [0xff; 6].into();
        let addr = mac_to_multiaddr(mac);
        let recovered = multiaddr_to_mac(&addr).expect("should decode");
        assert_eq!(mac, recovered);
    }
}
