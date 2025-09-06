use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use anyhow::{anyhow, Result};

/// Fixed key for initial implementation. In production, this would be exchanged securely.
const FIXED_KEY: &[u8; 32] = b"vigilant_parakeet_fixed_key_256!";

/// Encrypt plaintext using AES-256-GCM with a random nonce.
/// Returns encrypted data with nonce prepended (12 bytes nonce + ciphertext).
pub fn encrypt_payload(plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(FIXED_KEY));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| anyhow!("Encryption failed: {}", e))?;

    // Prepend nonce to ciphertext for transmission
    let mut result = nonce.to_vec();
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// Decrypt data that was encrypted with encrypt_payload.
/// Expects nonce (12 bytes) + ciphertext.
pub fn decrypt_payload(encrypted_data: &[u8]) -> Result<Vec<u8>> {
    if encrypted_data.len() < 12 {
        return Err(anyhow!("Encrypted data too short (missing nonce)"));
    }

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(FIXED_KEY));
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow!("Decryption failed: {}", e))?;

    Ok(plaintext)
}

/// Check if data follows the custom application protocol format.
/// Returns true if data appears to be application protocol data (MAC+MAC+payload),
/// false if it appears to be IP packets or other network traffic.
pub fn is_application_protocol_data(data: &[u8]) -> bool {
    // Application protocol format: [dest_mac(6) + src_mac(6) + payload]
    // Must be at least 12 bytes to contain both MAC addresses
    if data.len() < 12 {
        return false;
    }

    // IP packets start with version field (4 bits) = 4 for IPv4 or 6 for IPv6
    // In the first byte: top 4 bits = version, bottom 4 bits = IHL
    let first_byte = data[0];
    let ip_version = (first_byte >> 4) & 0x0F;

    // If it looks like an IP packet (version 4 or 6), it's not application protocol data
    if ip_version == 4 || ip_version == 6 {
        return false;
    }

    // Additional heuristic: check if first 12 bytes could be valid MAC addresses
    // MAC addresses can be any value, but all-zero or all-0xFF in both positions is unlikely
    let dest_mac = &data[0..6];
    let src_mac = &data[6..12];

    // Reject if both MACs are all zeros or all 0xFF (very unlikely for real traffic)
    let dest_all_zero = dest_mac.iter().all(|&b| b == 0);
    let src_all_zero = src_mac.iter().all(|&b| b == 0);
    let dest_all_ff = dest_mac.iter().all(|&b| b == 0xFF);
    let src_all_ff = src_mac.iter().all(|&b| b == 0xFF);

    if (dest_all_zero && src_all_zero) || (dest_all_ff && src_all_ff) {
        return false;
    }

    // If we get here, it's likely application protocol data
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"test payload data";
        let encrypted = encrypt_payload(plaintext).expect("encryption failed");
        let decrypted = decrypt_payload(&encrypted).expect("decryption failed");
        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn encrypt_produces_different_ciphertext() {
        let plaintext = b"test payload data";
        let encrypted1 = encrypt_payload(plaintext).expect("encryption failed");
        let encrypted2 = encrypt_payload(plaintext).expect("encryption failed");
        // Should be different due to random nonce
        assert_ne!(encrypted1, encrypted2);
    }

    #[test]
    fn decrypt_invalid_data_fails() {
        let invalid_data = b"too short";
        assert!(decrypt_payload(invalid_data).is_err());
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let plaintext = b"test data";
        let encrypted = encrypt_payload(plaintext).expect("encryption failed");

        // Modify the ciphertext to simulate wrong key/corruption
        let mut corrupted = encrypted;
        corrupted[15] ^= 0x01; // Flip a bit in the ciphertext

        assert!(decrypt_payload(&corrupted).is_err());
    }

    #[test]
    fn is_application_protocol_data_detects_format() {
        // Test application protocol format (MAC+MAC+payload)
        let mut app_data = Vec::new();
        app_data.extend_from_slice(&[1, 2, 3, 4, 5, 6]); // dest MAC
        app_data.extend_from_slice(&[10, 11, 12, 13, 14, 15]); // src MAC
        app_data.extend_from_slice(b"payload data"); // payload
        assert!(is_application_protocol_data(&app_data));

        // Test IPv4 packet (should not be considered application data)
        let ipv4_packet = [
            0x45, 0x00, 0x00, 0x20, // version=4, IHL=5, TOS=0, length=32
            0x00, 0x01, 0x40, 0x00, // ID=1, flags=0x4000, frag_offset=0
            0x40, 0x01, 0x00, 0x00, // TTL=64, protocol=1(ICMP), checksum=0
            0x7f, 0x00, 0x00, 0x01, // src IP = 127.0.0.1
            0x7f, 0x00, 0x00, 0x01, // dest IP = 127.0.0.1
        ];
        assert!(!is_application_protocol_data(&ipv4_packet));

        // Test IPv6 packet (should not be considered application data)
        let ipv6_packet = [
            0x60, 0x00, 0x00, 0x00, // version=6, traffic_class=0, flow_label=0
            0x00, 0x08, 0x3a,
            0x40, // payload_length=8, next_header=58(ICMPv6), hop_limit=64
                  // ... IPv6 addresses would follow
        ];
        assert!(!is_application_protocol_data(&ipv6_packet));

        // Test too short data
        assert!(!is_application_protocol_data(b"short"));

        // Test edge cases with all-zero or all-FF MACs (should be rejected)
        let mut bad_data1 = Vec::new();
        bad_data1.extend_from_slice(&[0; 6]); // all-zero dest MAC
        bad_data1.extend_from_slice(&[0; 6]); // all-zero src MAC
        bad_data1.extend_from_slice(b"payload");
        assert!(!is_application_protocol_data(&bad_data1));

        let mut bad_data2 = Vec::new();
        bad_data2.extend_from_slice(&[0xFF; 6]); // all-FF dest MAC
        bad_data2.extend_from_slice(&[0xFF; 6]); // all-FF src MAC
        bad_data2.extend_from_slice(b"payload");
        assert!(!is_application_protocol_data(&bad_data2));
    }
}
