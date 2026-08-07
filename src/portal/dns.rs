//! Catch-all DNS responder for the provisioning hotspot.
//!
//! Answers every A query with the portal address so phones raise their
//! captive-portal sheet and any typed hostname lands on the config page.

use super::AP_IP;

/// Build the answer for one query datagram. Returns the reply length in
/// `out`, or None when the datagram is not a plain query.
pub fn answer(query: &[u8], out: &mut [u8; 512]) -> Option<usize> {
    // Header: id(2) flags(2) qdcount(2) ancount(2) nscount(2) arcount(2)
    if query.len() < 12 || query.len() > out.len() - 16 {
        return None;
    }
    let is_response = query[2] & 0x80 != 0;
    let question_count = u16::from_be_bytes([query[4], query[5]]);
    if is_response || question_count == 0 {
        return None;
    }

    // Find the end of the first question (name, then type+class).
    let mut at = 12;
    while at < query.len() && query[at] != 0 {
        at += query[at] as usize + 1;
    }
    let question_end = at + 5; // zero byte + type(2) + class(2)
    if question_end > query.len() {
        return None;
    }

    // Echo id + question, set response flags, one answer.
    out[..question_end].copy_from_slice(&query[..question_end]);
    out[2] = 0x81; // response, recursion desired
    out[3] = 0x80; // recursion available, no error
    out[4] = 0;
    out[5] = 1; // one question
    out[6] = 0;
    out[7] = 1; // one answer
    out[8..12].fill(0);

    // Answer: pointer to the question name, A IN TTL=60, the portal IP.
    let answer: [u8; 16] = [
        0xC0, 0x0C, // name pointer to offset 12
        0x00, 0x01, // type A
        0x00, 0x01, // class IN
        0x00, 0x00, 0x00, 0x3C, // TTL 60s
        0x00, 0x04, // rdlength
        AP_IP[0], AP_IP[1], AP_IP[2], AP_IP[3],
    ];
    out[question_end..question_end + 16].copy_from_slice(&answer);
    Some(question_end + 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A query for connectivitycheck.gstatic.com-style name "a.b".
    fn query() -> Vec<u8> {
        let mut q = vec![
            0x12, 0x34, // id
            0x01, 0x00, // standard query, RD
            0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        q.extend_from_slice(&[1, b'a', 1, b'b', 0]); // name "a.b"
        q.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]); // A IN
        q
    }

    #[test]
    fn answers_with_the_portal_ip() {
        let mut out = [0u8; 512];
        let len = answer(&query(), &mut out).expect("query should be answered");
        assert_eq!(&out[..2], &[0x12, 0x34], "id echoed");
        assert_eq!(out[2] & 0x80, 0x80, "response bit set");
        assert_eq!(&out[len - 4..len], &AP_IP, "A record is the portal IP");
    }

    #[test]
    fn responses_and_garbage_are_ignored() {
        let mut out = [0u8; 512];
        let mut response = query();
        response[2] |= 0x80;
        assert!(answer(&response, &mut out).is_none());
        assert!(answer(&[0u8; 5], &mut out).is_none());
    }
}
