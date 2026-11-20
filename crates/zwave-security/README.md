# zwave-security

Z-Wave security for host stacks: Security 0 and Security 2.

This crate holds the parts of Z-Wave security that a controller (or a
simulated node) runs on the host:

- `s2`: Security 2 key derivation (CKDF), the singlecast and multicast
  nonce state (SPAN and MPAN) on top of `drbg::CtrDrbg`, and the
  Message Encapsulation format with its extensions, all per the
  Z-Wave Alliance's Security 2 specification.
- `s0`: Security 0 nonces and Message Encapsulation (AES-OFB with a
  CBC-MAC).
- `kex`: the key exchange constants and the Curve25519 key pair.
- `dsk`: Device Specific Keys, PINs and the SmartStart home ids derived
  from them.
- `qr` and `provisioning`: SmartStart QR codes and the provisioning
  list a controller keeps.

The block cipher, CMAC, CCM, OFB, CBC-MAC and X25519 come from maintained
crates (RustCrypto, dalek). The only construction written here is the
AES-128 CTR_DRBG without a derivation function that S2 nonces need,
checked against NIST's CAVP vectors.

Part of [zwave-bridge](https://github.com/pulzar-net/zwave-bridge), maintained by
[Pulzar](https://pulzr.net). See the [user guide](https://pulzar-net.github.io/zwave-bridge/), the
[library guide](https://pulzar-net.github.io/zwave-bridge/library/index.html) and the
[API documentation](https://pulzar-net.github.io/zwave-bridge/api/zwave_security/index.html).

License: MIT.
