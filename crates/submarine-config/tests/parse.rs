//! Integration tests for parsing, serializing and redacting wg-quick configurations.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use submarine_config::{ConfigError, Endpoint, HIDDEN, parse, parse_edited};

// sample keys (valid base64, 32 bytes each)
const PRIVATE: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
const PUBLIC: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";
const PSK: &str = "FpCyhws9cxwWoV4xELtfJvjJN+zQVRPISllRWgeopVE=";

/// A realistic configuration exercising every supported directive, a hook script,
/// mixed-case section names and keys, and an inline `;` comment.
fn full_config() -> String {
    format!(
        "# Exported by provider
[Interface]
PrivateKey = {PRIVATE}
Address = 10.8.0.2/32, fd00::2/128
DNS = 1.1.1.1, 2606:4700:4700::1111, corp.example
MTU = 1420
PostUp = iptables -A FORWARD -j ACCEPT

[Peer]
PublicKey = {PUBLIC}
PresharedKey = {PSK}
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = vpn.example.com:51820
PersistentKeepalive = 25

[peer]
publickey = {PUBLIC}
allowedips = 192.168.10.0/24 ; lan
endpoint = [2001:db8::1]:51820
"
    )
}

// every directive of the full configuration is parsed into the model
#[test]
fn parses_full_config() {
    let parsed = parse(&full_config()).unwrap();
    let cfg = parsed.config;

    assert_eq!(cfg.interface.private_key.to_base64(), PRIVATE);
    assert_eq!(cfg.interface.addresses.len(), 2);
    assert_eq!(cfg.interface.dns_servers.len(), 2);
    assert_eq!(cfg.interface.dns_search, vec!["corp.example"]);
    assert_eq!(cfg.interface.mtu, Some(1420));

    assert_eq!(cfg.peers.len(), 2);
    let first = &cfg.peers[0];
    assert!(first.is_default_route());
    assert_eq!(first.preshared_key.as_ref().unwrap().to_base64(), PSK);
    assert_eq!(
        first.endpoint,
        Some(Endpoint::Host {
            host: "vpn.example.com".into(),
            port: 51820
        })
    );
    assert_eq!(first.persistent_keepalive, Some(25));

    let second = &cfg.peers[1];
    assert!(!second.is_default_route());
    assert_eq!(
        second.endpoint,
        Some(Endpoint::Addr(
            "[2001:db8::1]:51820".parse::<SocketAddr>().unwrap()
        ))
    );
}

// the `PostUp` hook is NOT executed: it only produces a warning on its line
#[test]
fn hook_scripts_are_dropped_with_warning() {
    let parsed = parse(&full_config()).unwrap();
    assert_eq!(parsed.warnings.len(), 1);
    assert_eq!(parsed.warnings[0].line, 7);
    assert!(parsed.warnings[0].message.contains("PostUp"));
}

// serializing and parsing again gives back the same configuration, without warnings
#[test]
fn round_trips_through_conf_string() {
    let original = parse(&full_config()).unwrap().config;
    let reparsed = parse(&original.to_conf_string()).unwrap();
    assert_eq!(original, reparsed.config);
    assert!(reparsed.warnings.is_empty());
}

// the redacted form hides secrets and is restored by `parse_edited`
#[test]
fn redacted_config_hides_keys_and_reads_back() {
    let original = parse(&full_config()).unwrap().config;
    let redacted = original.to_redacted_conf_string();
    assert!(!redacted.contains(PRIVATE) && !redacted.contains(PSK));
    assert!(redacted.contains(&format!("PrivateKey = {HIDDEN}")));
    assert!(redacted.contains(&format!("PresharedKey = {HIDDEN}")));
    assert_eq!(parse_edited(&redacted, &original).unwrap().config, original);
    // without the previous configuration the placeholder is not a key
    assert!(parse(&redacted).is_err());
}

// edited keys replace the old ones, hidden ones are kept from the previous configuration
#[test]
fn edited_config_replaces_keys_and_rejects_unknown_hidden_ones() {
    let original = parse(&full_config()).unwrap().config;
    let other = "cHJpdmF0ZWtleXByaXZhdGVrZXlwcml2YXRla2V5MDA=";
    let edited = original
        .to_redacted_conf_string()
        .replacen(
            &format!("PrivateKey = {HIDDEN}"),
            &format!("PrivateKey = {other}"),
            1,
        )
        .replace("MTU = 1420", "MTU = 1380");
    let config = parse_edited(&edited, &original).unwrap().config;
    assert_eq!(config.interface.private_key.to_base64(), other);
    assert_eq!(config.interface.mtu, Some(1380));
    assert_eq!(
        config.peers[0].preshared_key,
        original.peers[0].preshared_key
    );

    // a hidden preshared key on a peer that had none cannot be restored
    let new_peer = format!(
        "{}\n[Peer]\nPublicKey = {PRIVATE}\nPresharedKey = {HIDDEN}\nAllowedIPs = 10.9.0.0/24\n",
        original.to_redacted_conf_string()
    );
    assert!(matches!(
        parse_edited(&new_peer, &original),
        Err(ConfigError::InvalidValue { key, .. }) if key == "PresharedKey"
    ));
}

