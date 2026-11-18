use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::RngExt;

const ID_ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";

/// Short random id for URLs (no ambiguous characters).
pub fn short_id(len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| ID_ALPHABET[rng.random_range(0..ID_ALPHABET.len())] as char)
        .collect()
}

/// 256-bit secret token as hex.
pub fn secret_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    hex::encode(bytes)
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Addresses that cannot belong to a viewer on the public internet.
pub fn is_local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || {
                // 100.64.0.0/10 carrier-grade NAT is not "local" for us.
                false
            }
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_local_ip(IpAddr::V4(v4));
            }
            v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_ids_use_alphabet() {
        let id = short_id(10);
        assert_eq!(id.len(), 10);
        assert!(id.bytes().all(|b| ID_ALPHABET.contains(&b)));
    }

    #[test]
    fn local_ips() {
        assert!(is_local_ip("192.168.1.5".parse().unwrap()));
        assert!(is_local_ip("10.0.0.1".parse().unwrap()));
        assert!(is_local_ip("::ffff:172.16.0.3".parse().unwrap()));
        assert!(!is_local_ip("93.184.216.34".parse().unwrap()));
        assert!(!is_local_ip("2a00:1450:4010::66".parse().unwrap()));
    }
}
