//! Gateway EUI-64 derivation.

/// MAC-48 to EUI-64 with the standard FFFE insertion, the convention LoRaWAN
/// single-channel forwarders use for their gateway id.
pub fn gateway_eui(mac: &[u8; 6]) -> [u8; 8] {
    [mac[0], mac[1], mac[2], 0xFF, 0xFE, mac[3], mac[4], mac[5]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_fffe_in_the_middle() {
        let eui = gateway_eui(&[0x24, 0x6F, 0x28, 0xAA, 0xBB, 0xCC]);
        assert_eq!(eui, [0x24, 0x6F, 0x28, 0xFF, 0xFE, 0xAA, 0xBB, 0xCC]);
    }
}