// addresses without a prefix length become /32 (or /128) host routes
#[test]
fn bare_address_becomes_host_route() {
    let input = format!(
        "[Interface]\nPrivateKey = {PRIVATE}\nAddress = 10.0.0.5\n[Peer]\nPublicKey = {PUBLIC}\nAllowedIPs = 10.0.0.1\n"
    );
    let cfg = parse(&input).unwrap().config;
    assert_eq!(cfg.interface.addresses[0].to_string(), "10.0.0.5/32");
    assert_eq!(
        cfg.peers[0].allowed_ips[0].addr(),
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
    );
}

// both `off` and `0` disable the persistent keepalive
#[test]
fn keepalive_off_or_zero_disables_it() {
    for value in ["off", "0"] {
        let input = format!(
            "[Interface]\nPrivateKey = {PRIVATE}\n[Peer]\nPublicKey = {PUBLIC}\nPersistentKeepalive = {value}\n"
        );
        assert_eq!(
            parse(&input).unwrap().config.peers[0].persistent_keepalive,
            None
        );
    }
}

// a key that is not base64 is an invalid value on its own line
#[test]
fn rejects_invalid_key() {
    let input = format!("[Interface]\nPrivateKey = not-a-key\n[Peer]\nPublicKey = {PUBLIC}\n");
    assert!(matches!(
        parse(&input),
        Err(ConfigError::InvalidValue { line: 2, .. })
    ));
}

// a key that decodes to fewer than 32 bytes is rejected
#[test]
fn rejects_short_key() {
    let input = format!("[Interface]\nPrivateKey = AAAA\n[Peer]\nPublicKey = {PUBLIC}\n");
    assert!(parse(&input).is_err());
}

// each mandatory section or key has its own error
#[test]
fn rejects_missing_sections_and_keys() {
    let no_iface = format!("[Peer]\nPublicKey = {PUBLIC}\n");
    assert_eq!(parse(&no_iface).unwrap_err(), ConfigError::MissingInterface);

    let no_peers = format!("[Interface]\nPrivateKey = {PRIVATE}\n");
    assert_eq!(parse(&no_peers).unwrap_err(), ConfigError::NoPeers);

    let no_private = format!("[Interface]\nAddress = 10.0.0.2/32\n[Peer]\nPublicKey = {PUBLIC}\n");
    assert_eq!(
        parse(&no_private).unwrap_err(),
        ConfigError::MissingPrivateKey
    );

    let no_public =
        format!("[Interface]\nPrivateKey = {PRIVATE}\n\n[Peer]\nAllowedIPs = 0.0.0.0/0\n");
    assert_eq!(
        parse(&no_public).unwrap_err(),
        ConfigError::MissingPublicKey { line: 4 }
    );
}

// directives outside sections, missing `=` and invalid hostnames are rejected
#[test]
fn rejects_malformed_lines() {
    let outside = "PrivateKey = x\n";
    assert!(matches!(
        parse(outside),
        Err(ConfigError::Syntax { line: 1, .. })
    ));

    // the `=` of the base64 padding must not be taken as the separator
    let no_equals = format!("[Interface]\nPrivateKey {PRIVATE}\n");
    assert!(matches!(
        parse(&no_equals),
        Err(ConfigError::Syntax { line: 2, .. })
    ));

    let bad_endpoint = format!(
        "[Interface]\nPrivateKey = {PRIVATE}\n[Peer]\nPublicKey = {PUBLIC}\nEndpoint = host with spaces:1\n"
    );
    assert!(matches!(
        parse(&bad_endpoint),
        Err(ConfigError::InvalidValue { line: 5, .. })
    ));
}

// secret keys never appear in `Debug` output, public keys do
#[test]
fn private_key_is_redacted_in_debug() {
    let cfg = parse(&full_config()).unwrap().config;
    let debug = format!("{cfg:?}");
    assert!(!debug.contains(PRIVATE));
    assert!(!debug.contains(PSK));
    assert!(debug.contains(PUBLIC));
}
