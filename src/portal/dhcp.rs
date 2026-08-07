//! Minimal DHCP server for the provisioning hotspot.
//!
//! One client, one fixed lease: the phone that joins the SoftAP gets
//! 192.168.4.2. Parses just enough of RFC 2131 (DISCOVER -> OFFER,
//! REQUEST -> ACK) and nothing more.

use super::{AP_IP, CLIENT_IP, NETMASK};

/// DHCP magic cookie after the fixed header.
const MAGIC: [u8; 4] = [0x63, 0x82, 0x53, 0x63];
/// Fixed BOOTP header length up to the options.
const OPTIONS_AT: usize = 240;

const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_LEASE_TIME: u8 = 51;
const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_DNS: u8 = 6;
const OPT_END: u8 = 255;

const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const ACK: u8 = 5;

/// A parsed client request worth answering.
pub struct DhcpRequest {
    transaction_id: [u8; 4],
    client_mac: [u8; 16],
    message_type: u8,
}

/// Parse a datagram from a client; None when it is not a DHCP request we
/// answer (replies, malformed, unknown message types).
pub fn parse_request(data: &[u8]) -> Option<DhcpRequest> {
    if data.len() < OPTIONS_AT || data[0] != 1 || data[236..240] != MAGIC {
        return None;
    }
    let mut transaction_id = [0u8; 4];
    transaction_id.copy_from_slice(&data[4..8]);
    let mut client_mac = [0u8; 16];
    client_mac.copy_from_slice(&data[28..44]);

    // Walk the options for the message type.
    let mut at = OPTIONS_AT;
    while at + 1 < data.len() {
        let option = data[at];
        if option == OPT_END {
            break;
        }
        if option == 0 {
            at += 1;
            continue;
        }
        let len = data[at + 1] as usize;
        if option == OPT_MESSAGE_TYPE && len == 1 && at + 2 < data.len() {
            let message_type = data[at + 2];
            if message_type == DISCOVER || message_type == REQUEST {
                return Some(DhcpRequest { transaction_id, client_mac, message_type });
            }
            return None;
        }
        at += 2 + len;
    }
    None
}

/// Build the OFFER/ACK reply. Returns the datagram length in `out`
/// (broadcast it to 255.255.255.255:68).
pub fn build_reply(request: &DhcpRequest, out: &mut [u8; 300]) -> usize {
    out.fill(0);
    out[0] = 2; // BOOTREPLY
    out[1] = 1; // Ethernet
    out[2] = 6; // MAC length
    out[4..8].copy_from_slice(&request.transaction_id);
    out[10] = 0x80; // broadcast flag
    out[16..20].copy_from_slice(&CLIENT_IP); // yiaddr
    out[20..24].copy_from_slice(&AP_IP); // siaddr
    out[28..44].copy_from_slice(&request.client_mac);
    out[236..240].copy_from_slice(&MAGIC);

    let reply_type = if request.message_type == DISCOVER { OFFER } else { ACK };
    let mut at = OPTIONS_AT;
    for (option, value) in [
        (OPT_MESSAGE_TYPE, &[reply_type][..]),
        (OPT_SERVER_ID, &AP_IP[..]),
        (OPT_LEASE_TIME, &3600u32.to_be_bytes()[..]),
        (OPT_SUBNET_MASK, &NETMASK[..]),
        (OPT_ROUTER, &AP_IP[..]),
        (OPT_DNS, &AP_IP[..]),
    ] {
        out[at] = option;
        out[at + 1] = value.len() as u8;
        out[at + 2..at + 2 + value.len()].copy_from_slice(value);
        at += 2 + value.len();
    }
    out[at] = OPT_END;
    at + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn discover(message_type: u8) -> [u8; 260] {
        let mut data = [0u8; 260];
        data[0] = 1; // BOOTREQUEST
        data[4..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        data[28..34].copy_from_slice(&[2, 4, 6, 8, 10, 12]);
        data[236..240].copy_from_slice(&MAGIC);
        data[240] = OPT_MESSAGE_TYPE;
        data[241] = 1;
        data[242] = message_type;
        data[243] = OPT_END;
        data
    }

    #[test]
    fn discover_gets_an_offer_with_the_fixed_lease() {
        let request = parse_request(&discover(DISCOVER)).expect("valid DISCOVER");
        let mut out = [0u8; 300];
        let len = build_reply(&request, &mut out);
        assert!(len > OPTIONS_AT);
        assert_eq!(out[0], 2, "BOOTREPLY");
        assert_eq!(&out[4..8], &[0xDE, 0xAD, 0xBE, 0xEF], "xid echoed");
        assert_eq!(&out[16..20], &CLIENT_IP, "yiaddr is the fixed lease");
        assert_eq!(&out[28..34], &[2, 4, 6, 8, 10, 12], "chaddr echoed");
        assert_eq!(out[242], OFFER);
    }

    #[test]
    fn request_gets_an_ack() {
        let request = parse_request(&discover(REQUEST)).expect("valid REQUEST");
        let mut out = [0u8; 300];
        build_reply(&request, &mut out);
        assert_eq!(out[242], ACK);
    }

    #[test]
    fn non_requests_are_ignored() {
        // A BOOTREPLY (op=2) must not be answered.
        let mut reply = discover(DISCOVER);
        reply[0] = 2;
        assert!(parse_request(&reply).is_none());
        // Unknown message type (RELEASE=7).
        assert!(parse_request(&discover(7)).is_none());
        // Truncated.
        assert!(parse_request(&[1u8; 100]).is_none());
    }
}
